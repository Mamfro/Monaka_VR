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
//! `crate::upscale_per_eye`) comes after the provider in each call and is never seen here.

use crate::ffx::{Camera, Float2};
use crate::motion::{self, MotionFix};
use crate::upscale::{GameDispatch, create};
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
    /// The game's (toward the frame before: the other eye's) chained over the frame before's own
    /// onto this eye's previous frame (`motion_compose.hlsl`), into R16G16B16A16 float. A frame
    /// whose chain does not hold ([`chain_holds`]) starts its history over.
    Chained,
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
    /// the depth convention of the upscale context (`flags`, `crate::upscale::create`). `None` with
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
    match wanted {
        Motion::Chained => chained(s, list, frame),
        Motion::None | Motion::Float => copied(list, frame, wanted),
    }
}

// --- Labelled at the Streamline constants ----------------------------------------------------------

/// `slSetConstants` for a frame labelled `eye` (`None`: not an eye's, not taken).
pub fn at_constants(constants: *const u8, eye: Option<usize>) {
    if constants.is_null() {
        return;
    }
    let mut s = STAGED.locked();
    (s.eye, s.rendering, s.depth, s.motion, s.render) = (None, None, None, None, None);
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
            buffer::DEPTH => s.depth = Some((tag.native, tag.state)),
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
            let recorded = composer.record(list, previous, motion, &motion::Constants::chaining(frame.chain_scale));
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
mod tests {
    use super::*;
    use crate::ffx::Size2;
    use crate::upscale::GameResource;
    use monaka_channel::d3d12::Recorder;
    use monaka_streamline::{TAG_BYTES, constants as at, read_tags};
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;

    #[test]
    fn the_rect_is_the_render_size_part() {
        assert_eq!(rect((1920, 1080), Some((1920, 1080))), None, "the whole texture");
        assert_eq!(rect((1920, 1080), None), None, "render size not known");
        assert_eq!(rect((2048, 2048), Some((1440, 1600))), Some([0, 0, 1440, 1600]), "a larger depth texture");
        assert_eq!(rect((1440, 1600), Some((2160, 2160))), None, "never past the texture");
        assert_eq!(rect((1440, 1600), Some((0, 1600))), None, "no size");
    }

    #[test]
    fn the_motion_vectors_are_read_as_their_float_member() {
        assert_eq!(float_format(DXGI_FORMAT_R16G16_TYPELESS), Some(DXGI_FORMAT_R16G16_FLOAT));
        assert_eq!(float_format(DXGI_FORMAT_R16G16B16A16_FLOAT), Some(DXGI_FORMAT_R16G16B16A16_FLOAT));
        assert_eq!(float_format(DXGI_FORMAT_R32G32_TYPELESS), Some(DXGI_FORMAT_R32G32_FLOAT));
        assert_eq!(float_format(DXGI_FORMAT_R8G8B8A8_UNORM), None);
    }

    #[test]
    fn the_chain_holds_over_the_other_eyes_frame_of_the_same_size() {
        let size = (1440, 1600);
        assert!(chain_holds(Some(1), 0, size, size));
        assert!(chain_holds(Some(0), 1, size, size));
        assert!(!chain_holds(Some(0), 0, size, size), "the same eye twice: a frame went missing");
        assert!(!chain_holds(None, 0, size, size), "no frame before");
        assert!(!chain_holds(Some(1), 0, (1280, 1600), size), "the size changed");
    }

    /// The Beast's DLSS view to clip (row vectors): reverse-Z infinite, near 0.05.
    const VIEW_TO_CLIP: [f32; 16] = [1.036, 0.0, 0.0, 0.0, 0.0, 1.84, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.05, 0.0];
    const PLACE: Place = Place { position: [3.0, 1.7, -8.0], right: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0], forward: [-1.0, 0.0, 0.0] };

    fn put(bytes: &mut [u8], offset: usize, values: &[f32]) {
        for (i, v) in values.iter().enumerate() {
            bytes[offset + 4 * i..][..4].copy_from_slice(&v.to_le_bytes());
        }
    }

    /// The game's constants with the camera's projection and `reset`.
    fn constants(reset: bool) -> Vec<u8> {
        let mut bytes = vec![0u8; at::BYTES];
        put(&mut bytes, at::CAMERA_VIEW_TO_CLIP, &VIEW_TO_CLIP);
        put(&mut bytes, at::NEAR, &[0.05]);
        bytes[at::RESET] = reset as u8;
        bytes
    }

    /// The same frame as DLSS's constants and as FSR's dispatch (2560x1440, vectors in pixels).
    fn both_sources() -> (Constants, GameDispatch) {
        let (width, height) = (2560u32, 1440u32);
        let mut bytes = constants(false);
        put(&mut bytes, at::POSITION, &PLACE.position);
        put(&mut bytes, at::UP, &PLACE.up);
        put(&mut bytes, at::RIGHT, &PLACE.right);
        put(&mut bytes, at::FORWARD, &PLACE.forward);
        put(&mut bytes, at::JITTER, &[0.25, -0.125]);
        put(&mut bytes, at::MVEC_SCALE, &[1.0 / width as f32, 1.0 / height as f32]);
        put(&mut bytes, at::FAR, &[4000.0]);
        put(&mut bytes, at::FOV, &[2.0 * (1.0f32 / 1.84).atan()]);
        let resource = GameResource { resource: 0, format: 0, width, height, state: 0 };
        let dispatch = GameDispatch {
            command_list: 0,
            color: resource,
            depth: resource,
            motion_vectors: resource,
            reactive: resource,
            transparency_and_composition: resource,
            output: resource,
            jitter: Float2 { x: 0.25, y: -0.125 },
            motion_vector_scale: Float2 { x: 1.0, y: 1.0 },
            render_size: Size2 { width, height },
            upscale_size: Size2 { width: 3840, height: 2160 },
            sharpening: None,
            frame_time_ms: 16.0,
            pre_exposure: 1.0,
            reset: false,
            near: 0.05,
            far: 4000.0,
            fov_vertical: 2.0 * (1.0f32 / 1.84).atan(),
            view_space_to_meters: 1.0,
            flags: 0,
        };
        (Constants::read(bytes.as_ptr()).unwrap(), dispatch)
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 1e-5 * a.abs().max(1.0))
    }

    #[test]
    fn fsrs_dispatch_tells_the_features_what_dlss_constants_do() {
        let (constants, dispatch) = both_sources();
        let dlss = Rendering::from_constants(&constants);
        let frustum = Frustum::centred([1.0 / 1.036, 1.0 / 1.84]).unwrap();
        let flags = create::DEPTH_INVERTED | create::DEPTH_INFINITE;
        let fsr = Rendering::from_dispatch(&dispatch, PLACE, &frustum, flags).unwrap();
        assert!(close(&fsr.view_to_clip, &dlss.view_to_clip), "{:?} {:?}", fsr.view_to_clip, dlss.view_to_clip);
        let (a, b) = (fsr.camera, dlss.camera);
        assert!(close(&[a.motion_vector_scale.x, a.motion_vector_scale.y], &[b.motion_vector_scale.x, b.motion_vector_scale.y]), "to the screen");
        assert_eq!((a.jitter, a.near, a.far, a.position, a.right, a.up, a.forward), (b.jitter, b.near, b.far, b.position, b.right, b.up, b.forward));
        assert!((a.fov_vertical - b.fov_vertical).abs() < 1e-6);
        let size = Size2 { width: 2560, height: 1440 };
        let (a, b) = (a.for_render(size), b.for_render(size));
        assert!(close(&[a.motion_vector_scale.x, a.motion_vector_scale.y], &[1.0, 1.0]) && close(&[b.motion_vector_scale.x, b.motion_vector_scale.y], &[1.0, 1.0]), "pixels for frame generation");

        // The other depth conventions: depth 1 at the near plane when reversed, 0 when not; the far
        // plane at the other end unless infinite.
        let depth = |p: &[f32; 16], distance: f32| {
            // Row vectors: a point `distance` ahead (view space looks down -z).
            let (z, w) = (-distance * p[10] + p[14], -distance * p[11] + p[15]);
            z / w
        };
        for (flags, near, far) in [(create::DEPTH_INVERTED, 1.0, 0.0), (0, 0.0, 1.0), (create::DEPTH_INFINITE, 0.0, 1.0)] {
            let r = Rendering::from_dispatch(&dispatch, PLACE, &frustum, flags).unwrap();
            assert!((depth(&r.view_to_clip, 0.05) - near).abs() < 1e-5, "flags {flags}: near");
            let at_far = if flags & create::DEPTH_INFINITE != 0 { 1e9 } else { 4000.0 };
            assert!((depth(&r.view_to_clip, at_far) - far).abs() < 1e-4, "flags {flags}: far");
        }
        // No far plane given: infinite. No render size or near plane: nothing to go on.
        let endless = Rendering::from_dispatch(&GameDispatch { far: f32::INFINITY, ..dispatch }, PLACE, &frustum, create::DEPTH_INVERTED).unwrap();
        assert!(close(&endless.view_to_clip, &dlss.view_to_clip));
        assert!(Rendering::from_dispatch(&GameDispatch { render_size: Size2::default(), ..dispatch }, PLACE, &frustum, flags).is_none());
        assert!(Rendering::from_dispatch(&GameDispatch { near: 0.0, ..dispatch }, PLACE, &frustum, flags).is_none());
    }

    fn device() -> ID3D12Device {
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: creates a device on the default adapter.
        unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
        device.unwrap()
    }

    /// The game's tag array for `textures` ((buffer type, texture)), with the `sl::Resource`s it
    /// points at (kept alive beside it).
    fn tag_array(textures: &[(u32, &ID3D12Resource)]) -> (Vec<u8>, Vec<[u8; 128]>) {
        let resources: Vec<[u8; 128]> = textures
            .iter()
            .map(|(_, texture)| {
                let mut resource = [0u8; 128];
                resource[32] = RESOURCE_TEX2D;
                resource[40..48].copy_from_slice(&(texture.as_raw() as usize).to_le_bytes());
                resource
            })
            .collect();
        let mut tags = vec![0u8; textures.len() * TAG_BYTES];
        for (i, (kind, _)) in textures.iter().enumerate() {
            tags[i * TAG_BYTES + 32..][..8].copy_from_slice(&(resources[i].as_ptr() as usize).to_le_bytes());
            tags[i * TAG_BYTES + 40..][..4].copy_from_slice(&kind.to_le_bytes());
        }
        (tags, resources)
    }

    /// The consumers' asks for the steps that follow.
    fn ask(motion: Motion, accept: Option<fn(&Rendering) -> bool>) {
        disarm();
        want(motion, accept);
        arm();
    }

    /// The store is the run's: one test walks it.
    #[test]
    fn inputs_come_fresh_until_their_eyes_present_takes_them() {
        clear();
        let device = device();
        // SAFETY: creates a direct queue on a live device.
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }) }.unwrap();
        let none = D3D12_RESOURCE_FLAG_NONE;
        let depth = d3d12::texture(&device, 16, 8, DXGI_FORMAT_R32_FLOAT, none).unwrap();
        let motion = d3d12::texture(&device, 16, 8, DXGI_FORMAT_R16G16_FLOAT, none).unwrap();
        let colour = d3d12::texture(&device, 12, 6, DXGI_FORMAT_R8G8B8A8_UNORM, none).unwrap();
        let mut recorder = Recorder::new(&device, 1).unwrap();
        let list = recorder.begin().unwrap().unwrap();
        let common = D3D12_RESOURCE_STATE_COMMON;

        // Known at each call: each input fresh once tagged, both taken at the present.
        tagged(&list, 0, Input::Depth, &depth, common, false);
        assert_eq!(fresh(0), Fresh { depth: true, motion: false });
        tagged(&list, 0, Input::MotionVectors, &motion, common, false);
        let (inputs, _) = take_late(0).unwrap();
        assert!(inputs.fresh.both() && inputs.depth.is_some() && inputs.motion.is_some());
        assert_eq!(inputs.motion_format, DXGI_FORMAT_R16G16_FLOAT, "in their own format");
        assert_eq!(fresh(0), Fresh::default(), "taken");
        // Late: kept at the tag, fresh, copied at the present into the same copies.
        tagged(&list, 0, Input::Depth, &depth, common, true);
        tagged(&list, 0, Input::MotionVectors, &motion, common, true);
        let (late_inputs, late) = take_late(0).unwrap();
        assert!(late_inputs.fresh.both());
        assert!(late.depth.as_ref().is_some_and(|(.., target)| Some(target) == inputs.depth.as_ref()));
        late.record(&list);
        assert!(take_late(0).unwrap().1.depth.is_none(), "copied once");
        assert!(take(1).is_none(), "the other eye has none");

        // Labelled at the constants, copied at the evaluate.
        let (tags, _resources) = tag_array(&[(buffer::DEPTH, &depth), (buffer::MOTION_VECTORS, &motion), (buffer::SCALING_INPUT_COLOR, &colour)]);
        let tags = read_tags(tags.as_ptr(), 3);
        // The render size: the input colour's, else the depth's.
        sizes_tagged(&tags);
        assert_eq!(render_sizes(), Some(((12, 6), None)));
        let (depth_only, _kept) = tag_array(&[(buffer::DEPTH, &depth)]);
        sizes_tagged(&read_tags(depth_only.as_ptr(), 1));
        assert_eq!(render_sizes(), Some(((16, 8), None)));
        let still = constants(false);
        let frame = |eye: Option<usize>| {
            at_constants(still.as_ptr(), eye);
            at_tags(&tags);
            at_evaluate(Some(list.clone()));
        };
        let copied = copies()[1];
        ask(Motion::Float, None);
        frame(Some(1));
        let inputs = peek(1).unwrap();
        assert_eq!(inputs.fresh, Fresh { depth: true, motion: true });
        assert_eq!((inputs.motion_format, inputs.rect, inputs.reset), (DXGI_FORMAT_R16G16_FLOAT, Some([0, 0, 12, 6]), false));
        assert!(inputs.camera().is_some_and(|c| c.near == 0.05));
        assert_eq!(inputs.rendering.map(|r| r.view_to_clip), Some(VIEW_TO_CLIP));
        assert_eq!(copies()[1], copied + 1);
        assert!(take(1).unwrap().fresh.both());
        assert!(!take(1).unwrap().fresh.depth, "taken once");
        // A frame not labelled, or refused by any consumer, leaves the eye's inputs as they were.
        frame(None);
        want(Motion::None, Some(|_| true));
        want(Motion::None, Some(|_| false));
        frame(Some(1));
        assert_eq!(fresh(1), Fresh::default());
        // The evaluate takes the labelled frame once.
        ask(Motion::Float, None);
        at_evaluate(Some(list.clone()));
        assert_eq!(fresh(1), Fresh::default());
        // Without the motion vectors: the depth alone.
        ask(Motion::None, Some(|_| true));
        frame(Some(1));
        let inputs = take(1).unwrap();
        assert_eq!(inputs.fresh, Fresh { depth: true, motion: false });
        assert!(inputs.motion.is_none());
        // The game's reset.
        let cut = constants(true);
        at_constants(cut.as_ptr(), Some(0));
        at_tags(&tags);
        at_evaluate(Some(list.clone()));
        assert!(take(0).unwrap().reset);

        // Chained (the most asked wins): the chain holds over the other eye's frame just before.
        ask(Motion::Float, None);
        want(Motion::Chained, None);
        want(Motion::None, None);
        assert!(collecting());
        frame(Some(0));
        assert!(take(0).unwrap().reset, "no frame before");
        frame(Some(1));
        let inputs = take(1).unwrap();
        assert!(!inputs.reset && inputs.fresh.both(), "the left eye's frame before");
        assert_eq!(inputs.motion_format, DXGI_FORMAT_R16G16B16A16_FLOAT);
        frame(Some(1));
        assert!(take(1).unwrap().reset, "the same eye twice");
        frame(Some(0));
        assert!(!take(0).unwrap().reset);
        // An evaluate without a labelled frame, or a frame without its tags, breaks the chain.
        at_evaluate(Some(list.clone()));
        frame(Some(1));
        assert!(take(1).unwrap().reset, "an evaluate with no labelled frame");
        frame(Some(0));
        assert!(!take(0).unwrap().reset, "it holds again");
        at_constants(still.as_ptr(), Some(1));
        at_evaluate(Some(list.clone()));
        frame(Some(0));
        assert!(take(0).unwrap().reset, "a frame without its tags");
        // No native list breaks it too.
        frame(Some(0));
        at_constants(still.as_ptr(), Some(1));
        at_tags(&tags);
        at_evaluate(None);
        frame(Some(1));
        assert!(take(1).unwrap().reset, "no native list");

        // FSR's dispatch: the same store, the same chain (the vectors scaled to render pixels by
        // the game's scale), whichever source gave the frame before.
        let (_, dispatch) = both_sources();
        let frustum = Frustum::centred([1.0 / 1.036, 1.0 / 1.84]).unwrap();
        let rendering = Rendering::from_dispatch(&dispatch, PLACE, &frustum, create::DEPTH_INVERTED | create::DEPTH_INFINITE).unwrap();
        let fsr = |eye: usize, reset: bool| Upscaled { eye, rendering, reset, depth: (&depth, common), motion: (&motion, common), render: Some((12, 6)), chain_scale: [16.0, 8.0] };
        upscaled(&list, Some(fsr(0, false)));
        let inputs = take(0).unwrap();
        assert!(!inputs.reset && inputs.fresh.both(), "chained over the DLSS frame just before");
        assert_eq!((inputs.rendering, inputs.rect), (Some(rendering), Some([0, 0, 12, 6])));
        upscaled(&list, Some(fsr(1, true)));
        assert!(take(1).unwrap().reset, "the game's reset");
        upscaled(&list, None);
        upscaled(&list, Some(fsr(0, false)));
        assert!(take(0).unwrap().reset, "a dispatch of no eye breaks the chain");
        upscaled(&list, Some(fsr(1, false)));
        assert!(!take(1).unwrap().reset);
        ask(Motion::Float, None);
        upscaled(&list, Some(fsr(0, false)));
        let inputs = take(0).unwrap();
        assert_eq!((inputs.fresh, inputs.motion_format), (Fresh { depth: true, motion: true }, DXGI_FORMAT_R16G16_FLOAT));
        ask(Motion::None, Some(|r| r.camera.near > 1.0));
        upscaled(&list, Some(fsr(0, false)));
        assert_eq!(fresh(0), Fresh::default(), "refused");
        sizes_known((2560, 1440), Some((3840, 2160)));
        assert_eq!(render_sizes(), Some(((2560, 1440), Some((3840, 2160)))));
        sizes_known((0, 1440), None);
        assert_eq!(render_sizes(), Some(((2560, 1440), Some((3840, 2160)))), "no size");

        recorder.submit(&queue).unwrap();
        assert!(recorder.idle(2000));
        disarm();
        assert!(!collecting());
        clear();
        assert!(take(0).is_none() && take(1).is_none());
    }
}
