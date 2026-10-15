//! Each eye's frame inputs from the calls a game makes for its upscaler, for the work done per eye
//! on them (frame generation, frame warping, depth to the headset): copies of the frame's depth
//! (plane 0 as R32, the depth texture's size, with the part that lines up with the image: [`rect`])
//! and motion vectors, how it was rendered (its camera and view to clip: [`Rendering`],
//! [`EyeInputs::camera`]), whether its history starts over, and which of them came since the eye's
//! last present ([`Fresh`]). One store for the run; each eye's present takes its inputs
//! ([`take`]). Also the size each frame renders at and is upscaled to ([`render_sizes`]).
//!
//! Collected on demand: each consumer asks at its install ([`want`]), and collecting starts at the
//! provider's arm ([`arm`]) when any did, whatever order the components start in.
//!
//! The sources, whichever the game's upscaler gives; the consumers do not tell them apart:
//! - Streamline's calls for DLSS, by when the frame's eye is known (the provider asks the game's
//!   labels): at the frame's constants ([`at_constants`] with the eye, then [`at_tags`], then at
//!   DLSS's evaluate [`at_evaluate`] copies everything on the game's native command list, just
//!   before DLSS); or at each call ([`set_rendering`] and [`tagged`], copied on the tag's list when
//!   tagged, or, `late`: buffers complete only by the present, kept and copied at the eye's present,
//!   [`take_late`]);
//! - the game's FSR upscale dispatch ([`upscaled`], with [`Rendering::from_dispatch`]): copied on
//!   the game's command list just before FSR, as at DLSS's evaluate.
//!
//! The inputs are always the game's own, as it made them for its upscaler: a per-eye upscaler's
//! rewrite of the motion vectors and constants (`monaka_streamline::per_eye`,
//! `crate::upscale_per_eye`) comes after the provider in each call and is never seen here. What
//! the game's motion vectors point at depends on how the game was made to render: its own previous
//! frame (with alternate eyes the other eye's) or, when a producer hands each eye's render its own
//! previous camera, that eye's own previous frame. The producer says which ([`set_previous`]) and
//! each frame's inputs carry it ([`EyeInputs::previous`]), so a consumer rebases or chains only
//! what needs it.

use monaka_ffx::{Camera, Float2};
use crate::motion::{self, MotionFix};
use monaka_ffx::upscale::{GameDispatch, create};
use monaka_channel::d3d12;
use monaka_core::camera::Frustum;
use monaka_core::sync::Locked;
use monaka_producer::log;
use monaka_streamline::{Constants, RESOURCE_TEX2D, Tag, buffer, constants as at};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

/// How the motion vectors are taken; of several asks, the later in this list wins. At the tags
/// ([`tagged`]) they are copied in their own format whenever asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Motion {
    /// Not taken.
    None,
    /// Copied whole into the float member of their family ([`float_format`]); not taken in another
    /// format.
    Float,
    /// Onto this eye's own previous frame: the game's (toward the frame before, the other eye's)
    /// chained over the frame before's own (`motion_compose.hlsl`), into R16G16B16A16 float; a frame
    /// whose chain does not hold ([`chain_holds`]) starts its history over. A frame whose vectors
    /// already reach this eye's own previous frame ([`PreviousFrame::OwnEye`]) is copied as `Float`.
    Chained,
}

/// What a frame's motion vectors point at: the frame the game treated as its previous one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviousFrame {
    /// The game's own previous frame, one frame back: the frame presented before, whichever eye
    /// rendered it (the other eye's when the eyes take turns).
    #[default]
    OtherEye,
    /// This eye's own previous frame, `frames_back` game frames back (two with alternate eyes, one
    /// with both eyes in a frame): a producer handed the render that eye's own previous camera.
    OwnEye { frames_back: u8 },
}

impl PreviousFrame {
    /// How many frames back the vectors reach.
    pub fn frames_back(self) -> u8 {
        match self {
            PreviousFrame::OtherEye => 1,
            PreviousFrame::OwnEye { frames_back } => frames_back.max(1),
        }
    }

    fn code(self) -> u8 {
        match self {
            PreviousFrame::OtherEye => 0,
            PreviousFrame::OwnEye { frames_back } => frames_back.max(1),
        }
    }

    fn from_code(code: u8) -> Self {
        if code == 0 { PreviousFrame::OtherEye } else { PreviousFrame::OwnEye { frames_back: code } }
    }
}

/// What the frames being rendered now have as their previous frame ([`set_previous`]).
static PREVIOUS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The producer says, before a frame's upscaler calls, what that frame's motion vectors point at
/// (it stays until said again; the game's own previous frame unless said).
pub fn set_previous(previous: PreviousFrame) {
    PREVIOUS.store(previous.code(), Relaxed);
}

/// What the frame being rendered has as its previous frame.
pub fn previous() -> PreviousFrame {
    PreviousFrame::from_code(PREVIOUS.load(Relaxed))
}

/// Where a frame's camera was in the world: its position and axes (view space looks down
/// `forward`), for a source whose call does not carry them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Place {
    pub position: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
}

/// How a frame was rendered, whichever of the game's calls told it: its camera (its place, jitter,
/// the motion vectors' scale to the screen, near and far planes and vertical field of view) and its
/// view to clip (row vectors, as DirectX games and Streamline give it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rendering {
    pub camera: Camera,
    pub view_to_clip: [f32; 16],
}

impl Rendering {
    /// From the DLSS constants the game gave Streamline.
    pub fn from_constants(constants: &Constants) -> Self {
        Self { camera: crate::camera(constants), view_to_clip: constants.matrix(at::CAMERA_VIEW_TO_CLIP) }
    }

    /// From the game's FSR upscale dispatch, which gives the jitter, the motion vectors' scale (to
    /// render pixels), the near and far planes, the vertical field of view and the render size, with
    /// what it does not: where the camera was (`place`), the frustum the frame was drawn with, and
    /// the depth convention of the upscale context (`flags`, `monaka_ffx::upscale::create`). `None` with
    /// no render size or near plane.
    pub fn from_dispatch(dispatch: &GameDispatch, place: Place, frustum: &Frustum, flags: u32) -> Option<Self> {
        let size = dispatch.render_size;
        let (near, far) = (dispatch.near, dispatch.far);
        if size.width == 0 || size.height == 0 || !(near.is_finite() && near > 0.0) {
            return None;
        }
        let inverted = flags & create::DEPTH_INVERTED != 0;
        let infinite = flags & create::DEPTH_INFINITE != 0 || !far.is_finite() || far <= near;
        let projection = perspective(frustum, near, far, inverted, infinite);
        let scale = dispatch.motion_vector_scale;
        let camera = Camera {
            render_size: Default::default(),
            jitter: dispatch.jitter,
            motion_vector_scale: Float2 { x: scale.x / size.width as f32, y: scale.y / size.height as f32 },
            near,
            far,
            fov_vertical: dispatch.fov_vertical,
            position: place.position,
            up: place.up,
            right: place.right,
            forward: place.forward,
        };
        Some(Self { camera, view_to_clip: std::array::from_fn(|i| projection[(i % 4) * 4 + i / 4]) })
    }
}

/// A canonical perspective projection (row-major, right-handed, clip w = -z) for `frustum` with
/// these planes and depth convention (reversed: 1 at the near plane; infinite: no far plane).
fn perspective(frustum: &Frustum, near: f32, far: f32, inverted: bool, infinite: bool) -> [f32; 16] {
    // Depth d = -p10 + p11 / distance.
    let (p10, p11) = match (inverted, infinite) {
        (true, true) => (0.0, near),
        (false, true) => (-1.0, -near),
        (true, false) => (near / (far - near), near * far / (far - near)),
        (false, false) => (-far / (far - near), -near * far / (far - near)),
    };
    let mut projection = [0.0; 16];
    frustum.apply_to_projection(&mut projection);
    (projection[10], projection[11], projection[14]) = (p10, p11, -1.0);
    projection
}

/// What the consumers asked for this run.
struct Demand {
    wanted: bool,
    motion: Motion,
    accepts: Vec<fn(&Rendering) -> bool>,
}

static DEMAND: Mutex<Demand> = Mutex::new(Demand { wanted: false, motion: Motion::None, accepts: Vec::new() });
static COLLECTING: AtomicBool = AtomicBool::new(false);

/// A consumer's ask, at its install: the inputs collected this run, with the motion vectors taken
/// as `motion` at least, and only of frames `accept` takes (every consumer's test must pass; a
/// frame refused leaves the eye's inputs as they were).
pub fn want(motion: Motion, accept: Option<fn(&Rendering) -> bool>) {
    let mut demand = DEMAND.locked();
    demand.wanted = true;
    demand.motion = demand.motion.max(motion);
    demand.accepts.extend(accept);
}

/// The provider's arm, once every component is installed: collecting starts if any consumer asked.
pub fn arm() {
    COLLECTING.store(DEMAND.locked().wanted, Release);
}

/// The provider's stop: collecting ends and the asks are dropped (the next run asks again).
pub fn disarm() {
    COLLECTING.store(false, Release);
    *DEMAND.locked() = Demand { wanted: false, motion: Motion::None, accepts: Vec::new() };
}

/// Whether the inputs are collected now.
pub fn collecting() -> bool {
    COLLECTING.load(Acquire)
}

/// How the consumers asked for the motion vectors.
pub fn motion_wanted() -> Motion {
    DEMAND.locked().motion
}

/// Which inputs came since the eye's last present.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fresh {
    pub depth: bool,
    pub motion: bool,
}

impl Fresh {
    pub fn both(self) -> bool {
        self.depth && self.motion
    }
}

/// One eye's inputs: our copies (in COMMON, written again by the eye's next frame) and what came
/// with them.
#[derive(Clone, Default)]
pub struct EyeInputs {
    /// The depth: plane 0 as R32 (typeless), the depth texture's size.
    pub depth: Option<ID3D12Resource>,
    /// The motion vectors, in `motion_format`.
    pub motion: Option<ID3D12Resource>,
    pub motion_format: DXGI_FORMAT,
    /// How the frame was rendered.
    pub rendering: Option<Rendering>,
    /// The frame starts a history over: the game reset its own, or the motion vectors' chain broke.
    pub reset: bool,
    /// What the motion vectors point at, as the frame was rendered (chained vectors reach this eye's
    /// own previous frame whatever this says).
    pub previous: PreviousFrame,
    /// The part of the depth that lines up with the image ([`rect`]).
    pub rect: Option<[u32; 4]>,
    pub fresh: Fresh,
}

impl EyeInputs {
    /// The camera the frame was rendered with.
    pub fn camera(&self) -> Option<Camera> {
        self.rendering.map(|r| r.camera)
    }
}

#[derive(Default)]
struct Eye {
    inputs: EyeInputs,
    /// Buffers complete only by the eye's present, kept to be copied there.
    late_depth: Option<(ID3D12Resource, D3D12_RESOURCE_STATES)>,
    late_motion: Option<(ID3D12Resource, D3D12_RESOURCE_STATES)>,
}

static EYES: Mutex<[Option<Eye>; 2]> = Mutex::new([None, None]);
static COPIES: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static CAMERAS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// The frame being rendered, labelled at its Streamline constants: what its calls said so far; and
/// the motion vectors' chain over the frames, whichever source gave them.
struct Staged {
    eye: Option<usize>,
    rendering: Option<Rendering>,
    reset: bool,
    depth: Option<(usize, u32)>,
    motion: Option<(usize, u32)>,
    /// The size the frame is rendered at (DLSS's input colour).
    render: Option<(u32, u32)>,
    /// The depth textures the frame's tags named, in order (some games name two: [`settled_depth`]).
    depths: [Option<(usize, u32)>; 2],
    /// The depth texture the frames have settled on, and one newly named last, waiting for a
    /// second frame ([`settled_depth`]).
    settled: Option<usize>,
    newcomer: Option<usize>,
    /// The game's motion vectors of the frame before (a copy), and its eye.
    previous_motion: Option<ID3D12Resource>,
    previous_eye: Option<usize>,
    composer: Option<MotionFix>,
    broken: bool,
}

static STAGED: Mutex<Staged> = Mutex::new(Staged {
    eye: None,
    rendering: None,
    reset: false,
    depth: None,
    motion: None,
    render: None,
    depths: [None; 2],
    settled: None,
    newcomer: None,
    previous_motion: None,
    previous_eye: None,
    composer: None,
    broken: false,
});

// --- At the upscaler's call --------------------------------------------------------------------------

/// A frame at its upscaler's call, its depth and motion vectors complete: what the inputs take of it.
pub struct Upscaled<'a> {
    pub eye: usize,
    pub rendering: Rendering,
    /// The game starts its upscaler's history over with this frame.
    pub reset: bool,
    /// The game's depth and motion vectors, each in its state.
    pub depth: (&'a ID3D12Resource, D3D12_RESOURCE_STATES),
    pub motion: (&'a ID3D12Resource, D3D12_RESOURCE_STATES),
    /// The size the frame is rendered at (its upscaler's input).
    pub render: Option<(u32, u32)>,
    /// What turns the stored motion vectors into render pixels, for their chain.
    pub chain_scale: [f32; 2],
}

/// The game's upscaler call for a frame (its FSR dispatch): the frame's inputs copied for its eye
/// on `list` (the game's, before the upscaler), as the consumers asked. `None`, a frame of no eye
/// or without its inputs, breaks the motion vectors' chain.
pub fn upscaled(list: &ID3D12GraphicsCommandList, frame: Option<Upscaled>) {
    let mut s = STAGED.locked();
    match frame.filter(|f| f.eye < 2) {
        Some(frame) => take_frame(&mut s, list, &frame),
        None => s.previous_eye = None,
    }
}

/// The frame's inputs copied for its eye on `list`, as the consumers asked; a frame any consumer
/// refuses leaves the eye's inputs as they were.
fn take_frame(s: &mut Staged, list: &ID3D12GraphicsCommandList, frame: &Upscaled) {
    let wanted = {
        let demand = DEMAND.locked();
        demand.accepts.iter().all(|accept| accept(&frame.rendering)).then_some(demand.motion)
    };
    let Some(wanted) = wanted else { return };
    match (wanted, previous()) {
        (Motion::Chained, PreviousFrame::OtherEye) => chained(s, list, frame),
        // The game's vectors already reach this eye's own previous frame: the chain is not needed,
        // and does not run across such a frame.
        (Motion::Chained, PreviousFrame::OwnEye { .. }) => {
            s.previous_eye = None;
            copied(list, frame, Motion::Float);
        }
        (Motion::None | Motion::Float, _) => copied(list, frame, wanted),
    }
}

// --- Which depth, when a frame's tags name two -------------------------------------------------------

/// Frames whose last named depth was passed over for the settled one ([`settled_depth`]).
static PASSED_OVER: AtomicU64 = AtomicU64::new(0);

/// The depth a frame gives, from `last` (the texture its tags named last, and its state). Some
/// games (The Beast) name two depth textures every frame, the current one last, and swap their
/// roles now and then; but for a single frame now and then the one named last is the other,
/// still holding the frame before (with alternate eyes, the other eye's depth: a warp from it
/// leaves near things where the other eye saw them). So while a frame names two, a newly last one
/// is taken from its second frame in a row, the settled one until then. A frame naming one depth
/// gives it.
fn settled_depth(s: &mut Staged, last: (usize, u32)) -> (usize, u32) {
    let named: Vec<usize> = s.depths.iter().flatten().map(|&(d, _)| d).collect();
    let (chosen, settled, newcomer) = settle(s.settled, s.newcomer, &named, last.0);
    (s.settled, s.newcomer) = (settled, newcomer);
    if chosen == last.0 {
        return last;
    }
    PASSED_OVER.fetch_add(1, Relaxed);
    s.depths.iter().flatten().copied().find(|&(d, _)| d == chosen).unwrap_or(last)
}

/// [`settled_depth`]'s rule: of the depth textures a frame `named`, `last` the one named last, the
/// texture to take, and the settled one and newcomer after it.
fn settle(settled: Option<usize>, newcomer: Option<usize>, named: &[usize], last: usize) -> (usize, Option<usize>, Option<usize>) {
    match settled {
        Some(kept) if named.len() == 2 && named.contains(&kept) && kept != last && newcomer != Some(last) => (kept, Some(kept), Some(last)),
        _ => (last, Some(last), None),
    }
}

/// Frames whose last named depth was passed over for the settled one (a game naming two).
pub fn depths_passed_over() -> u64 {
    PASSED_OVER.load(Relaxed)
}

// --- Labelled at the Streamline constants ----------------------------------------------------------

/// `slSetConstants` for a frame labelled `eye` (`None`: not an eye's, not taken).
pub fn at_constants(constants: *const u8, eye: Option<usize>) {
    if constants.is_null() {
        return;
    }
    let mut s = STAGED.locked();
    (s.eye, s.rendering, s.depth, s.motion, s.render) = (None, None, None, None, None);
    s.depths = [None; 2];
    let Some(eye) = eye.filter(|&e| e < 2) else { return };
    let Some(read) = Constants::read(constants) else { return };
    (s.rendering, s.reset) = (Some(Rendering::from_constants(&read)), read.reset());
    s.eye = Some(eye);
}

/// `slSetTagForFrame` for the labelled frame: its depth and motion vectors (native textures and
/// their states) and the size it is rendered at.
pub fn at_tags(tags: &[Tag]) {
    let mut s = STAGED.locked();
    if s.eye.is_none() {
        return;
    }
    for tag in tags.iter().filter(|t| t.resource_type == RESOURCE_TEX2D) {
        match tag.kind {
            buffer::DEPTH => {
                s.depth = Some((tag.native, tag.state));
                if !s.depths.iter().flatten().any(|&(d, _)| d == tag.native) {
                    match s.depths.iter_mut().find(|d| d.is_none()) {
                        Some(free) => *free = Some((tag.native, tag.state)),
                        None => s.depths = [s.depths[1], Some((tag.native, tag.state))],
                    }
                }
            }
            buffer::MOTION_VECTORS => s.motion = Some((tag.native, tag.state)),
            buffer::SCALING_INPUT_COLOR => s.render = tag.size(),
            _ => {}
        }
    }
}

/// `slEvaluateFeature` for DLSS: the labelled frame's depth and motion vectors are complete. On the
/// game's native command list, before DLSS: its inputs copied for its eye, as the consumers asked.
pub fn at_evaluate(native_list: Option<ID3D12GraphicsCommandList>) {
    let mut s = STAGED.locked();
    let (Some(eye), Some(rendering), Some((depth, depth_state)), Some((motion, motion_state))) = (s.eye, s.rendering, s.depth, s.motion) else {
        // A frame without its inputs breaks the chain.
        s.previous_eye = None;
        return;
    };
    s.eye = None;
    let (depth, depth_state) = settled_depth(&mut s, (depth, depth_state));
    let Some(list) = native_list else {
        s.previous_eye = None;
        return;
    };
    let (raw_depth, raw_motion) = (depth as *mut c_void, motion as *mut c_void);
    // SAFETY: the game's tagged textures, live while it records this frame.
    let (Some(depth), Some(motion)) = (unsafe { ID3D12Resource::from_raw_borrowed(&raw_depth) }, unsafe { ID3D12Resource::from_raw_borrowed(&raw_motion) }) else { return };
    let state = |s: u32| D3D12_RESOURCE_STATES(s as i32);
    // The game's tagged vectors are in render pixels.
    let frame = Upscaled { eye, rendering, reset: s.reset, depth: (depth, state(depth_state)), motion: (motion, state(motion_state)), render: s.render, chain_scale: [1.0, 1.0] };
    take_frame(&mut s, &list, &frame);
}

/// The frame's depth and (`Float`) motion vectors copied, the motion vectors as the game made them.
fn copied(list: &ID3D12GraphicsCommandList, frame: &Upscaled, motion: Motion) {
    let Ok(device) = d3d12::device_of(frame.depth.0) else { return };
    // SAFETY: reads descriptors.
    let (depth_desc, motion_desc) = unsafe { (frame.depth.0.GetDesc(), frame.motion.0.GetDesc()) };
    let mut eyes = EYES.locked();
    let inputs = &mut eyes[frame.eye].get_or_insert_with(Eye::default).inputs;
    let none = D3D12_RESOURCE_FLAG_NONE;
    let Some(depth_copy) = d3d12::fitting(&mut inputs.depth, &device, depth_desc.Width, depth_desc.Height, DXGI_FORMAT_R32_TYPELESS, none) else { return };
    d3d12::copy_plane0(list, &depth_copy, frame.depth.0, frame.depth.1);
    let format = float_format(motion_desc.Format).filter(|_| motion == Motion::Float);
    let motion_copy = format.and_then(|format| d3d12::fitting(&mut inputs.motion, &device, motion_desc.Width, motion_desc.Height, format, none));
    match &motion_copy {
        Some(copy) => d3d12::copy_whole(list, copy, frame.motion.0, frame.motion.1),
        None => inputs.motion = None,
    }
    inputs.motion_format = format.unwrap_or(DXGI_FORMAT_UNKNOWN);
    inputs.rendering = Some(frame.rendering);
    inputs.reset = frame.reset;
    inputs.previous = previous();
    inputs.rect = rect((depth_desc.Width as u32, depth_desc.Height), frame.render);
    inputs.fresh = Fresh { depth: true, motion: motion_copy.is_some() };
    COPIES[frame.eye].fetch_add(1, Relaxed);
}

/// The frame's depth copied and its motion vectors chained onto this eye's previous frame; the
/// game's own kept for the next frame's chain.
fn chained(s: &mut Staged, list: &ID3D12GraphicsCommandList, frame: &Upscaled) {
    if s.broken {
        return;
    }
    let ((depth, depth_state), (motion, motion_state)) = (frame.depth, frame.motion);
    let Ok(device) = d3d12::device_of(depth) else { return };
    if s.composer.is_none() {
        match MotionFix::composer(&device) {
            Ok(composer) => s.composer = Some(composer),
            Err(why) => {
                log!("frame generation off: per-eye motion vectors: {why}");
                s.broken = true;
                return;
            }
        }
    }
    // SAFETY: reads descriptors.
    let (depth_desc, motion_desc) = unsafe { (depth.GetDesc(), motion.GetDesc()) };
    let previous = s.previous_motion.clone().filter(|p| {
        // SAFETY: reads a descriptor.
        let d = unsafe { p.GetDesc() };
        chain_holds(s.previous_eye, frame.eye, (d.Width, d.Height), (motion_desc.Width, motion_desc.Height))
    });
    let mut eyes = EYES.locked();
    let inputs = &mut eyes[frame.eye].get_or_insert_with(Eye::default).inputs;
    let none = D3D12_RESOURCE_FLAG_NONE;
    let Some(depth_copy) = d3d12::fitting(&mut inputs.depth, &device, depth_desc.Width, depth_desc.Height, DXGI_FORMAT_R32_TYPELESS, none) else { return };
    let Some(motion_copy) = d3d12::fitting(&mut inputs.motion, &device, motion_desc.Width, motion_desc.Height, DXGI_FORMAT_R16G16B16A16_FLOAT, none) else { return };
    d3d12::copy_plane0(list, &depth_copy, depth, depth_state);
    let chained = match (&previous, s.composer.as_mut()) {
        (Some(previous), Some(composer)) => {
            d3d12::barrier(list, previous, D3D12_RESOURCE_STATE_COMMON, motion::READABLE);
            let recorded = composer.record(list, &[previous, motion], &motion::Constants::chaining(frame.chain_scale));
            d3d12::barrier(list, previous, motion::READABLE, D3D12_RESOURCE_STATE_COMMON);
            match recorded.and_then(|()| composer.output(motion_desc.Width as u32, motion_desc.Height)) {
                Ok(output) => {
                    d3d12::copy_whole(list, &motion_copy, &output, motion::READABLE);
                    true
                }
                Err(why) => {
                    log!("frame generation off: per-eye motion vectors: {why}");
                    s.broken = true;
                    false
                }
            }
        }
        _ => false,
    };
    // This frame's vectors, for the next frame's chain.
    if let Some(keep) = d3d12::fitting(&mut s.previous_motion, &device, motion_desc.Width, motion_desc.Height, motion_desc.Format, none) {
        d3d12::copy_whole(list, &keep, motion, motion_state);
        s.previous_eye = Some(frame.eye);
    }
    inputs.motion_format = DXGI_FORMAT_R16G16B16A16_FLOAT;
    inputs.rendering = Some(frame.rendering);
    inputs.reset = frame.reset || !chained;
    inputs.previous = PreviousFrame::OtherEye;
    inputs.rect = rect((depth_desc.Width as u32, depth_desc.Height), frame.render);
    inputs.fresh = Fresh { depth: true, motion: true };
    COPIES[frame.eye].fetch_add(1, Relaxed);
}

/// Eye `eye`'s motion vectors chain onto its own previous frame over the frame before when that
/// frame was the other eye's (`previous_eye`) and its kept vectors are the same size.
pub fn chain_holds(previous_eye: Option<usize>, eye: usize, previous_size: (u64, u32), size: (u64, u32)) -> bool {
    previous_eye == Some(1 - eye) && previous_size == size
}

// --- Known at each call ----------------------------------------------------------------------------

/// What the game tagged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Depth,
    MotionVectors,
}

/// How eye `eye` is rendered.
pub fn set_rendering(eye: usize, rendering: Rendering) {
    if let Some(slot) = EYES.locked().get_mut(eye) {
        slot.get_or_insert_with(Eye::default).inputs.rendering = Some(rendering);
        CAMERAS[eye].fetch_add(1, Relaxed);
    }
}

/// The game tagged `source` (in `state`) as eye `eye`'s `input`: copied on `list` (the motion
/// vectors in their own format), or (`late`: a buffer complete only by the eye's present) kept to
/// be copied there ([`take_late`]).
pub fn tagged(list: &ID3D12GraphicsCommandList, eye: usize, input: Input, source: &ID3D12Resource, state: D3D12_RESOURCE_STATES, late: bool) {
    let mut eyes = EYES.locked();
    let Some(slot) = eyes.get_mut(eye) else { return };
    let entry = slot.get_or_insert_with(Eye::default);
    // SAFETY: reads a descriptor.
    let desc = unsafe { source.GetDesc() };
    let none = D3D12_RESOURCE_FLAG_NONE;
    let inputs = &mut entry.inputs;
    inputs.previous = previous();
    match input {
        Input::Depth if late => {
            entry.late_depth = Some((source.clone(), state));
            inputs.fresh.depth = true;
        }
        Input::MotionVectors if late => {
            entry.late_motion = Some((source.clone(), state));
            inputs.motion_format = desc.Format;
            inputs.fresh.motion = true;
        }
        Input::Depth => {
            let Some(target) = d3d12::device_of(source).ok().and_then(|device| d3d12::fitting(&mut inputs.depth, &device, desc.Width, desc.Height, DXGI_FORMAT_R32_TYPELESS, none)) else { return };
            d3d12::copy_plane0(list, &target, source, state);
            inputs.fresh.depth = true;
        }
        Input::MotionVectors => {
            let Some(target) = d3d12::device_of(source).ok().and_then(|device| d3d12::fitting(&mut inputs.motion, &device, desc.Width, desc.Height, desc.Format, none)) else { return };
            d3d12::copy_whole(list, &target, source, state);
            inputs.motion_format = desc.Format;
            inputs.fresh.motion = true;
        }
    }
    COPIES[eye].fetch_add(1, Relaxed);
}

/// The buffers kept at their tags ([`tagged`] `late`), to be copied into the eye's own on the
/// present's list.
pub struct Late {
    depth: Option<(ID3D12Resource, D3D12_RESOURCE_STATES, ID3D12Resource)>,
    motion: Option<(ID3D12Resource, D3D12_RESOURCE_STATES, ID3D12Resource)>,
}

impl Late {
    /// Records the copies on `list`.
    pub fn record(&self, list: &ID3D12GraphicsCommandList) {
        if let Some((source, state, target)) = &self.depth {
            d3d12::copy_plane0(list, target, source, *state);
        }
        if let Some((source, state, target)) = &self.motion {
            d3d12::copy_whole(list, target, source, *state);
        }
    }
}

/// At eye `eye`'s present: its inputs (taken, as [`take`]) with the buffers kept late, whose copies
/// ([`Late::record`]) go on the present's list.
pub fn take_late(eye: usize) -> Option<(EyeInputs, Late)> {
    let mut eyes = EYES.locked();
    let entry = eyes.get_mut(eye)?.as_mut()?;
    let none = D3D12_RESOURCE_FLAG_NONE;
    let late_depth = entry.late_depth.take().and_then(|(source, state)| {
        // SAFETY: reads a descriptor.
        let d = unsafe { source.GetDesc() };
        let device = d3d12::device_of(&source).ok()?;
        d3d12::fitting(&mut entry.inputs.depth, &device, d.Width, d.Height, DXGI_FORMAT_R32_TYPELESS, none).map(|target| (source, state, target))
    });
    let late_motion = entry.late_motion.take().and_then(|(source, state)| {
        // SAFETY: reads a descriptor.
        let d = unsafe { source.GetDesc() };
        let device = d3d12::device_of(&source).ok()?;
        d3d12::fitting(&mut entry.inputs.motion, &device, d.Width, d.Height, d.Format, none).map(|target| (source, state, target))
    });
    let inputs = entry.inputs.clone();
    entry.inputs.fresh = Fresh::default();
    Some((inputs, Late { depth: late_depth, motion: late_motion }))
}

// --- At the present --------------------------------------------------------------------------------

/// Which of eye `eye`'s inputs came since its last present.
pub fn fresh(eye: usize) -> Fresh {
    EYES.locked().get(eye).and_then(|e| e.as_ref()).map(|e| e.inputs.fresh).unwrap_or_default()
}

/// Eye `eye`'s inputs as they are, left fresh.
pub fn peek(eye: usize) -> Option<EyeInputs> {
    Some(EYES.locked().get(eye)?.as_ref()?.inputs.clone())
}

/// Eye `eye`'s inputs at its present: as they are ([`EyeInputs::fresh`] says which came since its
/// last present), none fresh from here.
pub fn take(eye: usize) -> Option<EyeInputs> {
    let mut eyes = EYES.locked();
    let inputs = &mut eyes.get_mut(eye)?.as_mut()?.inputs;
    let taken = inputs.clone();
    inputs.fresh = Fresh::default();
    Some(taken)
}

/// Each eye's inputs copied (or kept late) so far.
pub fn copies() -> [u64; 2] {
    [COPIES[0].load(Relaxed), COPIES[1].load(Relaxed)]
}

/// Each eye's rendering given at its own call ([`set_rendering`]) so far.
pub fn cameras() -> [u64; 2] {
    [CAMERAS[0].load(Relaxed), CAMERAS[1].load(Relaxed)]
}

// --- The render size -------------------------------------------------------------------------------

/// The size frames render at and are upscaled to (width in the high half; 0 not known).
static SCENE_SIZE: AtomicU64 = AtomicU64::new(0);
static OUTPUT_SIZE: AtomicU64 = AtomicU64::new(0);

fn pack(size: Option<(u32, u32)>) -> u64 {
    size.map_or(0, |(w, h)| ((w as u64) << 32) | h as u64)
}

fn unpack(packed: u64) -> Option<(u32, u32)> {
    (packed != 0).then_some(((packed >> 32) as u32, packed as u32))
}

/// Every `slSetTagForFrame`: the size the frame renders at (DLSS's input colour, else its depth)
/// and the size it is upscaled to (its output), when the call tells the first.
pub fn sizes_tagged(tags: &[Tag]) {
    let scene = monaka_streamline::tagged_size(tags, buffer::SCALING_INPUT_COLOR).or_else(|| monaka_streamline::tagged_size(tags, buffer::DEPTH));
    if let Some(scene) = scene {
        sizes_known(scene, monaka_streamline::tagged_size(tags, buffer::SCALING_OUTPUT_COLOR));
    }
}

/// An upscaler's call told the size the frame renders at (`scene`) and is upscaled to (`output`).
pub fn sizes_known(scene: (u32, u32), output: Option<(u32, u32)>) {
    if scene.0 > 0 && scene.1 > 0 {
        SCENE_SIZE.store(pack(Some(scene)), Relaxed);
        OUTPUT_SIZE.store(pack(output), Relaxed);
    }
}

/// A texture's size: width, height.
pub type Size = (u32, u32);

/// The size the newest frame renders at, and the size it is upscaled to (`None` when not known).
pub fn render_sizes() -> Option<(Size, Option<Size>)> {
    Some((unpack(SCENE_SIZE.load(Relaxed))?, unpack(OUTPUT_SIZE.load(Relaxed))))
}

/// Everything dropped (at stop, once the GPU no longer reads it).
pub fn clear() {
    *EYES.locked() = [None, None];
    let mut s = STAGED.locked();
    (s.composer, s.previous_motion, s.previous_eye) = (None, None, None);
}

/// The float member of the motion vectors' family (the view they are read as): `None` for a
/// format not handled.
pub fn float_format(format: DXGI_FORMAT) -> Option<DXGI_FORMAT> {
    match format {
        DXGI_FORMAT_R16G16B16A16_TYPELESS | DXGI_FORMAT_R16G16B16A16_FLOAT => Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
        DXGI_FORMAT_R16G16_TYPELESS | DXGI_FORMAT_R16G16_FLOAT => Some(DXGI_FORMAT_R16G16_FLOAT),
        DXGI_FORMAT_R32G32_TYPELESS | DXGI_FORMAT_R32G32_FLOAT => Some(DXGI_FORMAT_R32G32_FLOAT),
        _ => None,
    }
}

/// The part of a depth texture of `depth` size that lines up with the image: the frame rendered at
/// `render` size (its upscaler's input colour) in its top-left corner. `None` (all of it) when the
/// sizes match or the render size is not known.
pub fn rect(depth: (u32, u32), render: Option<(u32, u32)>) -> Option<[u32; 4]> {
    let (width, height) = render.filter(|&(w, h)| w > 0 && h > 0)?;
    let (width, height) = (width.min(depth.0), height.min(depth.1));
    ((width, height) != depth).then_some([0, 0, width, height])
}

#[cfg(test)]
mod tests;
