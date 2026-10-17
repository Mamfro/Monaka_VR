//! Capability providers: what a game says once and every ready-made feature reads, so a feature is
//! one line in a game's component list.
//!
//! - [`EyeGeometry`]: how each eye is drawn (its frustum, the eyes' separation), read by the eye
//!   site and by the features that rebuild an eye's view.
//! - [`FrameLabels`]: which eye, drawn from which head pose, the frame being rendered and the one
//!   being presented are.
//! - [`FrameInputs`]: each eye's depth, motion vectors, camera and render size, collected from the
//!   calls the game makes for its upscaler ([`Sources`]: Streamline's for DLSS, FSR's dispatch) by
//!   the ready-made [`frame_inputs`] component for the features that ask
//!   (`monaka_framegen::inputs`); the features read the same inputs whichever upscaler runs.
//! - [`HudLayer`]: the game's HUD kept out of its frames as a layer and laid back over each eye at
//!   its present request, also over the images features make of their own ([`before_hud`],
//!   [`lay_hud_beside`]); on D3D12 the layer itself ([`HudLayer12`]), for the lay-over
//!   (`crate::hud_layer::present12`).
//!
//! A feature bounds its game on the providers it reads (`crate::afw::d3d12::frame_warping`,
//! `crate::depth_layer::depth_layer`, `crate::foveate12::foveation`, `crate::framegen12::per_eye`),
//! so a game without one does not compile.

use crate::driver::Pending;
use crate::hud_layer::d3d12::Params;
use monaka_core::camera::Frustum;
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_ffx::{Size2, state};
use monaka_framegen::ffx_imports;
use monaka_framegen::inputs::{self, Input, Motion, Place, Rendering, Upscaled};
use monaka_ffx::upscale::{GameDispatch, create};
use monaka_producer::game::{Component, Game, Lack, Start};
use monaka_producer::log_first;
use monaka_streamline::imports::{self, Handlers};
use monaka_streamline::{Constants, Tag, buffer, feature};
use std::ffi::c_void;
use std::sync::Mutex;
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12GraphicsCommandList, ID3D12Resource};
use windows::core::Interface;

// --- Eye geometry --------------------------------------------------------------------------------

/// How the game draws each eye: what the eye site writes and the features that rebuild an eye's
/// view read (`crate::eye_site::EyeSite`, [`FrameLabels`]).
pub trait EyeGeometry: Game {
    /// The frustum eye `eye` of `pose` is drawn with (`None`: not known here; the field of view the
    /// pose carries, or the game's own).
    fn frustum(pose: &HeadPose, eye: usize) -> Option<Frustum>;
    /// The eyes' separation for `pose`, in the game's units (`None`: not known yet).
    fn separation(pose: &HeadPose) -> Option<f32>;
}

// --- Frame labels --------------------------------------------------------------------------------

/// Which eye each frame is.
pub trait FrameLabels: EyeGeometry {
    /// The eye (0 left) and head pose of the frame the renderer records now, once the game knows it;
    /// `None` for a frame of no eye (a menu, a centre view). What the frame's inputs are kept as.
    fn labelled() -> Option<Pending>;
    /// The same while the frame's passes are recorded, foreseen before the game knows it (for a
    /// game that labels a frame partway through). By default [`labelled`](Self::labelled).
    fn rendering() -> Option<Pending> {
        Self::labelled()
    }
    /// The eye and pose of the frame about to present, during its present request.
    fn presenting() -> Option<Pending>;
    /// Whether the eyes are drawn now (stereo runs).
    fn running() -> bool;
    /// The game camera's own frustum, for a frame no eye camera drew.
    fn own_frustum() -> Option<Frustum> {
        None
    }
}

// --- Frame inputs --------------------------------------------------------------------------------

/// Where each eye's inputs are copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyAt {
    /// At DLSS's evaluate, on the game's native command list: the frame's eye is known at its
    /// constants ([`FrameLabels::labelled`] there).
    Evaluate,
    /// At each tag, on the tag's command list: the frame's eye is known at each call. The
    /// frame-generation call's buffers (complete only by the present) are copied at the eye's
    /// present ([`FrameInputs::late_at_present`]).
    Tags,
}

/// Where a game's frame inputs come from: the calls it makes for its upscaler. A game with both
/// gives its inputs from whichever upscaler it runs; the features do not tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sources {
    /// Its Streamline calls for DLSS, copied where [`CopyAt`] says (the game hooks its renderer's
    /// Streamline imports: `monaka_streamline::imports::install`).
    pub streamline: Option<CopyAt>,
    /// Its FSR upscale dispatch, copied there on the game's command list before FSR (the game hooks
    /// its renderer's FidelityFX imports: `monaka_framegen::ffx_imports::install`). The dispatch
    /// gives the depth, motion vectors, jitter, near and far planes, field of view and render size;
    /// the frame's label gives its eye and frustum, and [`FrameInputs::upscaled_place`] where its
    /// camera was.
    pub fsr: bool,
}

/// A game whose upscaler calls give each eye's inputs ([`frame_inputs`] collects them).
pub trait FrameInputs: FrameLabels {
    const SOURCES: Sources;
    /// The game's world units in a metre (each eye's depth to the headset).
    const UNITS_PER_METRE: f32 = 1.0;
    /// [`CopyAt::Tags`]: the frame-generation call's buffers are kept and copied at the eye's
    /// present (true), or copied at their tags.
    fn late_at_present() -> bool {
        true
    }
    /// [`Sources::fsr`]: where the camera was that the frame being upscaled was rendered from,
    /// asked during the game's dispatch (the dispatch does not carry it). `None`: the frame is not
    /// taken.
    fn upscaled_place() -> Option<Place> {
        None
    }
}

/// Each eye's inputs from the game's upscaler calls, for the features that ask (at their install,
/// `monaka_framegen::inputs::want`); collecting starts at the arm, when any did. The game hooks its
/// renderer's imports for each of its [`Sources`] (in a component of its own) and lists this
/// anywhere.
pub const fn frame_inputs<G: FrameInputs>() -> Component<G> {
    Component { name: "frame inputs", install: install_inputs::<G>, arm: inputs::arm, quiet: inputs::disarm, release: release_inputs, ..Component::NONE }
}

fn install_inputs<G: FrameInputs>(_: &mut Start<'_, G>) -> Result<(), Lack> {
    if G::SOURCES.streamline.is_some() {
        imports::provide(Handlers { tags: Some(tagged::<G>), constants: Some(constants::<G>), unchanged: None, evaluate: Some(evaluated::<G>) });
    }
    if G::SOURCES.fsr {
        ffx_imports::provide(Some(upscaled::<G>));
    }
    Ok(())
}

fn release_inputs() {
    imports::provide(Handlers::default());
    ffx_imports::provide(None);
    inputs::disarm();
}

/// `slSetTagForFrame`: the render size, and the frame's depth and motion vectors.
fn tagged<G: FrameInputs>(_: u64, _: usize, _: *const u8, tags: &[Tag], commands: *mut c_void) {
    inputs::sizes_tagged(tags);
    if !inputs::collecting() {
        return;
    }
    match G::SOURCES.streamline {
        Some(CopyAt::Evaluate) => inputs::at_tags(tags),
        Some(CopyAt::Tags) => copy_tagged::<G>(tags, commands),
        None => {}
    }
}

/// [`CopyAt::Tags`]: the frame's depth and motion vectors copied on the game's list for its eye.
fn copy_tagged<G: FrameInputs>(tags: &[Tag], commands: *mut c_void) {
    let Some(eye) = G::labelled().map(|l| l.eye) else { return };
    if commands.is_null() {
        return;
    }
    // SAFETY: the game's command list, being recorded for this frame.
    let Some(list) = (unsafe { ID3D12GraphicsCommandList::from_raw_borrowed(&commands) }) else { return };
    // The frame-generation call (it carries the UI layer or the HUD-less frame): its buffers are
    // "valid until present", complete only by then.
    let late = G::late_at_present() && tags.iter().any(|t| t.kind == buffer::UI_COLOR_AND_ALPHA || t.kind == buffer::HUDLESS_COLOR);
    let motion = inputs::motion_wanted() != Motion::None;
    for tag in tags {
        let input = match tag.kind {
            buffer::DEPTH => Input::Depth,
            buffer::MOTION_VECTORS if motion => Input::MotionVectors,
            _ => continue,
        };
        if let Some(resource) = tag.texture() {
            inputs::tagged(list, eye, input, resource, tag.state(), late);
        }
    }
}

/// `slSetConstants`: the camera the frame is rendered with, for its eye.
fn constants<G: FrameInputs>(_: u64, constants: *const u8, _: usize, _: *const u8) -> Option<usize> {
    if inputs::collecting() {
        let eye = G::labelled().map(|l| l.eye);
        match G::SOURCES.streamline {
            Some(CopyAt::Evaluate) => inputs::at_constants(constants, eye),
            Some(CopyAt::Tags) => {
                if let (Some(eye), Some(read)) = (eye, Constants::read(constants)) {
                    inputs::set_rendering(eye, Rendering::from_constants(&read));
                }
            }
            None => {}
        }
    }
    None
}

/// `slEvaluateFeature`: DLSS's inputs are complete ([`CopyAt::Evaluate`]).
fn evaluated<G: FrameInputs>(_: u64, feature: u32, _: *const u8, _: *const *const u8, _: u32, commands: *mut c_void) {
    if G::SOURCES.streamline == Some(CopyAt::Evaluate) && feature == feature::DLSS && inputs::collecting() {
        // SAFETY: the command list the game's call was given.
        inputs::at_evaluate(unsafe { imports::native_list(commands) });
    }
}

/// The game's FSR upscale dispatch ([`Sources::fsr`]), as the game made it: the render size, and
/// the frame's depth, motion vectors and camera for its eye, copied on the game's command list
/// before FSR (its native list behind Streamline's proxy when there is one).
fn upscaled<G: FrameInputs>(dispatch: &GameDispatch) {
    let size = |s: Size2| (s.width, s.height);
    inputs::sizes_known(size(dispatch.render_size), Some(size(dispatch.upscale_size)));
    if !inputs::collecting() {
        return;
    }
    let raw_list = dispatch.command_list as *mut c_void;
    // SAFETY: the command list the game's dispatch was given, being recorded.
    let Some(list) = (unsafe { imports::native_list(raw_list) }).or_else(|| unsafe { ID3D12GraphicsCommandList::from_raw_borrowed(&raw_list) }.cloned()) else { return };
    let (raw_depth, raw_motion) = (dispatch.depth.resource as *mut c_void, dispatch.motion_vectors.resource as *mut c_void);
    // SAFETY: the game's textures, live while it records this frame.
    let (depth, motion) = unsafe { (ID3D12Resource::from_raw_borrowed(&raw_depth), ID3D12Resource::from_raw_borrowed(&raw_motion)) };
    let frame = || -> Result<Upscaled, &str> {
        let label = G::labelled().ok_or("no label")?;
        let scale = dispatch.motion_vector_scale;
        Ok(Upscaled {
            eye: label.eye,
            rendering: upscaled_rendering::<G>(dispatch, &label)?,
            reset: dispatch.reset,
            depth: depth.zip(state::d3d12(dispatch.depth.state)).ok_or("no depth in a state FFX names")?,
            motion: motion.zip(state::d3d12(dispatch.motion_vectors.state)).ok_or("no motion vectors in a state FFX names")?,
            render: Some(size(dispatch.render_size)),
            chain_scale: [scale.x, scale.y],
        })
    };
    let frame = match frame() {
        Ok(frame) => {
            let ((rw, rh), (ow, oh)) = (size(dispatch.render_size), size(dispatch.upscale_size));
            log_first(1, format_args!("frame inputs: from FSR's dispatch: {rw}x{rh} upscaled to {ow}x{oh}, near {}, far {}, context flags {:?}", dispatch.near, dispatch.far, ffx_imports::upscale_flags()));
            Some(frame)
        }
        Err(why) => {
            log_first(5, format_args!("frame inputs: an FSR frame not taken: {why}"));
            None
        }
    };
    inputs::upscaled(&list, frame);
}

/// How the frame being upscaled (labelled `label`) was rendered: the dispatch's camera with where
/// the game says it was, its eye's frustum, the upscale context's depth convention.
fn upscaled_rendering<G: FrameInputs>(dispatch: &GameDispatch, label: &Pending) -> Result<Rendering, &'static str> {
    let place = G::upscaled_place().ok_or("no camera place")?;
    let frustum = G::frustum(&label.pose, label.eye).or_else(G::own_frustum).ok_or("no frustum")?;
    let flags = ffx_imports::upscale_flags().unwrap_or_else(|| {
        log_first(1, format_args!("frame inputs: FSR's depth convention is not known (its context was made before the hooks); taken as reversed and infinite"));
        create::DEPTH_INVERTED | create::DEPTH_INFINITE
    });
    Rendering::from_dispatch(dispatch, place, &frustum, flags).ok_or("no render size or near plane")
}

// --- D3D11 frame inputs --------------------------------------------------------------------------

/// A D3D11 game's frame inputs, for the features that work on each eye's world image at the
/// game's scene copy (AFW, `crate::afw::d3d11::ready`; the per-eye FSR upscaler,
/// `crate::upscaler::fsr11`). Its draws hand them each frame's world image (the scene copied to the
/// back buffer, before the HUD) and the HUD's draws; its eye site records the cameras each frame
/// was rendered with.
pub trait FrameInputs11: FrameLabels {
    /// The game's world units in a metre (each eye's depth to the headset).
    const UNITS_PER_METRE: f32 = 1.0;
    /// The game's D3D11 device (its swapchain's), for the features' own work.
    fn device(build: &Self::Build) -> Result<windows::Win32::Graphics::Direct3D11::ID3D11Device, monaka_producer::Rejection>;
    /// The scene depth of the frame shown at present `n`, kept from its scene copy.
    fn scene_depth(n: u64) -> Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>;
    /// Where the HUD layer goes in an eye image of the HUD's place `rect` (x, y, width, height): a
    /// game whose UI keeps a layout of its own moves it. By default `rect`.
    fn hud_rect(rect: [f32; 4]) -> [f32; 4] {
        rect
    }
    /// A probe's look at each image a feature publishes: the back buffer, the image going out,
    /// the HUD layer drawn on black, and what the image is ("eye", "flat"). By default none.
    fn probe(
        _device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
        _context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
        _back: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        _image: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        _hud: Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>,
        _what: &str,
    ) {
    }
}

// --- The HUD layer -------------------------------------------------------------------------------

/// A game whose HUD is kept out of its frames as a layer and laid back over each eye at its present
/// request. That request calls [`before_hud`] with the frame before the HUD goes over it, and
/// [`lay_hud_beside`] after, so the features' own images of the frame get the HUD too.
pub trait HudLayer: FrameLabels {
    /// How the layer goes over eye `eye`'s image of `size` (`None`: the eye of the frame about to
    /// present): its place, size and disparity. `None` while the game's own layout stands.
    fn over(size: (u32, u32), eye: Option<usize>) -> Option<Params>;
}

/// A D3D12 game's HUD layer itself, for the lay-over at its present request
/// (`crate::hud_layer::present12`): taken there with the frame's HUD, given back after.
pub trait HudLayer12: HudLayer {
    /// The layer with this frame's HUD (premultiplied RGBA of `size`, in COMMON), at the present;
    /// `None` until the HUD has been drawn into one.
    fn take_layer(queue: &ID3D12CommandQueue, size: (u32, u32)) -> Option<ID3D12Resource>;
    /// After the present's work with the layer: given back for the next frame's HUD.
    fn return_layer(queue: &ID3D12CommandQueue);
    /// The HUD is kept out of the frame (the lay-over puts it back); else it is drawn in as well.
    fn kept_out() -> bool;
    /// From the next frame's draws on, the HUD goes to the frame as the game draws it (`through`,
    /// a menu shown whole) or is kept out again.
    fn let_through(through: bool);
    /// Once a frame, at the present, after the layer's work.
    fn frame_end() {}
}

/// A feature's work on the frame (in PRESENT) before the HUD goes over it.
pub type BeforeHud = fn(&ID3D12CommandQueue, &ID3D12Resource);
/// A feature's own images of the frame given the HUD `layer` (in COMMON), each eye as its params say.
pub type HudBeside = fn(&ID3D12CommandQueue, &ID3D12Resource, [Params; 2]);

static BEFORE: Mutex<Vec<BeforeHud>> = Mutex::new(Vec::new());
static BESIDE: Mutex<Vec<HudBeside>> = Mutex::new(Vec::new());

/// A feature (at its install) works on each frame before its HUD goes over it, and has the HUD laid
/// over images of its own after.
pub fn join_hud(before: BeforeHud, beside: HudBeside) {
    let mut list = BEFORE.locked();
    if !list.iter().any(|&b| std::ptr::fn_addr_eq(b, before)) {
        list.push(before);
    }
    let mut list = BESIDE.locked();
    if !list.iter().any(|&b| std::ptr::fn_addr_eq(b, beside)) {
        list.push(beside);
    }
}

/// The feature leaves (at its release).
pub fn leave_hud(before: BeforeHud, beside: HudBeside) {
    BEFORE.locked().retain(|&b| !std::ptr::fn_addr_eq(b, before));
    BESIDE.locked().retain(|&b| !std::ptr::fn_addr_eq(b, beside));
}

/// The game's present request, in play (no menu), stereo running: `frame` (the back buffer, in
/// PRESENT) before the HUD goes over it, for the features that work on it.
pub fn before_hud(queue: &ID3D12CommandQueue, frame: &ID3D12Resource) {
    let before = BEFORE.locked().clone();
    for work in before {
        work(queue, frame);
    }
}

/// The game's present request, after it laid the HUD `layer` (in COMMON) over the frame of `size`
/// (in play, stereo running): the same layer over the features' own images, each eye where
/// [`HudLayer::over`] puts it (the whole frame when it says nothing), `leave_out` (the pieces on the
/// hands) cut out.
pub fn lay_hud_beside<G: HudLayer>(queue: &ID3D12CommandQueue, layer: &ID3D12Resource, size: (u32, u32), leave_out: &[[f32; 4]]) {
    let beside = BESIDE.locked().clone();
    if beside.is_empty() {
        return;
    }
    let whole = whole_frame(size);
    let over = |eye| G::over(size, Some(eye)).unwrap_or(whole).leaving_out(leave_out);
    let params = [over(0), over(1)];
    for lay in beside {
        lay(queue, layer, params);
    }
}

/// `crate::hud_layer::d3d12`'s params for the whole frame of `size`: the HUD where the game laid it out.
pub fn whole_frame(size: (u32, u32)) -> Params {
    Params { rect: [0.0, 0.0, size.0 as f32, size.1 as f32], ..Default::default() }
}

#[cfg(test)]
mod tests;
