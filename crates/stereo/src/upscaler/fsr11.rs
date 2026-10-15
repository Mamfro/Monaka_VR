//! The per-eye upscaler on D3D11: AMD's FSR (`monaka_fsr11`) for an alternate-eye game on the
//! present loop that renders each eye smaller, as a ready-made component ([`per_eye`]). Each eye
//! keeps its own upscaling history.
//!
//! - Each eye's projection is shifted by that eye's next FSR jitter: the game's eye site asks for it
//!   ([`jitter_for`], [`jittered`]) and records the cameras the frame was rendered with, unjittered
//!   ([`record`]).
//! - At the game's scene copy (the world, before the HUD) the frame is upscaled with motion vectors
//!   from its depth and its eye's cameras ([`capture`]; the frame inputs,
//!   `crate::providers::FrameInputs11`). The HUD's draws go into a layer of their own
//!   ([`hud_targets`]).
//! - At the present loop's step ([`before_present`], joined at install) the HUD layer goes over the
//!   upscaled eye (the frame labels' presenting eye, at its place from `crate::hud_layer::d3d11::eye_rects`)
//!   and that image goes out in place of the back buffer. Frames with no 3D view (a movie, a loading
//!   screen) have nothing to upscale: they go to the viewer's flat screen, resampled to the eye
//!   size.
//!
//! The game's own object motion (a G-buffer target), when it writes one, is added with a scale the
//! game gives ([`object_motion`]).

use crate::alternate::{Step, d3d11::AlternatePublisher};
use crate::driver::{Driver, Pending};
use crate::feature::Presents;
use crate::afw::d3d11::EyeCameras;
use crate::providers::FrameInputs11;
use monaka_channel::pose::HeadSource;
use monaka_core::alternate::{PresentRing, Schedule};
use monaka_core::camera::Frustum;
use monaka_core::eye_history::{EyeHistory, Rules};
use monaka_core::feature::Feature;
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_fsr11::{EyeFrame, Fsr11, Settings};
use monaka_ffx::upscale::create;
use monaka_producer::game::Component;
use monaka_producer::log;
use monaka_warp::{EyeView, HudLayer, ObjectMotion, Scaler};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::*};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
use windows::core::Interface;

#[cfg(test)]
mod tests;

/// The per-eye FSR upscaler component for game `G`: the feature (per-eye history), its release.
/// The game adds its options, its condition and its install (reading them, then [`start`]) by
/// struct update.
pub const fn per_eye<G: Presents + FrameInputs11>() -> Component<G> {
    Component { name: "fsr", features: &[Feature::PerEyeHistory], release, summary: Some(summary), ..Component::NONE }
}

/// What the upscaler reads of its game.
#[derive(Clone, Copy)]
struct GameFns {
    presenting: fn() -> Option<Pending>,
    scene_depth: fn(u64) -> Option<ID3D11Texture2D>,
    eye_rects: fn(&HeadPose, u32, u32) -> [[f32; 4]; 2],
    hud_rect: fn([f32; 4]) -> [f32; 4],
    probe: fn(&ID3D11Device, &ID3D11DeviceContext, &ID3D11Texture2D, &ID3D11Texture2D, Option<ID3D11Texture2D>, &str),
}

/// A frame's cameras and the jitter its projection was shifted by, kept under the present after
/// which its eye was chosen.
#[derive(Clone, Copy)]
struct Recorded {
    cameras: Option<EyeCameras>,
    jitter: [f32; 2],
}

struct State {
    game: Option<GameFns>,
    fsr: Option<Fsr11>,
    /// The game's object motion target and its scale to UV units (0: not used).
    object_scale: f32,
    gbuffer: Option<ID3D11Texture2D>,
    hud: Option<HudLayer>,
    hud_begun: bool,
    hud_views: Option<[ID3D11RenderTargetView; 2]>,
    recorded: PresentRing<Recorded>,
    /// This frame's upscaled eye: the present that shows it, and the image.
    output: Option<(u64, ID3D11Texture2D)>,
    /// Flat frames resampled to the eye size.
    scaler: Option<Scaler>,
    /// Frames in a row drawn with no 3D view.
    viewless: u32,
    flat: u64,
    /// Each eye's last upscaled frame (when, and the camera's position), and whether its history
    /// carries on.
    last: EyeHistory<(), (Instant, [f32; 3])>,
    upscaled: u64,
    busy: u64,
    missing: u64,
    failures: u64,
}

struct Cell(State);
// SAFETY: the D3D objects inside are used under the mutex on the game's render thread, or by the
// stop after the hooks are off.
unsafe impl Send for Cell {}

static STATE: Mutex<Cell> = Mutex::new(Cell(State {
    game: None,
    fsr: None,
    object_scale: 0.0,
    gbuffer: None,
    hud: None,
    hud_begun: false,
    hud_views: None,
    recorded: PresentRing::new(),
    output: None,
    scaler: None,
    viewless: 0,
    flat: 0,
    last: EyeHistory::new(Rules::OWN),
    upscaled: 0,
    busy: 0,
    missing: 0,
    failures: 0,
}));
static ENABLED: AtomicBool = AtomicBool::new(false);
static CAPTURED: AtomicBool = AtomicBool::new(false);

/// What the game says when it starts the upscaler.
#[derive(Clone, Copy, Debug)]
pub struct Start {
    /// The size the game renders each eye at, and the size each eye is upscaled to.
    pub render: (u32, u32),
    pub display: (u32, u32),
    /// RCAS sharpening 0..1, or none.
    pub sharpness: Option<f32>,
    /// 1 or -1: whether the dispatch is told the content shift or its opposite.
    pub jitter_sign: f32,
    /// The game's depth convention ([`create`] flags: inverted, infinite).
    pub depth_flags: u32,
    /// The game's object motion to UV units (0: not used).
    pub object_motion: f32,
}

/// The game's install: AMD's DLLs loaded from `dlls`, the upscaler made on `device` for `start`,
/// the present loop's step joined; a line for the log.
pub fn start<G: Presents + FrameInputs11>(device: &ID3D11Device, start: Start, dlls: &Path) -> Result<String, String> {
    let api = monaka_fsr11::load(dlls).map_err(|e| format!("FSR: {e}"))?;
    let settings = Settings { render: start.render, display: start.display, sharpness: start.sharpness, jitter_sign: start.jitter_sign, depth_flags: start.depth_flags };
    let fsr = Fsr11::new(device, api, settings).map_err(|e| format!("FSR: {e}"))?;
    let hud = HudLayer::new(device).map_err(|e| format!("FSR HUD layer: {e}"))?;
    let scaler = Scaler::new(device).map_err(|e| format!("FSR flat frames: {e}"))?;
    let line = format!("FSR {}: {}x{} upscaled to {}x{}, {} jitter phases, {start:?}", fsr.version(), start.render.0, start.render.1, start.display.0, start.display.1, fsr.phases());
    let mut state = STATE.locked();
    state.0.game = Some(GameFns { presenting: G::presenting, scene_depth: G::scene_depth, eye_rects: crate::hud_layer::d3d11::eye_rects::<G>, hud_rect: G::hud_rect, probe: G::probe });
    state.0.fsr = Some(fsr);
    state.0.object_scale = start.object_motion;
    state.0.hud = Some(hud);
    state.0.scaler = Some(scaler);
    drop(state);
    crate::driver::join_present(before_present);
    ENABLED.store(true, Release);
    Ok(line)
}

/// A reversed depth with an infinite far plane ([`Start::depth_flags`]).
pub const REVERSED_INFINITE: u32 = create::DEPTH_INVERTED | create::DEPTH_INFINITE;
/// A standard depth (0 near, 1 far) with an infinite far plane.
pub const STANDARD_INFINITE: u32 = create::DEPTH_INFINITE;

/// The upscaler runs this run.
pub fn enabled() -> bool {
    ENABLED.load(Acquire)
}

/// The jitter (pixels, x right, y down) for the frame whose eye `eye` was chosen after present
/// `chosen_at`: the eye's next one the first time, the same one for every later call for that
/// frame. `None` without the upscaler.
pub fn jitter_for(chosen_at: u64, eye: usize) -> Option<[f32; 2]> {
    if !enabled() {
        return None;
    }
    let mut state = STATE.lock().ok()?;
    let state = &mut state.0;
    if let Some(known) = state.recorded.get(chosen_at) {
        return Some(known.jitter);
    }
    let jitter = state.fsr.as_mut()?.jitter(eye);
    state.recorded.put(chosen_at, Recorded { cameras: None, jitter });
    Some(jitter)
}

/// `frustum` shifted so the image content moves by `jitter` pixels (x right, y down) at the
/// render size.
pub fn jittered(frustum: Frustum, jitter: [f32; 2]) -> Frustum {
    let Some(render) = STATE.lock().ok().and_then(|s| s.0.fsr.as_ref().map(Fsr11::render)) else { return frustum };
    jittered_at(frustum, jitter, render)
}

/// [`jittered`] at the render size `render`.
pub fn jittered_at(frustum: Frustum, jitter: [f32; 2], (width, height): (u32, u32)) -> Frustum {
    // Content moves +dx in normalised device x when the edges move -dx * (right - left) / 2.
    let dx = 2.0 * jitter[0] / width as f32 * (frustum.right - frustum.left) / 2.0;
    let dy = -2.0 * jitter[1] / height as f32 * (frustum.up - frustum.down) / 2.0;
    Frustum { left: frustum.left - dx, right: frustum.right - dx, up: frustum.up - dy, down: frustum.down - dy }
}

/// The eye site's cameras (unjittered) of the frame chosen after present `chosen_at`, and the
/// jitter actually written into its projection.
pub fn record(chosen_at: u64, cameras: EyeCameras, jitter: [f32; 2]) {
    if let Ok(mut state) = STATE.lock() {
        state.0.recorded.put(chosen_at, Recorded { cameras: Some(cameras), jitter });
    }
}

/// The game's object motion target, bound this frame (kept when it changes).
pub fn object_motion(texture: ID3D11Texture2D) {
    let Ok(mut state) = STATE.lock() else { return };
    if state.0.gbuffer.as_ref().is_none_or(|known| known.as_raw() != texture.as_raw()) {
        state.0.gbuffer = Some(texture);
    }
}

/// Whether this frame still has to be upscaled (checked before any work on the hot draw path).
pub fn needs_capture() -> bool {
    enabled() && !CAPTURED.load(Acquire)
}

/// The vertical field of view of a canonical projection.
pub fn fov_vertical(projection: &[f32; 16]) -> f32 {
    let (scale, offset) = (projection[5], projection[6]);
    ((offset + 1.0) / scale).atan() - ((offset - 1.0) / scale).atan()
}

/// A jump of more than this (metres) between an eye's frames is a cut: its history goes.
const CUT: f32 = 2.0;
/// The frame time assumed for an eye's first frame (ms).
const FIRST_FRAME_MS: f32 = 22.0;

/// For an eye's frame at `position` now, after its last at `last` (when, where): the frame time
/// (ms) and whether the camera jumped (a cut).
pub fn frame_time_and_cut(last: Option<(Instant, [f32; 3])>, now: Instant, position: [f32; 3]) -> (f32, bool) {
    match last {
        Some((then, at)) => {
            let moved = (0..3).map(|i| (position[i] - at[i]).powi(2)).sum::<f32>().sqrt();
            ((now - then).as_secs_f32() * 1000.0, moved > CUT)
        }
        None => (FIRST_FRAME_MS, false),
    }
}

/// Upscales the frame shown at present `n` (its world image is `color` now), once per frame.
pub fn capture(context: &ID3D11DeviceContext, color: &ID3D11Texture2D, n: u64, schedule: Schedule) {
    if !enabled() || CAPTURED.swap(true, AcqRel) {
        return;
    }
    let eye = schedule.eye_at(n);
    let chosen_at = schedule.chosen_at(n);
    let Ok(mut state) = STATE.lock() else { return };
    let state = &mut state.0;
    let Some(game) = state.game else { return };
    let depth = (game.scene_depth)(n);
    // No eye site call wrote this frame's camera (each records its jitter): no 3D view.
    state.viewless = if state.recorded.get(chosen_at).is_none() { state.viewless.saturating_add(1) } else { 0 };
    let recorded = state.recorded.get(chosen_at).filter(|_| Schedule::camera_eye(chosen_at) == eye);
    let (Some(depth), Some(Recorded { cameras: Some(cameras), jitter, .. })) = (depth, recorded) else {
        state.missing += 1;
        return;
    };
    let projection = cameras.projection[eye];
    let camera = cameras.camera[eye];
    let position = [camera[3], camera[7], camera[11]];
    let now = Instant::now();
    let (frame_time_ms, cut) = frame_time_and_cut(state.last.previous(eye).copied(), now, position);
    let reset = state.last.frame(eye, (), cut).reset.is_some();
    state.last.keep(eye, (now, position));
    let object = state.gbuffer.as_ref().filter(|_| state.object_scale != 0.0).map(|texture| ObjectMotion { texture, scale: [state.object_scale; 2] });
    let frame = EyeFrame {
        color,
        depth: &depth,
        object,
        view: EyeView { camera, projection },
        jitter,
        near: projection[11],
        fov_vertical: fov_vertical(&projection),
        frame_time_ms,
        reset,
    };
    let Some(fsr) = state.fsr.as_mut() else { return };
    match fsr.upscale(context, eye, &frame) {
        Ok(image) => {
            state.output = Some((n, image));
            state.upscaled += 1;
        }
        Err(monaka_fsr11::Error::Busy) => state.busy += 1,
        Err(e) => {
            monaka_producer::log_first(1, format_args!("FSR upscale failed: {e}"));
            state.failures += 1;
        }
    }
}

/// The black and white targets this frame's HUD draws go into, begun at its first HUD draw.
pub fn hud_targets(context: &ID3D11DeviceContext, width: u32, height: u32) -> Option<[ID3D11RenderTargetView; 2]> {
    let mut state = STATE.lock().ok()?;
    let state = &mut state.0;
    if state.hud_begun {
        return state.hud_views.clone();
    }
    match state.hud.as_mut()?.begin(context, width, height) {
        Ok(views) => {
            state.hud_begun = true;
            state.hud_views = Some(views.clone());
            Some(views)
        }
        Err(e) => {
            monaka_producer::log_first(1, format_args!("FSR HUD layer unavailable: {e}"));
            state.failures += 1;
            None
        }
    }
}

/// Frames in a row with no 3D view before they go to the flat screen: a frame now and then without
/// one (a level streaming in) does not flash it.
const FLAT_AFTER: u32 = 4;

/// What goes out at a present: the upscaled eye, flat (no 3D view for a few frames), or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    Eye,
    Flat,
    Nothing,
}

/// At present `n`, with this frame's upscaled eye made at present `made` (if any) and
/// `viewless` frames in a row without a 3D view: what goes out.
pub fn out(n: u64, made: Option<u64>, viewless: u32) -> Out {
    match made {
        Some(shown) if shown == n => Out::Eye,
        _ if viewless >= FLAT_AFTER => Out::Flat,
        _ => Out::Nothing,
    }
}

/// The present loop's step (joined at the upscaler's start): the frame not upscaled at its scene
/// copy (no HUD draws) upscaled now, then the upscaled eye (the HUD laid over) published in place of
/// the back buffer. True: the loop publishes nothing itself.
pub fn before_present(driver: &Driver, chain: &IDXGISwapChain, n: u64) -> bool {
    if !enabled() {
        return false;
    }
    if let Ok((image, device, context)) = monaka_channel::d3d::swapchain_parts(chain) {
        if let Some(schedule) = driver.schedule() {
            capture(&context, &image, n, schedule);
        }
        driver.with_publisher11(|publisher| publish(&device, &context, &image, n, publisher, driver.head(), |pose| driver.report(pose)));
    }
    true
}

/// At present `n`: lays this frame's HUD over its upscaled eye (the frame labels' presenting eye, at
/// its place) and gives it to the publisher. A frame with no 3D view goes out flat instead: `back`
/// (the back buffer: a movie, a loading screen) at the eye size, with whatever HUD it drew over the
/// whole of it.
fn publish(device: &ID3D11Device, context: &ID3D11DeviceContext, back: &ID3D11Texture2D, n: u64, publisher: &mut AlternatePublisher, head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
    CAPTURED.store(false, Release);
    let Ok(mut state) = STATE.lock() else { return Step::Failed };
    let state = &mut state.0;
    let Some(game) = state.game else { return Step::Failed };
    let hud_drawn = std::mem::take(&mut state.hud_begun);
    let output = state.output.take();
    let Some(Pending { eye, pose }) = (game.presenting)() else { return Step::Waiting };
    let image = match (out(n, output.as_ref().map(|(made, _)| *made), state.viewless), output) {
        (Out::Eye, Some((_, image))) => image,
        (Out::Flat, _) => return publish_flat(state, game, device, context, back, n, publisher, head, hud_drawn),
        _ => return Step::Failed,
    };
    let mut image = image;
    (game.probe)(device, context, back, &image, state.hud.as_ref().filter(|_| hud_drawn).and_then(HudLayer::drawn_on_black), "eye");
    if hud_drawn && let Some(layer) = state.hud.as_mut() {
        let desc = monaka_channel::d3d::texture_desc(&image);
        let rects = (game.eye_rects)(&pose, desc.Width, desc.Height);
        match layer.composite(context, eye, &image, (game.hud_rect)(rects[eye])) {
            Ok(with_hud) => image = with_hud.clone(),
            Err(e) => {
                monaka_producer::log_first(1, format_args!("FSR HUD composite failed: {e}"));
                state.failures += 1;
            }
        }
    }
    publisher.publish_image(device, context, &image, n, head, report)
}

/// A frame with no 3D view, to the viewer's flat screen (see [`publish`]).
#[allow(clippy::too_many_arguments)]
fn publish_flat(
    state: &mut State,
    game: GameFns,
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    back: &ID3D11Texture2D,
    n: u64,
    publisher: &mut AlternatePublisher,
    head: &HeadSource,
    hud_drawn: bool,
) -> Step {
    let (Some(fsr), Some(scaler)) = (state.fsr.as_ref(), state.scaler.as_mut()) else { return Step::Failed };
    let display = fsr.display();
    let mut image = match scaler.run(context, back, display) {
        Ok(image) => image.clone(),
        Err(e) => {
            monaka_producer::log_first(1, format_args!("FSR flat frame failed: {e}"));
            state.failures += 1;
            return Step::Failed;
        }
    };
    (game.probe)(device, context, back, &image, state.hud.as_ref().filter(|_| hud_drawn).and_then(HudLayer::drawn_on_black), "flat");
    if hud_drawn && let Some(layer) = state.hud.as_mut() {
        let desc = monaka_channel::d3d::texture_desc(back);
        match layer.composite(context, 0, &image, (game.hud_rect)(monaka_warp::fit((desc.Width, desc.Height), display))) {
            Ok(with_hud) => image = with_hud.clone(),
            Err(e) => {
                monaka_producer::log_first(1, format_args!("FSR HUD composite failed: {e}"));
                state.failures += 1;
            }
        }
    }
    if state.flat == 0 {
        log!("FSR: a frame with no 3D view (a movie or a loading screen) went to the flat screen");
    }
    state.flat += 1;
    publisher.publish_flat(device, context, &image, n, head)
}

fn summary() -> String {
    let state = STATE.locked();
    let s = &state.0;
    match s.fsr.is_some() {
        true => format!("FSR: upscaled={} busy={} missing_inputs={} flat={} failures={}", s.upscaled, s.busy, s.missing, s.flat, s.failures),
        false => String::new(),
    }
}

/// Drops the upscaler (after the hooks are off; it waits for its GPU work first).
fn release() {
    ENABLED.store(false, Release);
    crate::driver::leave_present(before_present);
    let mut state = STATE.locked();
    let state = &mut state.0;
    state.output = None;
    state.scaler = None;
    state.hud_views = None;
    state.hud = None;
    state.gbuffer = None;
    state.fsr = None;
    state.game = None;
}
