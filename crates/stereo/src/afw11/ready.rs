//! AFW on D3D11 as a ready-made component ([`frame_warping`], the core's shape:
//! [`crate::afw::component`]) on the present loop, for a D3D11 game with frame inputs
//! (`crate::providers::FrameInputs11`): a pair every frame, this frame's real eye and the other eye
//! reprojected from it ([`super::Afw11`]), the HUD a layer over both eyes.
//!
//! The game's eye site records each frame's cameras ([`record_cameras`]); its draws hand over the
//! frame's world image at its scene copy ([`capture`]) and the HUD's draws ([`hud_targets`]). The
//! present loop's step ([`before_present`], joined at install) publishes the pair.

use super::{Afw11, At, EyeCameras};
use crate::afw::{self, Counters, Shot};
use crate::driver::{Driver, Pending};
use crate::feature::Presents;
use crate::providers::FrameInputs11;
use monaka_core::alternate::{PresentRing, Schedule};
use monaka_core::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Every, Lack, Missing, Start};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::*};
use windows::Win32::Graphics::Direct3D11::{ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

/// Frame warping on D3D11 for game `G` (AFW, the stereo method `stereo=afw`): a pair every frame;
/// the HUD a layer over both eyes while it is shown. The game says when it is on ([`afw::on`]) and
/// what it works on top of (`needs`: its eyes) by struct update.
pub const fn frame_warping<G: Presents + FrameInputs11>() -> Component<G> {
    afw::component(install::<G>, release, &EVERY, summary, true)
}

/// What frame warping reads of its game.
#[derive(Clone, Copy)]
struct GameFns {
    scene_depth: fn(u64) -> Option<ID3D11Texture2D>,
    eye_rects: fn(&HeadPose, u32, u32) -> [[f32; 4]; 2],
    presenting: fn() -> Option<Pending>,
}

struct State {
    game: Option<GameFns>,
    afw: Option<Afw11>,
    cameras: PresentRing<EyeCameras>,
}

struct Cell(State);
// SAFETY: the D3D objects inside are used under the mutex by the game's render thread, or by the
// stop after the hooks are off.
unsafe impl Send for Cell {}

static STATE: Mutex<Cell> = Mutex::new(Cell(State { game: None, afw: None, cameras: PresentRing::new() }));
static ENABLED: AtomicBool = AtomicBool::new(false);
static HUD_LAYER: AtomicBool = AtomicBool::new(false);
static CAPTURED: AtomicBool = AtomicBool::new(false);
pub(super) static COUNTERS: Counters = Counters::new();

const EVERY: [Every; 1] = [Every::millis(afw::REPORT_MS, log_report)];

fn install<G: Presents + FrameInputs11>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let hud = start.shared.hud.shown;
    let device = G::device(start.build)?;
    let mut afw = Afw11::new(&device, start.channel()?.clone()).map_err(|e| format!("alternate-eye with depth: {e}")).missing()?;
    if hud {
        afw.enable_hud(&device).map_err(|e| format!("HUD layer: {e}")).missing()?;
    }
    let mut state = STATE.locked();
    state.0.afw = Some(afw);
    state.0.game = Some(GameFns { scene_depth: G::scene_depth, eye_rects: crate::hud11::eye_rects::<G>, presenting: G::presenting });
    drop(state);
    HUD_LAYER.store(hud, Release);
    crate::driver::join_present(before_present);
    ENABLED.store(true, Release);
    log!("alternate-eye with depth: a pair every frame; HUD layer {hud}");
    Ok(())
}

/// Frame warping runs this run.
pub fn enabled() -> bool {
    ENABLED.load(Acquire)
}

/// Whether HUD draws go into the HUD layer (laid over both eyes) instead of the back buffer.
pub fn hud_layer() -> bool {
    enabled() && HUD_LAYER.load(Acquire)
}

/// The black and white HUD targets of this frame (each HUD draw goes into both).
pub fn hud_targets(context: &ID3D11DeviceContext, width: u32, height: u32) -> Option<[ID3D11RenderTargetView; 2]> {
    STATE.lock().ok()?.0.afw.as_mut()?.hud_targets(context, width, height)
}

/// Whether this frame still has to be captured (checked before any work on the hot draw path).
pub fn needs_capture() -> bool {
    enabled() && !CAPTURED.load(Acquire)
}

/// The eye site wrote the cameras of the frame whose eye was chosen after present `chosen_at`.
pub fn record_cameras(chosen_at: u64, cameras: EyeCameras) {
    if let Ok(mut state) = STATE.lock() {
        state.0.cameras.put(chosen_at, cameras);
    }
}

/// Keeps the frame shown at present `n` (its world image is `color` now) as its eye's latest real
/// frame, once per frame: which eye and how it was rendered by the core ([`Shot::scheduled`]).
pub fn capture(context: &ID3D11DeviceContext, color: &ID3D11Texture2D, n: u64, schedule: Schedule) {
    if !enabled() || CAPTURED.swap(true, AcqRel) {
        return;
    }
    let Some(game) = STATE.lock().ok().and_then(|s| s.0.game) else { return };
    let Some(depth) = (game.scene_depth)(n) else { return };
    let Ok(mut state) = STATE.lock() else { return };
    let State { afw, cameras, .. } = &mut state.0;
    let Some(shot) = Shot::scheduled(schedule, n, cameras) else { return };
    if let Some(afw) = afw.as_mut() {
        afw.capture(context, &shot, color, &depth);
    }
    static DIAGNOSED: AtomicBool = AtomicBool::new(false);
    if !DIAGNOSED.swap(true, Relaxed)
        && let Some(cameras) = cameras.get(schedule.chosen_at(n))
    {
        diagnose_depth(context, &depth, &cameras);
    }
}

/// Once per run: the depth buffer's actual values (sky at the top, ground at the bottom) against
/// the projection the mapping was built from, to tell standard from reversed depth.
fn diagnose_depth(context: &ID3D11DeviceContext, depth: &ID3D11Texture2D, cameras: &EyeCameras) {
    let format = monaka_channel::d3d::texture_desc(depth).Format;
    // SAFETY: COM call on the game's live context.
    let Ok(device) = (unsafe { context.GetDevice() }) else { return };
    // A staging copy on the game's render thread (stalls; once per run). Each texel starts with its
    // 32-bit depth (D32S8: 8 bytes, the stencil after it).
    let Ok((w, h, texel, texels)) = monaka_channel::d3d::read_texels(&device, context, depth) else { return };
    if texel < 4 {
        return;
    }
    let value = |x: u32, y: u32| {
        let at = (y * w + x) as usize * texel;
        f32::from_le_bytes([texels[at], texels[at + 1], texels[at + 2], texels[at + 3]])
    };
    let samples = [(w / 2, 2), (w / 2, h / 4), (w / 2, h / 2), (w / 2, 3 * h / 4), (w / 2, h - 3)];
    let values: Vec<String> = samples.iter().map(|&(x, y)| format!("({x},{y})={:.6}", value(x, y))).collect();
    let p = &cameras.projection[0];
    log!(
        "depth diagnosis: format {} values {}; projection P10 {} P11 {} P14 {} P15 {}; mapping a {} b {}",
        format.0,
        values.join(" "),
        p[10],
        p[11],
        p[14],
        p[15],
        cameras.mapping[0].a,
        cameras.mapping[0].b
    );
}

/// The present loop's step (joined at install): the frame not captured at its scene copy (no HUD
/// draws) taken now, then what the core says goes out (the pair, by the rules) published, the HUD
/// layer at its place in each eye (from the latest pose: frame warping publishes from present
/// `latency` on, a present before the schedule calls a frame shown). True: the loop publishes
/// nothing itself.
pub fn before_present(driver: &Driver, chain: &IDXGISwapChain, n: u64) -> bool {
    if !enabled() {
        return false;
    }
    if let Ok((image, device, context)) = monaka_channel::d3d::swapchain_parts(chain) {
        if let Some(schedule) = driver.schedule() {
            capture(&context, &image, n, schedule);
        }
        let size = monaka_channel::d3d::texture_desc(&image);
        let Some(game) = STATE.lock().ok().and_then(|s| s.0.game) else { return true };
        let rects = (game.eye_rects)(&driver.pending_pose(), size.Width, size.Height);
        let at = At { head: driver.head(), hud_rects: rects, flat: driver.flat().is_some(), label: (game.presenting)(), present: n };
        CAPTURED.store(false, Release);
        if let Ok(mut state) = STATE.lock()
            && let Some(afw) = state.0.afw.as_mut()
        {
            afw.publish(&device, &context, &image, &at, |pose| driver.report(pose));
        }
    }
    true
}

fn log_report(_: f64) {
    log!("{}", report());
}

/// The periodic report's line (the core's, [`afw::report_line`]): no depth goes with the pairs and
/// no frame generation runs on D3D11.
pub fn report() -> String {
    let (counts, captured) = STATE.lock().ok().and_then(|s| s.0.afw.as_ref().map(Afw11::counts)).unwrap_or_default();
    afw::report_line(captured, counts, &COUNTERS, 0, None)
}

fn summary() -> String {
    let published = STATE.locked().0.afw.as_ref().map(Afw11::published);
    match published {
        Some((pairs, failures)) => format!("{}; alternate-eye with depth: pairs={pairs} failures={failures}", report()),
        None => String::new(),
    }
}

fn release() {
    ENABLED.store(false, Release);
    crate::driver::leave_present(before_present);
    let mut state = STATE.locked();
    state.0.afw = None;
    state.0.game = None;
}
