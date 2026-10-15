//! Per-eye FSR frame generation for same-frame stereo on D3D12, as a ready-made component
//! ([`per_eye_pairs`]) on the same-frame loop (`crate::same_frame`).
//!
//! Each eye is its own stream of rendered frames with its own depth, motion vectors and camera
//! (with each eye's own temporal history the game's motion vectors are relative to that eye's
//! previous frame), so each eye gets its own frame-generation context ([`EyeStream`]). At an eye's
//! present the frame is copied into a ring and the frame halfway between it and that eye's previous
//! frame is generated. After the right eye, the generated pair goes out at once with the head pose
//! halfway between the two real pairs' poses, and the real pair half a frame later
//! (`monaka_framegen::pacing`), so the headset gets twice the rendered rate, evenly spaced; both go
//! through the loop's pair sink (`crate::same_frame::d3d12::sink`).
//!
//! Each eye's depth, motion vectors and camera are the frame inputs (`monaka_framegen::inputs`, the
//! frame-generation call's copied at the eye's present).
//!
//! The HUD is kept out of the interpolation: a game that tags each eye's UI layer gives it here
//! ([`tagged_ui`]), so the world is recovered from the frame and that layer, interpolated, and the
//! newest real frame's layer is laid over the generated frame (`monaka_framegen::hud12`). With the
//! loop's HUD layer (`crate::same_frame::PairHud`) the layer kept out of the frame is laid over the
//! real and the generated frames instead.

use crate::same_frame::d3d12::sink;
use crate::same_frame::{EyeHud, PairHud, PresentsPairs};
use crate::providers::FrameInputs;
use monaka_channel::d3d12::{self, Recorder};
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_ffx::Size2;
use monaka_framegen::hud12::HudComposer;
use monaka_framegen::inputs::{self, Motion};
use monaka_framegen::pacing::{Interval, Pacing};
use monaka_framegen::{EyeStream, FrameInputs as Generated, SLOTS, fitting_stream, skip_eye as skip};
use monaka_producer::game::{Component, Lack, Start};
use monaka_producer::log;
use monaka_warp::warp12::HudOverlay;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R16G16_FLOAT, DXGI_FORMAT_R16G16_TYPELESS};

/// No per-eye history epoch: generation starts afresh only when the HUD layer comes or goes.
fn no_epoch() -> u32 {
    0
}

static ON: AtomicBool = AtomicBool::new(false);
/// The game's per-eye history epoch: when it changes, each eye's generation starts afresh.
static EPOCH: Mutex<fn() -> u32> = Mutex::new(no_epoch);

pub static GENERATED: AtomicU64 = AtomicU64::new(0);
pub static REAL: AtomicU64 = AtomicU64::new(0);
static UI_TAGS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static HUD_FREE: AtomicU64 = AtomicU64::new(0);
/// Pairs whose frames got the HUD layer laid over.
static HUD_LAID: AtomicU64 = AtomicU64::new(0);

/// Per-eye frame generation of same-frame pairs for game `G`. FSR frame generation is loaded by
/// `monaka_framegen::feature::framegen()`, which this `needs`; the game says when it is on (`on`) by
/// struct update, and its history epoch ([`set_epoch`]) at its install.
pub const fn per_eye_pairs<G: PresentsPairs + FrameInputs>() -> Component<G> {
    Component { name: "frame generation per eye", install: install::<G>, release: close, ..Component::NONE }
}

fn install<G: PresentsPairs + FrameInputs>(_: &mut Start<'_, G>) -> Result<(), Lack> {
    // The game's depth and motion vectors as it tags them (the motion vectors in their own format).
    inputs::want(Motion::Float, None);
    ON.store(true, Release);
    Ok(())
}

/// The game's per-eye history epoch for this run.
pub fn set_epoch(epoch: fn() -> u32) {
    *EPOCH.locked() = epoch;
}

/// Frame generation of pairs runs: the component is on and FSR frame generation is loaded.
pub fn enabled() -> bool {
    ON.load(Acquire) && monaka_framegen::enabled()
}

/// One eye's UI layer (tagged by the game in play) as the game tagged it: complete only by the
/// eye's present ("valid until present"), so it is copied there; and its copy.
#[derive(Default)]
struct Ui {
    tagged: Option<(ID3D12Resource, D3D12_RESOURCE_STATES)>,
    copy: Option<ID3D12Resource>,
    /// Tagged since the eye's last present.
    fresh: bool,
}

static UI: Mutex<[Option<Ui>; 2]> = Mutex::new([None, None]);

/// The game tagged the UI layer of eye `eye` (rendering now): kept to be copied at the eye's
/// present.
pub fn tagged_ui(eye: usize, source: &ID3D12Resource, state: D3D12_RESOURCE_STATES) {
    let eye = eye.min(1);
    let mut ui = UI.locked();
    let ui = ui[eye].get_or_insert_with(Ui::default);
    ui.tagged = Some((source.clone(), state));
    ui.fresh = true;
    UI_TAGS[eye].fetch_add(1, Relaxed);
}

/// One eye's generator and output ([`EyeStream`]), with the world it interpolates when the HUD is
/// kept out.
struct Stream {
    eye: EyeStream,
    /// The world without its HUD, interpolated in place of the frame: a ring like the frame copies,
    /// because frame generation reads the previous frame from its own texture (with one texture
    /// rewritten each frame, the "generated" frame is just the newest one).
    world: [ID3D12Resource; SLOTS],
    /// The ring slot written last.
    world_slot: usize,
    epoch: u32,
    /// What the history holds: frames without their HUD (true) or finished frames.
    hud_free: bool,
}

struct State {
    recorder: Recorder,
    streams: [Option<Stream>; 2],
    real: [[Option<ID3D12Resource>; 2]; SLOTS],
    /// The real frames with the HUD layer laid over (published in place of `real` when made).
    shown: [[Option<ID3D12Resource>; 2]; SLOTS],
    /// Each eye's generated frame with the HUD layer laid over.
    generated_shown: [Option<ID3D12Resource>; 2],
    /// Whether this pair's frames got the HUD laid over, per eye.
    hud_laid: [bool; 2],
    /// Lays the HUD layer over frames (made on first use; none if the GPU cannot).
    overlay: Option<Result<HudOverlay, ()>>,
    slot: usize,
    /// Each eye's frame this pair was generated.
    generated: [bool; 2],
    previous_pose: Option<HeadPose>,
    interval: Interval,
    /// Lays the UI layer over generated frames (made on first use; none if the GPU cannot).
    composer: Option<Result<HudComposer, ()>>,
}

// SAFETY: the D3D objects are free-threaded; the contexts are used under the state's lock only.
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// A real pair waiting for its time.
struct Due {
    eyes: [ID3D12Resource; 2],
    pose: HeadPose,
}

static PACING: Pacing<Due> = Pacing::new("Monaka VR frame pacing", publish_real);

fn publish_pair(eyes: &[ID3D12Resource; 2], pose: HeadPose) {
    const COMMON: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COMMON;
    sink([(&eyes[0], COMMON), (&eyes[1], COMMON)], Some([pose, pose]), None);
}

fn publish_real(pair: Due) {
    publish_pair(&pair.eyes, pair.pose);
    REAL.fetch_add(1, Relaxed);
}

/// The texture in `slot` with `format` and `source`'s size, made (again) if it does not fit.
fn fitting(slot: &mut Option<ID3D12Resource>, device: &ID3D12Device, source: &D3D12_RESOURCE_DESC, format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS) -> Option<ID3D12Resource> {
    d3d12::fitting(slot, device, source.Width, source.Height, format, flags)
}

fn create_stream(device: &ID3D12Device, frame: &D3D12_RESOURCE_DESC, render_size: Size2) -> Result<Stream, String> {
    let eye = EyeStream::new(device, frame, render_size)?;
    let make = || d3d12::texture(device, frame.Width as u32, frame.Height, frame.Format, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).map_err(|e| e.to_string());
    let world = [make()?, make()?, make()?];
    Ok(Stream { eye, world, world_slot: 0, epoch: 0, hud_free: false })
}

/// At the same-frame loop's present of eye `eye` (0 left; `buffer` the back buffer, in PRESENT,
/// drawn from `pose`): the frame to its generator, the HUD laid over as `hud` gives it. After the
/// right eye, the generated pair now and the real pair half a frame later. Whether the eye was
/// handled.
pub fn present(queue: &ID3D12CommandQueue, buffer: &ID3D12Resource, eye: usize, pose: HeadPose, hud: Option<PairHud>) -> bool {
    let taken = hud.and_then(|hud| (hud.take)(queue, buffer, eye));
    let handled = match eye {
        0 => left(queue, buffer, taken.as_ref()),
        _ => right(queue, buffer, pose, taken.as_ref()),
    };
    if let Some(hud) = hud {
        (hud.done)(queue, taken.as_ref());
    }
    handled
}

/// At eye `index`'s (0 or 1) present: keep the frame and generate the one before it. True when the
/// generated frame is usable (the eye's previous frame was real and continuous).
fn eye_present(state: &mut State, queue: &ID3D12CommandQueue, index: usize, buffer: &ID3D12Resource, hud: Option<&EyeHud>) -> bool {
    state.hud_laid[index] = false;
    // SAFETY: reads a descriptor.
    let frame = unsafe { buffer.GetDesc() };
    let Ok(device) = d3d12::device_of(queue) else { return skip("no device") };
    let Some(real) = fitting(&mut state.real[state.slot][index], &device, &frame, frame.Format, D3D12_RESOURCE_FLAG_NONE) else { return skip("no frame copy") };
    if hud.is_some() && state.overlay.is_none() {
        state.overlay = Some(HudOverlay::new(&device).map_err(|why| log!("HUD lay-over in frame generation unavailable ({why})")));
    }
    let overlay_ready = matches!(state.overlay, Some(Ok(_)));
    let hud = hud.filter(|_| overlay_ready);
    // The frame-generation call's depth and motion vectors: copied now (`late`, below), on the queue
    // after the frame's own work.
    let Some((taken, late)) = inputs::take_late(index) else { return skip("no inputs yet") };
    // This frame's UI layer: copied now, on the queue after the frame's own work.
    let hud_free = {
        let mut ui = UI.locked();
        let ui = ui[index].get_or_insert_with(Ui::default);
        let tagged = match ui.tagged.take() {
            Some(tagged) if ui.fresh => {
                // SAFETY: reads a descriptor.
                let d = unsafe { tagged.0.GetDesc() };
                // Typed like the back buffer (the game's is typeless).
                let format = if d.Format == DXGI_FORMAT_R8G8B8A8_TYPELESS { DXGI_FORMAT_R8G8B8A8_UNORM } else { d.Format };
                fitting(&mut ui.copy, &device, &d, format, D3D12_RESOURCE_FLAG_NONE).map(|copy| (tagged, copy))
            }
            _ => None,
        };
        ui.fresh = false;
        tagged
    };
    let stream = state.streams[index].as_mut().map(|s| &mut s.eye);
    let Some(inputs) = Generated::fresh(taken.fresh.both(), taken.depth.as_ref(), taken.motion.as_ref(), taken.camera(), stream) else { return false };
    let Generated { depth, motion, camera, render_size } = inputs;
    let stream = match fitting_stream(&mut state.streams[index], &state.recorder, |s| s.eye.fits(&frame, render_size), || create_stream(&device, &frame, render_size)) {
        Ok(stream) => stream,
        Err(why) => {
            log!("frame generation unavailable: {why}");
            return false;
        }
    };
    let epoch = (*EPOCH.locked())();
    let same = |t: &ID3D12Resource| {
        // SAFETY: reads a descriptor.
        let d = unsafe { t.GetDesc() };
        d.Width == frame.Width && d.Height == frame.Height && d.Format == frame.Format
    };
    if hud_free.is_some() && state.composer.is_none() {
        state.composer = Some(HudComposer::new(&device).map_err(|why| log!("HUD kept out of frame generation: unavailable ({why})")));
    }
    let composer_ready = matches!(state.composer, Some(Ok(_)));
    let hud_free = hud_free.filter(|(_, ui)| composer_ready && same(ui) && frame.Format == DXGI_FORMAT_R8G8B8A8_UNORM);
    let reset = stream.epoch != epoch || stream.hud_free != hud_free.is_some();
    (stream.epoch, stream.hud_free) = (epoch, hud_free.is_some());
    let Ok(Some(list)) = state.recorder.begin() else { return skip("command lists busy") };
    stream.world_slot = state.slot;
    let world = stream.world[state.slot].clone();
    d3d12::copy_whole(&list, &real, buffer, D3D12_RESOURCE_STATE_PRESENT);
    // The HUD layer over the real frame (the frame has no HUD of its own): what is shown.
    let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    if let (Some(hud), Some(shown), Some(Ok(overlay))) = (hud, fitting(&mut state.shown[state.slot][index], &device, &frame, DXGI_FORMAT_R8G8B8A8_UNORM, uav), state.overlay.as_mut()) {
        overlay.lay(&list, &real, &hud.layer, &shown, &hud.params, index);
        state.hud_laid[index] = true;
    }
    late.record(&list);
    if let (Some(((ui_source, ui_state), ui)), Some(Ok(composer))) = (&hud_free, state.composer.as_mut()) {
        d3d12::copy_whole(&list, ui, ui_source, *ui_state);
        composer.unblend(&list, index, ui, &real, &world);
    }
    let motion_view = if taken.motion_format == DXGI_FORMAT_R16G16_TYPELESS { Some(DXGI_FORMAT_R16G16_FLOAT) } else { None };
    if hud_free.is_some() {
        HUD_FREE.fetch_add(1, Relaxed);
    }
    // What is interpolated: the world without its HUD when the game gave its UI layer, else the frame.
    let source = if hud_free.is_some() { &world } else { &real };
    let generated = stream.eye.generate(&list, &camera, &depth, &motion, motion_view, source, reset, 16.7);
    if generated.is_ok()
        && let (Some((_, ui)), Some(Ok(composer))) = (&hud_free, state.composer.as_mut())
    {
        composer.compose(&list, index, ui, &stream.eye.output);
    }
    // The HUD layer over the generated frame too.
    if generated.is_ok()
        && state.hud_laid[index]
        && let (Some(hud), Some(shown), Some(Ok(overlay))) = (hud, fitting(&mut state.generated_shown[index], &device, &frame, DXGI_FORMAT_R8G8B8A8_UNORM, uav), state.overlay.as_mut())
    {
        overlay.lay(&list, &stream.eye.output, &hud.layer, &shown, &hud.params, index);
    }
    if let Err(e) = state.recorder.submit(queue) {
        return skip(&format!("submit: {e}"));
    }
    match generated {
        Ok(true) => true,
        Ok(false) => skip("history reset"),
        Err(why) => skip(&why),
    }
}

/// The left eye's present.
fn left(queue: &ID3D12CommandQueue, buffer: &ID3D12Resource, hud: Option<&EyeHud>) -> bool {
    let mut guard = STATE.locked();
    if guard.is_none() {
        match d3d12::device_of(queue).map(|d| Recorder::new(&d, 6)) {
            Ok(Ok(recorder)) => {
                *guard = Some(State {
                    recorder,
                    streams: [None, None],
                    real: Default::default(),
                    shown: Default::default(),
                    generated_shown: [None, None],
                    hud_laid: [false; 2],
                    overlay: None,
                    slot: 0,
                    generated: [false; 2],
                    previous_pose: None,
                    interval: Interval::new(Duration::from_millis(28)),
                    composer: None,
                });
            }
            _ => return false,
        }
    }
    let state = guard.as_mut().expect("made above");
    state.generated[0] = eye_present(state, queue, 0, buffer, hud);
    true
}

/// The right eye's present: the generated pair now, the real pair half a frame later.
fn right(queue: &ID3D12CommandQueue, buffer: &ID3D12Resource, pose: HeadPose, hud: Option<&EyeHud>) -> bool {
    let mut guard = STATE.locked();
    let Some(state) = guard.as_mut() else { return false };
    state.generated[1] = eye_present(state, queue, 1, buffer, hud);
    let slot = state.slot;
    // The frames with the HUD laid over when both eyes got it, else the frames as presented.
    let hud_laid = state.hud_laid == [true, true];
    let pair = |frames: &[Option<ID3D12Resource>; 2]| frames[0].clone().zip(frames[1].clone());
    let Some((left, right)) = (if hud_laid { pair(&state.shown[slot]) } else { None }).or_else(|| pair(&state.real[slot])) else { return false };
    if hud_laid && HUD_LAID.fetch_add(1, Relaxed) == 0 {
        log!("frame generation: the HUD layer (kept out of the frame) is laid over the real and the generated frames; the pieces go to the hands");
    }
    state.slot = (slot + 1) % SLOTS;
    let due = state.interval.due(Instant::now());
    let previous = state.previous_pose.replace(pose);
    let generated = state.generated == [true, true];
    if generated && let (Some(previous), Some(l), Some(r)) = (previous, state.streams[0].as_ref(), state.streams[1].as_ref()) {
        let mut halfway = monaka_framegen::blend(&previous, &pose, 0.5);
        // Marks a generated pair for PC checks (the probe); gaze is otherwise unused.
        halfway.gaze.sample_time = -1;
        let shown = if hud_laid && let Some((gl, gr)) = pair(&state.generated_shown) {
            [gl, gr]
        } else {
            [l.eye.output.clone(), r.eye.output.clone()]
        };
        let real = Due { eyes: [left, right], pose };
        PACING.now_then(
            || {
                publish_pair(&shown, halfway);
                GENERATED.fetch_add(1, Relaxed);
            },
            Some((due, real)),
        );
        return true;
    }
    PACING.now_then(|| publish_real(Due { eyes: [left, right], pose }), None);
    true
}

/// Stops the pacing thread and frees everything once the GPU is done with it.
pub fn close() {
    ON.store(false, Release);
    PACING.stop();
    *EPOCH.locked() = no_epoch;
    let Some(state) = STATE.locked().take() else { return };
    monaka_framegen::free_when_idle(state, |s| &s.recorder);
}

/// The counts' line.
pub fn report() -> String {
    let (copies, cameras) = (inputs::copies(), inputs::cameras());
    format!(
        "frame generation: generated pairs={}, real pairs={}, eyes not generated={}, inputs copied L/R={}/{}, cameras L/R={}/{}, UI layers L/R={}/{}, frames with the HUD kept out={}",
        GENERATED.load(Relaxed),
        REAL.load(Relaxed),
        monaka_framegen::eyes_skipped(),
        copies[0],
        copies[1],
        cameras[0],
        cameras[1],
        UI_TAGS[0].load(Relaxed),
        UI_TAGS[1].load(Relaxed),
        HUD_FREE.load(Relaxed)
    )
}
