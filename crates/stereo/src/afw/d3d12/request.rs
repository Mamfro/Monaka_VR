//! At the present request, before the HUD goes over the frame: the back buffer warped into the
//! other eye ([`warp`]), frame generation's frame halfway between this pair and the one before
//! generated per eye ([`generate`], the game's motion vectors rebased onto each eye's own previous
//! image from the cameras, [`motion_constants`]), and the HUD laid over the images made of the
//! frame ([`lay_hud`]).

use super::*;
use monaka_framegen::inputs::PreviousFrame;
use monaka_warp::source_matrix;


/// The real eye's inputs for its present.
pub(super) struct Inputs {
    pub(super) depth: ID3D12Resource,
    pub(super) motion: Option<ID3D12Resource>,
    pub(super) rendered: Rendered,
    pub(super) camera: Camera,
    /// The motion vectors' scale (to the screen).
    pub(super) mvec_scale: [f32; 2],
    /// The part of the depth that lines up with the image, and the jitter it was rendered with.
    pub(super) rect: Option<[u32; 4]>,
    pub(super) jitter: [f32; 2],
    /// What the game's motion vectors point at.
    pub(super) previous: PreviousFrame,
    pub(super) reset: bool,
}

/// Eye `eye`'s inputs if they came since its last present (taken).
pub(super) fn take_inputs(eye: usize) -> Option<Inputs> {
    let taken = inputs::take(eye).filter(|i| i.fresh.depth)?;
    let rendering = taken.rendering?;
    let camera = rendering.camera;
    Some(Inputs {
        depth: taken.depth?,
        motion: taken.motion,
        rendered: Rendered::from_rendering(&rendering)?,
        camera,
        mvec_scale: [camera.motion_vector_scale.x, camera.motion_vector_scale.y],
        rect: taken.rect,
        jitter: [camera.jitter.x, camera.jitter.y],
        previous: taken.previous,
        reset: taken.reset,
    })
}

/// What one present made, for its publish.
pub(super) struct Prepared {
    /// The present it is for ([`Driver::presents`] once it is counted).
    pub(super) present: u64,
    pub(super) eye: usize,
    pub(super) pose: HeadPose,
    pub(super) pair: Pair,
    /// Frame generation: copies of the pair to show half a frame later (the HUD goes over them),
    /// and the generated pair's pose when one was made (its images are the streams' outputs).
    pub(super) shown: Option<[ID3D12Resource; 2]>,
    pub(super) generated: Option<HeadPose>,
}

pub(super) struct State {
    pub(super) afw: Afw12,
    pub(super) recorder: Recorder,
    pub(super) prepared: Option<Prepared>,
    /// Our images can take the HUD: they have the back buffer's format.
    pub(super) hud_fits: bool,
    pub(super) streams: [Option<EyeStream>; 2],
    /// The motion passes: the real eye's rebase and the warped eye's carry (none if the GPU cannot).
    pub(super) passes: Option<Result<(MotionFix, MotionFix), ()>>,
    /// Each eye's motion vectors for its stream (left, right).
    pub(super) motion: [Option<ID3D12Resource>; 2],
    pub(super) shown: [[Option<ID3D12Resource>; 2]; SLOTS],
    pub(super) shown_slot: usize,
    pub(super) interval: Interval,
    /// What goes out at each present (the core's: labels, plain pairs, frame generation's history).
    pub(super) presents: afw::Presents,
    /// Each eye's last frame as presented, for a plain pair when a frame could not be warped (its
    /// pose and present are the core's).
    pub(super) plain: [Option<ID3D12Resource>; 2],
}

// SAFETY: the D3D objects are free-threaded; everything is used under the state's lock only.
unsafe impl Send for State {}

pub(super) static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The state, made on first use on the game queue's device.
pub(super) fn state_for<'a>(guard: &'a mut Option<State>, queue: &ID3D12CommandQueue) -> Option<&'a mut State> {
    if guard.is_none() {
        let device = d3d12::device_of(queue).ok()?;
        // Frame generation reads the previous pair's images at the next present: a ring of them.
        let rules = *RULES_CHOSEN.locked();
        let made = Afw12::new(&device, SLOTS, rules).and_then(|afw| Ok((afw, Recorder::new(&device, 6).map_err(|e| e.to_string())?)));
        match made {
            Ok((afw, recorder)) => {
                *guard = Some(State {
                    afw,
                    recorder,
                    prepared: None,
                    hud_fits: false,
                    streams: [None, None],
                    passes: None,
                    motion: [None, None],
                    shown: Default::default(),
                    shown_slot: 0,
                    interval: Interval::new(Duration::from_millis(16)),
                    presents: afw::Presents::new(rules),
                    plain: [None, None],
                })
            }
            Err(why) => {
                log_first(1, format_args!("frame warping unavailable: {why}"));
                return None;
            }
        }
    }
    guard.as_mut()
}

/// At the present request, before the HUD goes over `buffer` (the back buffer, in PRESENT): the
/// frame about to present warped into the other eye and, with frame generation, the in-between pair
/// generated.
pub fn warp(queue: &ID3D12CommandQueue, buffer: &ID3D12Resource) {
    if !enabled() {
        return;
    }
    let Some(game) = *GAME.locked() else { return };
    let present = game.present_warped();
    let mut guard = STATE.locked();
    let Some(state) = state_for(&mut guard, queue) else { return };
    state.prepared = None;
    let Some(label) = (game.presenting)() else { return };
    let (eye, other) = (label.eye.min(1), 1 - label.eye.min(1));
    let Some(inputs) = take_inputs(eye) else {
        Counters::add(&COUNTERS.no_inputs);
        return;
    };
    let Some(separation) = (game.separation)(&label.pose) else { return };
    let shot = Shot::labelled(label, inputs.rendered, separation, (game.frustum)(&label.pose, other));
    // SAFETY: reads a descriptor.
    let format = unsafe { buffer.GetDesc() }.Format;
    state.hud_fits = keeps_format(format);
    let real = RealEye { color: buffer, color_state: D3D12_RESOURCE_STATE_PRESENT, depth: &inputs.depth, depth_state: D3D12_RESOURCE_STATE_COMMON, depth_rect: inputs.rect, jitter: inputs.jitter };
    // How each eye was seen in the pair before, and which eye it rendered: each stream's previous
    // image, and what the game had as its previous frame.
    let before = state.afw.last().map(|pair| (pair.views, pair.real_eye));
    let pair = match state.afw.warp(queue, &shot, &real) {
        Ok(Some(pair)) => pair,
        Ok(None) => return,
        Err(why) => {
            log_first(3, format_args!("frame warping: a frame not warped: {why}"));
            return;
        }
    };
    let mut prepared = Prepared { present, eye, pose: shot.pose, pair, shown: None, generated: None };
    if framegen_running() {
        generate(state, queue, &mut prepared, &inputs, separation, before);
    }
    state.prepared = Some(prepared);
}

/// The render area of a depth of `size` with the part `rect` lining up with the image.
pub(super) fn render_size(rect: Option<[u32; 4]>, size: (u32, u32)) -> Size2 {
    match rect {
        Some([_, _, width, height]) if width > 0 && height > 0 => Size2 { width: width.min(size.0), height: height.min(size.1) },
        _ => Size2 { width: size.0, height: size.1 },
    }
}

/// Frame generation: copies of this pair to show later, and each eye's frame halfway between the
/// last pair and this one (`before`: how each eye was seen in that pair, and its real eye).
pub(super) fn generate(state: &mut State, queue: &ID3D12CommandQueue, prepared: &mut Prepared, inputs: &Inputs, separation: f32, before: Option<([Rendered; 2], usize)>) {
    let Ok(device) = d3d12::device_of(queue) else { return };
    let (eye, pair) = (prepared.eye, &prepared.pair);
    // Whether the history runs on (the core's rule), the last pair taken.
    let (previous, continuous) = state.presents.generation(eye, prepared.present, inputs.reset);
    // SAFETY: reads descriptors.
    let (frame, depth) = unsafe { (pair.images[0].GetDesc(), pair.depths[0].GetDesc()) };
    let render = render_size(pair.depth_rect, (depth.Width as u32, depth.Height));
    let Ok(Some(list)) = state.recorder.begin() else {
        skip("command lists busy");
        return;
    };
    let slot = state.shown_slot;
    state.shown_slot = (slot + 1) % SLOTS;
    let shown = [0, 1].map(|e| d3d12::fitting(&mut state.shown[slot][e], &device, frame.Width, frame.Height, frame.Format, D3D12_RESOURCE_FLAG_NONE));
    if let [Some(left), Some(right)] = shown {
        d3d12::copy_whole(&list, &left, &pair.images[0], D3D12_RESOURCE_STATE_COMMON);
        d3d12::copy_whole(&list, &right, &pair.images[1], D3D12_RESOURCE_STATE_COMMON);
        prepared.shown = Some([left, right]);
    }
    let motion = before.and_then(|(before, before_eye)| eye_motion(state, &list, &device, pair, inputs, &before, before_eye, render));
    let made = motion.map(|motion| {
        [0, 1].map(|e| {
            let camera = eye_camera(&inputs.camera, eye, e, separation).for_render(render);
            let stream = match fitting_stream(&mut state.streams[e], &state.recorder, |s| s.fits(&frame, render), || EyeStream::new(&device, &frame, render)) {
                Ok(stream) => stream,
                Err(why) => return skip(&format!("no context: {why}")),
            };
            match stream.generate(&list, &camera, &pair.depths[e], &motion[e], None, &pair.images[e], !continuous, 16.7) {
                Ok(made) => made,
                Err(why) => skip(&why),
            }
        })
    });
    if let Err(e) = state.recorder.submit(queue) {
        log_first(3, format_args!("frame warping: in-between frames not submitted: {e}"));
        return;
    }
    if made.is_some() {
        state.presents.generated(afw::Previous { pose: prepared.pose, eye, present: prepared.present });
    }
    if made == Some([true, true])
        && let Some(before) = previous
        && prepared.shown.is_some()
    {
        prepared.generated = Some(afw::halfway(&before.pose, &prepared.pose));
    }
}

/// The real eye `eye`'s camera moved to eye `e` (`separation` apart along its right axis).
pub(super) fn eye_camera(camera: &Camera, eye: usize, e: usize, separation: f32) -> Camera {
    let mut camera = *camera;
    let shift = (e as f32 - eye as f32) * separation;
    for k in 0..3 {
        camera.position[k] += camera.right[k] * shift;
    }
    camera
}

/// Each eye's motion vectors for its stream (left, right), recorded on `list` into textures of ours
/// in COMMON: the game's (the real eye's) rebased onto that eye's previous image (its own frame of
/// the pair before, warped or real), and carried into the warped eye's view onto its own image of
/// that pair ([`motion_constants`]; `before`: how each eye was seen in that pair, `before_eye` its
/// real eye). None without the game's vectors, or the frames they reach, or a GPU that runs the
/// passes.
#[allow(clippy::too_many_arguments)]
pub(super) fn eye_motion(
    state: &mut State,
    list: &ID3D12GraphicsCommandList,
    device: &ID3D12Device,
    pair: &Pair,
    inputs: &Inputs,
    before: &[Rendered; 2],
    before_eye: usize,
    render: Size2,
) -> Option<[ID3D12Resource; 2]> {
    let game = inputs.motion.as_ref()?;
    let (eye, other) = (pair.real_eye, 1 - pair.real_eye);
    let make = || MotionFix::new(device).and_then(|rebase| Ok((rebase, MotionFix::warper(device)?)));
    let (rebase, warper) = state.passes.get_or_insert_with(|| make().map_err(|why| log!("frame warping: no in-between frames: motion vectors: {why}"))).as_mut().ok()?;
    // The frame the game's vectors reach, and the object motion they carry: this eye's own frame
    // before (over as many frames as the provider says), or the frame presented before, whichever
    // eye rendered it.
    let (reached, object_scale) = match inputs.previous {
        PreviousFrame::OwnEye { frames_back } => (pair.previous[eye], 1.0 / f32::from(frames_back.max(1))),
        PreviousFrame::OtherEye => (pair.previous[if before_eye == eye { eye } else { other }], 1.0),
    };
    let constants = motion_constants(&pair.views, reached?, object_scale, eye, before, inputs.mvec_scale, [render.width, render.height])?;
    let (real_depth, warped_depth) = (&pair.depths[eye], &pair.depths[other]);
    for texture in [real_depth, warped_depth, game] {
        barrier(list, texture, D3D12_RESOURCE_STATE_COMMON, motion::READABLE);
    }
    // SAFETY: reads a descriptor.
    let desc = unsafe { game.GetDesc() };
    let recorded = rebase
        .record(list, &[real_depth, game], &constants[eye])
        .and_then(|()| rebase.output(desc.Width as u32, desc.Height))
        .and_then(|real| Ok((real, warper.record(list, &[warped_depth, real_depth, game], &constants[other]).and_then(|()| warper.output(render.width, render.height))?)));
    for texture in [real_depth, warped_depth, game] {
        barrier(list, texture, motion::READABLE, D3D12_RESOURCE_STATE_COMMON);
    }
    let (real, warped) = recorded.map_err(|why| skip(&format!("motion vectors: {why}"))).ok()?;
    let outputs = in_eye_order(eye, real, warped);
    let mut targets: [Option<ID3D12Resource>; 2] = [None, None];
    for e in 0..2 {
        // SAFETY: reads a descriptor.
        let out = unsafe { outputs[e].GetDesc() };
        let target = d3d12::fitting(&mut state.motion[e], device, out.Width, out.Height, DXGI_FORMAT_R16G16B16A16_FLOAT, D3D12_RESOURCE_FLAG_NONE)?;
        d3d12::copy_whole(list, &target, &outputs[e], motion::READABLE);
        targets[e] = Some(target);
    }
    let [Some(left), Some(right)] = targets else { return None };
    Some([left, right])
}

/// The motion passes' constants for a pair, by eye (left, right): the real eye's rebase and the
/// warped eye's carry. `views`: how each eye is seen in this pair; `reached`: how the frame the
/// game's vectors point at was seen, their object motion scaled to one present by `object_scale`;
/// `before`: how each eye was seen in the pair before, each stream's previous image; `size`: the
/// warped eye's output. None without a matrix between two views.
pub(super) fn motion_constants(
    views: &[Rendered; 2],
    reached: Rendered,
    object_scale: f32,
    real_eye: usize,
    before: &[Rendered; 2],
    mvec_scale: [f32; 2],
    size: [u32; 2],
) -> Option<[motion::Constants; 2]> {
    let (eye, other) = (real_eye.min(1), 1 - real_eye.min(1));
    let now = &views[eye];
    let game_to_prev = to_clip(now, &reached)?;
    let real = motion::Constants { game_to_prev, same_to_prev: to_clip(now, &before[eye])?, mvec_scale, object_scale, ..Default::default() };
    let warped = motion::Constants {
        game_to_prev,
        same_to_prev: to_clip(&views[other], &before[other])?,
        target_to_source: to_clip(&views[other], now)?,
        size,
        mvec_scale,
        object_scale,
        ..Default::default()
    };
    Some(in_eye_order(eye, real, warped))
}

/// From `from`'s clip space to `to`'s, for a point `from` sees (its normalised position and depth,
/// w 1), as the motion passes take it: row vectors, as Streamline's matrices. None when the cameras
/// make no reprojection.
pub fn to_clip(from: &Rendered, to: &Rendered) -> Option<Mat44> {
    let m = source_matrix(&to.view(1, 1), &from.projection, &from.camera)?;
    Some(std::array::from_fn(|i| m[(i % 4) * 4 + i / 4]))
}

/// After the HUD went over the back buffer: the HUD `layer` (in COMMON) over this present's other
/// images, eye `e` as `params[e]` says. AFW alone: the warped eye. Frame generation: both eyes of
/// the pair shown later and of the generated pair.
pub fn lay_hud(queue: &ID3D12CommandQueue, layer: &ID3D12Resource, params: [hud_layer::d3d12::Params; 2]) {
    let mut guard = STATE.locked();
    let Some(state) = guard.as_mut() else { return };
    let Some(prepared) = &state.prepared else { return };
    if !state.hud_fits {
        Counters::add(&COUNTERS.no_hud);
        log_first(1, format_args!("frame warping: the back buffer's format is not one the warp writes; the warped eye goes without the HUD"));
        return;
    }
    let Ok(Some(list)) = state.recorder.begin() else {
        Counters::add(&COUNTERS.no_hud);
        return;
    };
    let common = D3D12_RESOURCE_STATE_COMMON;
    let mut frames: Vec<(ID3D12Resource, usize)> = Vec::new();
    match &prepared.shown {
        Some(shown) => frames.extend([(shown[0].clone(), 0), (shown[1].clone(), 1)]),
        None => frames.push((prepared.pair.images[1 - prepared.eye].clone(), 1 - prepared.eye)),
    }
    if prepared.generated.is_some() {
        frames.extend((0..2).filter_map(|e| Some((state.streams[e].as_ref()?.output.clone(), e))));
    }
    for (frame, e) in &frames {
        hud_layer::d3d12::lay_on(queue, &list, frame, common, layer, &params[*e]);
    }
    if let Err(e) = state.recorder.submit(queue) {
        log_first(3, format_args!("frame warping: HUD not laid over: {e}"));
    }
}
