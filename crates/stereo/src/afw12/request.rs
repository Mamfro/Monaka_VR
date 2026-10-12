//! At the present request, before the HUD goes over the frame: the back buffer warped into the
//! other eye ([`warp`]), frame generation's frame halfway between this pair and the one before
//! generated per eye ([`generate`], its motion carried onto each eye by the eyes' [`parallax`]),
//! and the HUD laid over the images made of the frame ([`lay_hud`]).

use super::*;


/// The real eye's inputs for its present.
pub(super) struct Inputs {
    pub(super) depth: ID3D12Resource,
    pub(super) motion: Option<ID3D12Resource>,
    pub(super) rendered: Rendered,
    pub(super) camera: Camera,
    /// The frame's view to clip (row vectors) and motion-vector scale (to the screen).
    pub(super) view_to_clip: Mat44,
    pub(super) mvec_scale: [f32; 2],
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
        view_to_clip: rendering.view_to_clip,
        mvec_scale: [camera.motion_vector_scale.x, camera.motion_vector_scale.y],
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
    pub(super) fix: Option<Result<MotionFix, ()>>,
    pub(super) motion: Option<ID3D12Resource>,
    pub(super) shown: [[Option<ID3D12Resource>; 2]; SLOTS],
    pub(super) shown_slot: usize,
    /// The last pair's pose, real eye and present (frame generation's history).
    pub(super) previous: Option<(HeadPose, usize, u64)>,
    pub(super) interval: Interval,
    /// When a labelled frame was last presented.
    pub(super) last_labelled: Option<Instant>,
    /// Each eye's last frame as presented, its pose and present, for a plain pair when a frame
    /// could not be warped.
    pub(super) plain: [Option<(ID3D12Resource, HeadPose, u64)>; 2],
}

// SAFETY: the D3D objects are free-threaded; everything is used under the state's lock only.
unsafe impl Send for State {}

pub(super) static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The state, made on first use on the game queue's device.
pub(super) fn state_for<'a>(guard: &'a mut Option<State>, queue: &ID3D12CommandQueue) -> Option<&'a mut State> {
    if guard.is_none() {
        let device = d3d12::device_of(queue).ok()?;
        // Frame generation reads the previous pair's images at the next present: a ring of them.
        let made = Afw12::new(&device, SLOTS).and_then(|afw| Ok((afw, Recorder::new(&device, 6).map_err(|e| e.to_string())?)));
        match made {
            Ok((afw, recorder)) => {
                *guard = Some(State {
                    afw,
                    recorder,
                    prepared: None,
                    hud_fits: false,
                    streams: [None, None],
                    fix: None,
                    motion: None,
                    shown: Default::default(),
                    shown_slot: 0,
                    previous: None,
                    interval: Interval::new(Duration::from_millis(16)),
                    last_labelled: None,
                    plain: [None, None],
                })
            }
            Err(why) => {
                log_first!(1, "frame warping unavailable: {why}");
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
        COUNTERS.no_inputs.fetch_add(1, Relaxed);
        return;
    };
    let Some(separation) = (game.separation)(&label.pose) else { return };
    let frustum = (game.frustum)(&label.pose, other);
    // SAFETY: reads a descriptor.
    let format = unsafe { buffer.GetDesc() }.Format;
    state.hud_fits = keeps_format(format);
    let real = RealEye {
        eye,
        color: buffer,
        color_state: D3D12_RESOURCE_STATE_PRESENT,
        depth: &inputs.depth,
        depth_state: D3D12_RESOURCE_STATE_COMMON,
        rendered: inputs.rendered,
        other: inputs.rendered.other_eye(eye, separation, frustum),
    };
    let pair = match state.afw.warp(queue, &real) {
        Ok(Some(pair)) => pair,
        Ok(None) => return,
        Err(why) => {
            log_first!(3, "frame warping: a frame not warped: {why}");
            return;
        }
    };
    let mut prepared = Prepared { present, eye, pose: label.pose, pair, shown: None, generated: None };
    if framegen_running() {
        generate(state, queue, &mut prepared, &inputs, separation);
    }
    state.prepared = Some(prepared);
}

/// Frame generation: copies of this pair to show later, and each eye's frame halfway between the
/// last pair and this one.
pub(super) fn generate(state: &mut State, queue: &ID3D12CommandQueue, prepared: &mut Prepared, inputs: &Inputs, separation: f32) {
    let Ok(device) = d3d12::device_of(queue) else { return };
    let (eye, pair) = (prepared.eye, &prepared.pair);
    // The history runs on when the last pair was the other eye's frame of the present before, the
    // game kept its own history and nothing was forgotten (the warped eye's gaps came from its own
    // last frame). It is this pair's once both streams were given it.
    let previous = state.previous.take();
    let continuous = previous.is_some_and(|(_, e, at)| e != eye && at + 1 == prepared.present) && !inputs.reset && pair.filled;
    // SAFETY: reads descriptors.
    let (frame, depth) = unsafe { (pair.images[0].GetDesc(), pair.depths[0].GetDesc()) };
    let render = Size2 { width: depth.Width as u32, height: depth.Height };
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
    let motion = fixed_motion(state, &list, &device, eye, &pair.depths[eye], inputs, separation);
    let made = motion.map(|motion| {
        [0, 1].map(|e| {
            let camera = eye_camera(&inputs.camera, eye, e, separation).for_render(render);
            let stream = match fitting_stream(&mut state.streams[e], &state.recorder, |s| s.fits(&frame, render), || EyeStream::new(&device, &frame, render)) {
                Ok(stream) => stream,
                Err(why) => return skip(&format!("no context: {why}")),
            };
            match stream.generate(&list, &camera, &pair.depths[e], &motion, None, &pair.images[e], !continuous, 16.7) {
                Ok(made) => made,
                Err(why) => skip(&why),
            }
        })
    });
    if let Err(e) = state.recorder.submit(queue) {
        log_first!(3, "frame warping: in-between frames not submitted: {e}");
        return;
    }
    if made.is_some() {
        state.previous = Some((prepared.pose, eye, prepared.present));
    }
    if made == Some([true, true])
        && let Some((before, ..)) = previous
        && prepared.shown.is_some()
    {
        prepared.generated = Some(monaka_framegen::blend(&before, &prepared.pose, 0.5));
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

/// The real eye's motion vectors (the game's: toward the other eye's frame before) carried onto
/// each eye's previous image ([`parallax`]), recorded on `list` into a texture of ours in COMMON.
pub(super) fn fixed_motion(
    state: &mut State,
    list: &ID3D12GraphicsCommandList,
    device: &ID3D12Device,
    eye: usize,
    depth: &ID3D12Resource,
    inputs: &Inputs,
    separation: f32,
) -> Option<ID3D12Resource> {
    let game = inputs.motion.as_ref()?;
    let fix = state.fix.get_or_insert_with(|| MotionFix::new(device).map_err(|why| log!("frame warping: no in-between frames: motion vectors: {why}"))).as_mut().ok()?;
    let constants = fix_constants(&inputs.view_to_clip, inputs.mvec_scale, eye, separation)?;
    barrier(list, depth, D3D12_RESOURCE_STATE_COMMON, motion::READABLE);
    barrier(list, game, D3D12_RESOURCE_STATE_COMMON, motion::READABLE);
    let recorded = fix.record(list, depth, game, &constants);
    barrier(list, game, motion::READABLE, D3D12_RESOURCE_STATE_COMMON);
    barrier(list, depth, motion::READABLE, D3D12_RESOURCE_STATE_COMMON);
    // SAFETY: reads a descriptor.
    let desc = unsafe { game.GetDesc() };
    let output = recorded.and_then(|()| fix.output(desc.Width as u32, desc.Height)).map_err(|why| skip(&format!("motion vectors: {why}"))).ok()?;
    let target = d3d12::fitting(&mut state.motion, device, desc.Width, desc.Height, DXGI_FORMAT_R16G16B16A16_FLOAT, D3D12_RESOURCE_FLAG_NONE)?;
    d3d12::copy_whole(list, &target, &output, motion::READABLE);
    Some(target)
}

/// The motion-vector fix's constants for eye `eye`'s frame: the game's vectors kept (its "previous"
/// camera the identity), each pixel moved on by the eyes' [`parallax`].
pub(super) fn fix_constants(view_to_clip: &Mat44, mvec_scale: [f32; 2], eye: usize, separation: f32) -> Option<motion::Constants> {
    let identity: Mat44 = std::array::from_fn(|i| if i % 5 == 0 { 1.0 } else { 0.0 });
    Some(motion::Constants { game_to_prev: identity, same_to_prev: parallax(view_to_clip, eye, separation)?, size: [0; 2], mvec_scale, object_scale: 1.0, pad: [0.0; 3] })
}

/// For the motion-vector fix (`motion_fix.hlsl`'s "same eye's previous" camera, row vectors as
/// DLSS's): from eye `eye`'s clip space to that of a view `separation` further from the other
/// eye. A point's move between the two is its parallax between the eyes (it is linear in the
/// camera's move), from the other eye's image to this eye's: what carries the game's vectors,
/// which end in the other eye's frame, onto this eye's previous image, warped from that frame.
pub fn parallax(view_to_clip: &Mat44, eye: usize, separation: f32) -> Option<Mat44> {
    // The view moved away from the other eye: left for the left eye.
    let away = if eye == 0 { -separation } else { separation };
    let mut translate: Mat44 = std::array::from_fn(|i| if i % 5 == 0 { 1.0 } else { 0.0 });
    translate[12] = -away;
    Some(multiply(&multiply(&invert(view_to_clip)?, &translate), view_to_clip))
}

/// After the HUD went over the back buffer: the HUD `layer` (in COMMON) over this present's other
/// images, eye `e` as `params[e]` says. AFW alone: the warped eye. Frame generation: both eyes of
/// the pair shown later and of the generated pair.
pub fn lay_hud(queue: &ID3D12CommandQueue, layer: &ID3D12Resource, params: [hudover12::Params; 2]) {
    let mut guard = STATE.locked();
    let Some(state) = guard.as_mut() else { return };
    let Some(prepared) = &state.prepared else { return };
    if !state.hud_fits {
        COUNTERS.no_hud.fetch_add(1, Relaxed);
        log_first!(1, "frame warping: the back buffer's format is not one the warp writes; the warped eye goes without the HUD");
        return;
    }
    let Ok(Some(list)) = state.recorder.begin() else {
        COUNTERS.no_hud.fetch_add(1, Relaxed);
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
        hudover12::lay_on(queue, &list, frame, common, layer, &params[*e]);
    }
    if let Err(e) = state.recorder.submit(queue) {
        log_first!(3, "frame warping: HUD not laid over: {e}");
    }
}
