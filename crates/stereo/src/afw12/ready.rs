//! The ready-made components ([`frame_warping`], AFW alone; [`frame_generation`], frame generation;
//! [`frame_warping_pairs`], on the same-frame loop): what each game supplies (its frame labels,
//! inputs and queue, read once at the install), whether frame warping and frame generation run, and
//! their counts.

use super::*;
use monaka_core::manifest::{DOMINANT_EYE, Manifest};
use monaka_core::options::Options;
use std::sync::atomic::AtomicU8;

/// The present loop frame warping runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OnLoop {
    /// The alternate-eye loop (`crate::feature::DRIVER`): its pairs go out on a publisher of its own.
    Alternate,
    /// The same-frame loop (`crate::same_frame::PAIRS`), fed one eye per frame: its pairs go out
    /// through the loop's pair sink.
    SameFrame,
}

/// What frame warping reads of its game, from the game's providers and present loop.
#[derive(Clone, Copy)]
pub(super) struct GameFns {
    pub(super) on: OnLoop,
    pub(super) presenting: fn() -> Option<Pending>,
    pub(super) frustum: fn(&HeadPose, usize) -> Option<Frustum>,
    pub(super) separation: fn(&HeadPose) -> Option<f32>,
    /// Each eye's pose as it goes out, as the loop reports it.
    pub(super) report: fn(&mut HeadPose),
    pub(super) queue: fn(&IDXGISwapChain3) -> Option<ID3D12CommandQueue>,
    pub(super) units_per_metre: f32,
}

impl GameFns {
    /// The loop's head source.
    pub(super) fn head(&self) -> &'static HeadSource {
        match self.on {
            OnLoop::Alternate => DRIVER.head(),
            OnLoop::SameFrame => crate::same_frame::PAIRS.head(),
        }
    }

    /// The number of the present about to show the frame being warped: the alternate loop counts
    /// it at its present, the same-frame loop before the frame reaches the warp.
    pub(super) fn present_warped(&self) -> u64 {
        match self.on {
            OnLoop::Alternate => DRIVER.presents() + 1,
            OnLoop::SameFrame => crate::same_frame::PAIRS.presents(),
        }
    }
}

/// The alternate-eye loop's pose report for game `G` (its own, or the centred frustum around each
/// eye's).
pub(super) fn alternate_report<G: Presents>(pose: &mut HeadPose) {
    match G::LOOP.report {
        Some(report) => report(&DRIVER, pose),
        None => DRIVER.report_centred(pose),
    }
}

/// The same-frame loop's pose report for game `G` (its own, or as drawn).
pub(super) fn same_frame_report<G: PresentsPairs>(pose: &mut HeadPose) {
    if let Some(report) = G::LOOP.report {
        report(pose);
    }
}

/// The same-frame loop's game queue for game `G`.
pub(super) fn same_frame_queue<G: PresentsPairs>(_: &IDXGISwapChain3) -> Option<ID3D12CommandQueue> {
    G::LOOP.queue.and_then(|queue| queue())
}

pub(super) static GAME: Mutex<Option<GameFns>> = Mutex::new(None);
/// The rules this run warps by (the base ones with the install's choices).
pub(super) static RULES_CHOSEN: Mutex<Rules> = Mutex::new(RULES);
/// The dominant eye (0 left, 1 right): rendered every present.
static DOMINANT: AtomicU8 = AtomicU8::new(1);
/// Every this many presents the other eye renders instead, for its gap fill.
pub const REFRESH: u64 = 8;

/// The eye present `n`'s frame draws: the dominant eye, the other every [`REFRESH`]th present.
pub fn dominant_eye(n: u64) -> usize {
    let dominant = DOMINANT.load(Relaxed) as usize;
    if n % REFRESH == REFRESH - 1 { 1 - dominant } else { dominant }
}
/// Frame warping runs this run, and with frame generation's in-between frames.
pub(super) static ON: AtomicBool = AtomicBool::new(false);
pub(super) static FRAMEGEN: AtomicBool = AtomicBool::new(false);
pub(super) static CHANNEL: Mutex<Option<ChannelName>> = Mutex::new(None);

pub(super) static COUNTERS: Counters = Counters::new();

pub(super) const EVERY: [Every; 1] = [Every::millis(afw::REPORT_MS, log_report)];

/// Frame warping for game `G` (AFW alone): each frame warped into the other eye, a pair of one
/// moment at every present (the core's component, [`afw::component`]). The game says when it is
/// on ([`afw::on`]) and what it works on top of (`needs`: its eyes) by struct update; its pairs go
/// out at the present loop's step ([`before_present`], joined at its install).
pub const fn frame_warping<G: Presents + FrameInputs + HudLayer>() -> Component<G> {
    Component { options: &[DOMINANT_EYE], settings, ..afw::component(install::<G>, close, &EVERY, report, true) }
}

/// The component's settings: the dominant eye (0 left, 1 right), Monaka's own.
fn settings(options: &Options, _: &Manifest) {
    DOMINANT.store(options.choice("dominant_eye", &[("left", 0u8), ("right", 1u8)], 1), Relaxed);
}

/// Frame warping for game `G` on the same-frame loop (`crate::same_frame`) fed one eye per frame:
/// each frame warped into the other eye, the pair going out through the loop's pair sink. The loop
/// hands each frame over at its present ([`pair_present`]).
pub const fn frame_warping_pairs<G: PresentsPairs + EyeSite + FrameInputs + HudLayer>() -> Component<G> {
    afw::component(install_pairs::<G>, close, &EVERY, report, false)
}

/// Frame generation, on top of [`frame_warping`] (or [`frame_warping_pairs`]): FSR frame generation
/// (`monaka_framegen::feature`, on before it) doubling the pairs' rate. The game says when it is on
/// and lists it after both (`needs`).
pub const fn frame_generation<G: FrameInputs + HudLayer>() -> Component<G> {
    Component { name: "in-between frames", install: install_framegen::<G>, release: || FRAMEGEN.store(false, Release), ..Component::NONE }
}

pub(super) fn install<G: Presents + FrameInputs + HudLayer>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    *CHANNEL.locked() = Some(start.channel()?.clone());
    let game = GameFns {
        on: OnLoop::Alternate,
        presenting: G::presenting,
        frustum: G::frustum,
        separation: G::separation,
        report: alternate_report::<G>,
        queue: G::LOOP.queue.unwrap_or(crate::queue12::game_queue),
        units_per_metre: G::UNITS_PER_METRE,
    };
    let options = start.options;
    settings(options, start.manifest);
    let alternate = options.switch("afw_alternate", false);
    let fill_first = options.switch("afw_fill_first", true);
    *RULES_CHOSEN.locked() = if alternate { RULES.fill_first(fill_first) } else { RULES.every(REFRESH).fill_first(fill_first) };
    start_warping(game);
    DRIVER.choose_eyes((!alternate).then_some(dominant_eye as crate::driver::EyeSchedule));
    log!(
        "frame warping: {}{}",
        if alternate { "the eyes take turns".to_owned() } else { format!("the {} eye renders every present, the other every {REFRESH}th", ["left", "right"][DOMINANT.load(Relaxed) as usize]) },
        if fill_first { "; the eye's own last frame first where it shows the same surface" } else { "" }
    );
    crate::driver::join_present(present_step);
    Ok(())
}

/// The alternate-eye loop's step for the pairs frame warping publishes itself, while it runs.
pub(super) fn present_step(driver: &Driver, chain: &IDXGISwapChain, _: u64) -> bool {
    enabled() && before_present(driver, chain)
}

pub(super) fn install_pairs<G: PresentsPairs + EyeSite + FrameInputs + HudLayer>(_: &mut Start<'_, G>) -> Result<(), Lack> {
    let game = GameFns {
        on: OnLoop::SameFrame,
        presenting: G::presenting,
        frustum: <G as crate::providers::FrameLabels>::frustum,
        separation: <G as crate::providers::FrameLabels>::separation,
        report: same_frame_report::<G>,
        queue: same_frame_queue::<G>,
        units_per_metre: G::UNITS_PER_METRE,
    };
    start_warping(game);
    Ok(())
}

/// Frame warping for `game` from now on: its inputs asked for, its HUD joined.
pub(super) fn start_warping(game: GameFns) {
    *GAME.locked() = Some(game);
    inputs::want(Motion::None, Some(warpable));
    crate::providers::join_hud(warp, lay_hud);
    ON.store(true, Release);
    log!("frame warping: each frame warped into the other eye");
}

/// Frame warping runs on the same-frame loop this run.
pub fn on_same_frame() -> bool {
    enabled() && GAME.locked().is_some_and(|game| game.on == OnLoop::SameFrame)
}

pub(super) fn install_framegen<G: FrameInputs + HudLayer>(_: &mut Start<'_, G>) -> Result<(), Lack> {
    // Frame generation's motion vectors: the game's, as it made them (rebased per eye at the
    // present, from the cameras).
    inputs::want(Motion::Float, None);
    FRAMEGEN.store(true, Release);
    log!("frame warping: in-between frames added");
    Ok(())
}

pub fn enabled() -> bool {
    ON.load(Acquire)
}

/// Frame generation runs: chosen, and frame generation loaded.
pub(super) fn framegen_running() -> bool {
    framegen_chosen() && monaka_framegen::enabled()
}

/// Frame generation chosen for this run, with frame warping on.
pub(super) fn framegen_chosen() -> bool {
    FRAMEGEN.load(Acquire) && enabled()
}

/// Only frames whose camera can be warped from are taken.
pub(super) fn warpable(rendering: &Rendering) -> bool {
    let warpable = Rendered::from_rendering(rendering).is_some();
    if !warpable {
        log_first!(3, "frame warping: a frame's camera is not one to warp from (near {}, view to clip {:?})", rendering.camera.near, rendering.view_to_clip);
    }
    warpable
}
