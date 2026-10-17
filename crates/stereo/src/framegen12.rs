//! Per-eye FSR frame generation for alternate-eye stereo on D3D12, as a ready-made component
//! ([`per_eye`]) on the present loop, for a game with frame labels and frame inputs
//! ([`crate::providers`]).
//!
//! Alternate-eye renders one eye per frame: left at frames 0, 2, 4, right at 1, 3, 5. Each eye is
//! its own stream of frames two apart, so each gets its own frame-generation context. At an eye's
//! present the frame halfway between its previous and current frames is generated: that is the
//! moment of the other eye's newest real frame, so each present publishes one pair of a single
//! moment (this eye generated, the other real), each eye with its own head pose. Each eye then
//! changes every frame instead of every other one, both eyes in step, for one frame of latency.
//!
//! Its inputs: each eye's depth, camera and motion vectors carried onto that eye's own previous
//! frame two frames back by chaining the game's own over the other eye's frame between
//! (`monaka_framegen::inputs`, chained). The camera rebase per-eye DLSS uses is not used here: it
//! takes everything as still world, so it moves the weapon and arms, which move with the head, by
//! the eyes' parallax, and they ghost. With per-eye upscaling on (`crate::upscaler`) the inputs
//! stay the game's own and are chained here as without it; a game that renders each eye against its
//! own previous frame (its provider says so, `inputs::PreviousFrame::OwnEye`) gives vectors that
//! already reach it, and the inputs pass them as they are. The HUD goes through the interpolation.
//!
//! Each pair carries both eyes' depth for the headset while it goes there ([`crate::depth_layer`]).
//!
//! The same for same-frame stereo is [`pairs`] (its component [`pairs::per_eye_pairs`]): each eye's
//! frame halfway between its last two, a generated pair between the real ones.

pub mod pairs;

use crate::afw::Rendered;
use crate::driver::Driver;
use crate::feature::Presents;
use crate::pairs::d3d12::{EyeDepth12, PairDepth, PairPublisher12, Step};
use crate::providers::FrameInputs;
use monaka_channel::ChannelName;
use monaka_channel::d3d12::{self, Recorder};
use monaka_channel::pose::HeadSource;
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_framegen::inputs::{self, Motion};
use monaka_framegen::{EyeStream, FrameInputs as Generated, SLOTS, fitting_stream, skip_eye as skip};
use monaka_producer::game::{Component, Every, Lack, Start};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, IDXGISwapChain3};
use windows::core::Interface;

/// What per-eye frame generation reads of its game.
#[derive(Clone, Copy)]
struct GameFns {
    units_per_metre: f32,
}

static GAME: Mutex<Option<GameFns>> = Mutex::new(None);
static ON: AtomicBool = AtomicBool::new(false);
static CHANNEL: Mutex<Option<ChannelName>> = Mutex::new(None);

struct Counters {
    generated: AtomicU64,
    real_pairs: AtomicU64,
    unpaired: AtomicU64,
    /// Frames no eye camera drew, sent flat to both eyes.
    flat: AtomicU64,
}

static COUNTERS: Counters = Counters { generated: AtomicU64::new(0), real_pairs: AtomicU64::new(0), unpaired: AtomicU64::new(0), flat: AtomicU64::new(0) };

const EVERY: [Every; 1] = [Every::millis(2000, log_report)];

/// Per-eye frame generation for game `G`. FSR frame generation is loaded by
/// `monaka_framegen::feature::framegen()`, which this `needs`; the game says when it is on (`on`) by
/// struct update; its pairs go out at the present loop's step ([`before_present`], joined at its
/// install).
pub const fn per_eye<G: Presents + FrameInputs>() -> Component<G> {
    Component { name: "frame generation per eye", needs_channel: true, install: install::<G>, every: &EVERY, release: close, ..Component::NONE }
}

fn install<G: Presents + FrameInputs>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    *CHANNEL.locked() = Some(start.channel()?.clone());
    *GAME.locked() = Some(GameFns { units_per_metre: G::UNITS_PER_METRE });
    inputs::want(Motion::Chained, None);
    ON.store(true, Release);
    crate::driver::join_present(present_step);
    Ok(())
}

/// The present loop's step for the pairs frame generation publishes itself, while it runs.
fn present_step(driver: &Driver, chain: &IDXGISwapChain, _: u64) -> bool {
    enabled() && before_present(driver, chain)
}

/// Per-eye frame generation runs this run.
pub fn enabled() -> bool {
    ON.load(Acquire)
}

// --- At each present, on the present thread ---------------------------------------------------------

struct State {
    recorder: Recorder,
    publisher: PairPublisher12,
    streams: [Option<EyeStream>; 2],
    real: [[Option<ID3D12Resource>; SLOTS]; 2],
    frames: [usize; 2],
    /// Each eye's newest real frame and its pose.
    last: [Option<(ID3D12Resource, HeadPose)>; 2],
    last_eye: Option<usize>,
    /// When a labelled frame was last presented.
    last_labelled: Option<Instant>,
    /// Each eye's depth copy came with its newest real frame (for the headset).
    fresh_depth: [bool; 2],
}

// SAFETY: the D3D objects are free-threaded; everything is used under the state's lock only.
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Generates eye `eye`'s frame halfway between its previous one and `real` (its current one) on
/// `list`. True when the result is usable.
fn generate(state: &mut State, list: &ID3D12GraphicsCommandList, device: &ID3D12Device, eye: usize, real: &ID3D12Resource, frame: &D3D12_RESOURCE_DESC, sequence_broken: bool) -> bool {
    let Some(taken) = inputs::take(eye) else { return skip("no inputs yet") };
    let Some(generated) = Generated::fresh(taken.fresh.both(), taken.depth.as_ref(), taken.motion.as_ref(), taken.camera(), state.streams[eye].as_mut()) else { return false };
    let Generated { depth, motion, camera, render_size } = generated;
    let stream = match fitting_stream(&mut state.streams[eye], &state.recorder, |s| s.fits(frame, render_size), || EyeStream::new(device, frame, render_size)) {
        Ok(stream) => stream,
        Err(why) => return skip(&format!("no context: {why}")),
    };
    match stream.generate(list, &camera, &depth, &motion, None, real, taken.reset || sequence_broken, 33.3) {
        Ok(true) => true,
        Ok(false) => skip("history reset"),
        Err(why) => skip(&why),
    }
}

/// The state, made on first use (command lists on the game queue's device, the channel's publisher).
fn state_for<'a>(guard: &'a mut Option<State>, queue: &ID3D12CommandQueue) -> Option<&'a mut State> {
    if guard.is_none() {
        let channel = CHANNEL.locked().clone()?;
        let device = d3d12::device_of(queue).ok()?;
        match Recorder::new(&device, 6) {
            Ok(recorder) => {
                *guard = Some(State {
                    recorder,
                    publisher: PairPublisher12::new(channel),
                    streams: [None, None],
                    real: Default::default(),
                    frames: [0; 2],
                    last: [None, None],
                    last_eye: None,
                    last_labelled: None,
                    fresh_depth: [false; 2],
                })
            }
            Err(e) => {
                log!("frame generation: no command lists ({e})");
                return None;
            }
        }
    }
    guard.as_mut()
}

/// The present loop's step while [`enabled`] ([`present_step`]): the frame about to present
/// (`chain`'s back buffer) kept, this eye's frame of the other eye's newest moment generated, and
/// that pair published; a frame the loop shows flat (a menu) or an unlabelled one (the pause screen)
/// to both eyes. True: the loop publishes nothing itself.
fn before_present(driver: &Driver, chain: &IDXGISwapChain) -> bool {
    let Some(game) = *GAME.locked() else { return true };
    let Ok(swap) = chain.cast::<IDXGISwapChain3>() else { return true };
    let Some(queue) = crate::queue12::game_queue(Some(&swap)) else { return true };
    let report = |pose: &mut HeadPose| driver.report(pose);
    let flat = driver.flat().is_some();
    match driver.take_label() {
        Some(label) if !flat => present(&swap, &queue, label.eye, label.pose, driver.head(), report, game.units_per_metre),
        _ => present_unlabelled(&swap, &queue, flat, driver.head(), report),
    };
    true
}

/// Before the game presents eye `eye`'s frame (rendered from `pose`): keeps it, generates this eye's
/// frame of the other eye's newest moment, and publishes that pair.
fn present(swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue, eye: usize, pose: HeadPose, head: &HeadSource, report: impl Fn(&mut HeadPose), units_per_metre: f32) -> Step {
    let eye = eye.min(1);
    // SAFETY: COM calls on the game's live swapchain, on the thread that presents it.
    let Ok(buffer) = (unsafe { swap.GetBuffer::<ID3D12Resource>(swap.GetCurrentBackBufferIndex()) }) else { return Step::Failed };
    let Ok(device) = d3d12::device_of(queue) else { return Step::Failed };
    let mut guard = STATE.locked();
    let Some(state) = state_for(&mut guard, queue) else { return Step::Failed };
    if !state.publisher.ready_for(&buffer, queue) {
        return Step::Failed;
    }
    // SAFETY: reads a descriptor.
    let frame = unsafe { buffer.GetDesc() };
    let slot = state.frames[eye] % SLOTS;
    let Some(real) = d3d12::fitting(&mut state.real[eye][slot], &device, frame.Width, frame.Height, frame.Format, D3D12_RESOURCE_FLAG_NONE) else { return Step::Failed };
    let Ok(Some(list)) = state.recorder.begin() else {
        skip("command lists busy");
        return Step::Failed;
    };
    d3d12::copy_whole(&list, &real, &buffer, D3D12_RESOURCE_STATE_PRESENT);
    // Whether this frame brought its depth (generating takes the inputs).
    let fresh = inputs::fresh(eye).both();
    // The same eye twice in a row: frames went missing, this eye's history no longer runs.
    let sequence_broken = state.last_eye == Some(eye);
    let generated = generate(state, &list, &device, eye, &real, &frame, sequence_broken);
    state.fresh_depth[eye] = fresh;
    if let Err(e) = state.recorder.submit(queue) {
        log!("frame generation: submit failed: {e}");
        return Step::Failed;
    }
    let other = 1 - eye;
    let previous = state.last[eye].as_ref().map(|(_, p)| *p);
    let depth = pair_depth(state, eye, units_per_metre);
    let step = match (&state.last[other], previous) {
        // This eye's frame of the other eye's moment beside that eye's real frame.
        (Some((other_frame, other_pose)), Some(previous)) if generated => {
            let output = state.streams[eye].as_ref().map(|s| s.output.clone()).expect("generated");
            let halfway = monaka_framegen::blend(&previous, &pose, 0.5);
            let (images, poses) = order(eye, (&output, halfway), (other_frame, *other_pose));
            COUNTERS.generated.fetch_add(1, Relaxed);
            state.publisher.publish_with_depth(images, depth.as_ref(), poses, head, &report)
        }
        // Not generated: the pair of the two newest real frames, a frame apart (plain alternate-eye).
        (Some((other_frame, other_pose)), _) => {
            let (images, poses) = order(eye, (&real, pose), (other_frame, *other_pose));
            COUNTERS.real_pairs.fetch_add(1, Relaxed);
            state.publisher.publish_with_depth(images, depth.as_ref(), poses, head, &report)
        }
        (None, _) => {
            COUNTERS.unpaired.fetch_add(1, Relaxed);
            Step::Left
        }
    };
    state.last[eye] = Some((real, pose));
    state.frames[eye] += 1;
    state.last_eye = Some(eye);
    state.last_labelled = Some(Instant::now());
    step
}

/// A frame no eye camera drew goes to both eyes, and the eyes' histories start over after it: with a
/// reason to go flat (`flat`: a menu, the present loop's `Driver::flat`) to the viewer's flat screen;
/// otherwise (the pause screen, loading: the player camera is not updated) with the newest head pose.
fn present_unlabelled(swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue, flat: bool, head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
    // SAFETY: COM calls on the game's live swapchain, on the thread that presents it.
    let Ok(buffer) = (unsafe { swap.GetBuffer::<ID3D12Resource>(swap.GetCurrentBackBufferIndex()) }) else { return Step::Failed };
    let mut guard = STATE.locked();
    let Some(state) = state_for(&mut guard, queue) else { return Step::Failed };
    // A lone unlabelled frame in play is skipped (a flat frame would flash); a menu has none for long.
    if !flat && state.last_labelled.is_some_and(|t| t.elapsed().as_millis() < 200) {
        return Step::Waiting;
    }
    if !state.publisher.ready_for(&buffer, queue) {
        return Step::Failed;
    }
    (state.last, state.last_eye, state.fresh_depth) = ([None, None], None, [false; 2]);
    state.streams.iter_mut().flatten().for_each(|s| s.primed = false);
    COUNTERS.flat.fetch_add(1, Relaxed);
    if flat {
        return state.publisher.publish_flat([(&buffer, D3D12_RESOURCE_STATE_PRESENT); 2], head);
    }
    let pose = head.current().unwrap_or_default();
    state.publisher.publish([(&buffer, D3D12_RESOURCE_STATE_PRESENT); 2], [pose, pose], head, &report)
}

/// Both eyes' depth for the headset (while it goes there) with this present's pair, eye `eye` the
/// one presenting: each eye's copy from its newest real frame, when that frame brought one. A
/// generated eye lies halfway between two of its eye's real frames and gets the newer one's depth
/// (the frame just copied); the other eye's is its real frame's own.
fn pair_depth(state: &State, eye: usize, units_per_metre: f32) -> Option<PairDepth> {
    if !crate::depth_layer::on() || state.fresh_depth != [true; 2] {
        return None;
    }
    let eyes = [inputs::peek(0)?, inputs::peek(1)?];
    let eye_depth = |e: usize| Some(EyeDepth12 { texture: eyes[e].depth.clone()?, state: D3D12_RESOURCE_STATE_COMMON, rect: eyes[e].rect });
    let params = Rendered::from_rendering(eyes[eye].rendering.as_ref()?)?.depth_params(units_per_metre)?;
    Some(PairDepth { eyes: [eye_depth(0)?, eye_depth(1)?], params })
}

/// (left, right) images in the common state and their poses, from this eye's and the other's.
fn order<'a>(eye: usize, this: (&'a ID3D12Resource, HeadPose), other: (&'a ID3D12Resource, HeadPose)) -> ([(&'a ID3D12Resource, D3D12_RESOURCE_STATES); 2], [HeadPose; 2]) {
    let (left, right) = if eye == 0 { (this, other) } else { (other, this) };
    ([(left.0, D3D12_RESOURCE_STATE_COMMON), (right.0, D3D12_RESOURCE_STATE_COMMON)], [left.1, right.1])
}

/// Frees everything once the GPU is done with it (after the hooks are out).
pub fn close() {
    ON.store(false, Release);
    crate::driver::leave_present(present_step);
    let Some(mut state) = STATE.locked().take() else { return };
    log!("frame generation: {} pairs published, {} failures", state.publisher.pairs(), state.publisher.failures());
    state.publisher.close();
    monaka_framegen::free_when_idle(state, |s| &s.recorder);
    inputs::clear();
}

fn log_report(_: f64) {
    log!("{}", report());
}

/// The periodic report's line.
pub fn report() -> String {
    let collected = inputs::copies();
    format!(
        "frame generation: inputs L/R {}/{}, pairs with a generated eye {}, real pairs {}, eyes not generated {}, unpaired {}, flat (menus) {}, pairs with depth {}",
        collected[0],
        collected[1],
        COUNTERS.generated.load(Relaxed),
        COUNTERS.real_pairs.load(Relaxed),
        monaka_framegen::eyes_skipped(),
        COUNTERS.unpaired.load(Relaxed),
        COUNTERS.flat.load(Relaxed),
        STATE.lock().ok().and_then(|s| s.as_ref().map(|s| s.publisher.pairs_with_depth())).unwrap_or(0)
    )
}
