//! The game's own FSR upscaling kept per eye: the per-eye FFX upscaler. FSR keeps a history of past
//! frames, and with alternate eyes the game's previous frame is always the other eye's: the history
//! of the two eyes blends (shimmer, smearing). So each eye gets its own FSR context and its own
//! motion vectors, inside the game's own FidelityFX calls ([`crate::ffx_imports`] hooks them):
//!
//! - when the game makes its upscale context (`ffxCreateContext`), a twin is made from the same
//!   description: the game's serves the left eye's frames, the twin the right's;
//! - at each upscale (`ffxDispatch`, once a frame on the render thread, on the game's command list)
//!   the frame's eye is the game's answer ([`arm`]); the game's motion vectors are chained over two
//!   frames onto that eye's previous frame on the GPU ([`crate::motion`], the game's own vectors of
//!   the frame before kept for it), and a copy of the game's description with those vectors goes to
//!   that eye's context. A frame whose chain is broken (the first, or after a frame of the same eye)
//!   resets its eye's history.
//!
//! When the game itself renders each eye against that eye's own previous frame
//! ([`use_game_history`]), its vectors already reach that frame and pass as they are: only the
//! context is the eye's.
//!
//! The vectors are chained in the scale the game gives FSR for them (to render pixels), read in the
//! state it describes them in.

use monaka_ffx::{Resource, state};
use crate::ffx_imports::{CreateFn, DestroyFn, DispatchFn};
use crate::motion::{self, MotionFix};
use monaka_ffx::upscale::{GameDispatchCopy, creates_upscaler, read_game_dispatch};
use monaka_channel::d3d12;
use monaka_core::sync::Locked;
use monaka_hook::mem;
use monaka_producer::{log, log_first};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATES, ID3D12Device, ID3D12GraphicsCommandList, ID3D12Resource};
use windows::core::Interface;

/// Per-eye upscaling is on for this run.
static ON: AtomicBool = AtomicBool::new(false);
/// The game's motion vectors already reach each eye's own previous frame ([`use_game_history`]).
static GAME_HISTORY: AtomicBool = AtomicBool::new(false);
/// The game's answer, during its dispatch, to which eye the frame being upscaled is.
pub type EyeOfFrame = fn() -> Option<usize>;

static EYE: Mutex<Option<EyeOfFrame>> = Mutex::new(None);
/// Upscales per eye, and those passed through as the game made them.
static UPSCALED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static PASSED: AtomicU64 = AtomicU64::new(0);
static RESETS: AtomicU64 = AtomicU64::new(0);

struct State {
    /// The game's upscale context (its handle) and our twin for the right eye.
    game: usize,
    twin: usize,
    composer: Option<MotionFix>,
    /// The game's motion vectors of the frame before, and its eye.
    previous: Option<ID3D12Resource>,
    previous_eye: Option<usize>,
    /// Each eye has been upscaled on its context since it was made (with the game's history, a
    /// first frame resets it).
    seen: [bool; 2],
    /// Something failed for good: the game's upscaling runs as it was.
    broken: bool,
}

static STATE: Mutex<State> = Mutex::new(State { game: 0, twin: 0, composer: None, previous: None, previous_eye: None, seen: [false; 2], broken: false });

/// Per-eye upscaling on for the run: `eye` tells, during the game's dispatch, which eye the frame
/// being upscaled is (`None`: no eye's, upscaled as the game made it).
pub fn arm(eye: EyeOfFrame) {
    *EYE.locked() = Some(eye);
    ON.store(true, Release);
}

/// The game describes each eye's own previous frame itself (a producer that hands each eye's
/// render its own previous camera): its motion vectors pass as they are, nothing chained.
pub fn use_game_history(on: bool) {
    GAME_HISTORY.store(on, Relaxed);
}

/// At stop, first: the game's upscales go to its own context again, and no new twin is made (the
/// size switched back makes the game remake its context).
pub fn stop() {
    ON.store(false, Release);
}

/// Per-eye upscaling is on for this run.
pub fn armed() -> bool {
    ON.load(Acquire)
}

/// Each eye's frames go to its own context: armed, and the game's context has its twin.
pub fn running() -> bool {
    armed() && STATE.locked().twin != 0
}

/// After the game's `ffxCreateContext` made the context in `context` from `desc` with `result`
/// (`create` the loader's own): an upscale context gets a twin for the right eye.
///
/// # Safety
/// The arguments are the game's own call's (its description chain alive for the call), `create`
/// the loader's function.
pub unsafe fn created(create: CreateFn, context: *mut *mut c_void, desc: *const c_void, allocator: *const c_void, result: u32) {
    if result != 0 || !creates_upscaler(desc) || !armed() {
        return;
    }
    let game = mem::read::<usize>(context as usize).unwrap_or(0);
    let mut twin: *mut c_void = std::ptr::null_mut();
    // SAFETY: the loader's create, with the game's own description (its chain is alive for the
    // call) and allocator, into our own handle.
    let made = unsafe { create(&mut twin, desc, allocator) };
    let mut s = STATE.locked();
    if made == 0 && !twin.is_null() {
        log!("fsr: the game's upscale context {game:#x} has a twin {:#x} for the right eye", twin as usize);
        (s.game, s.twin, s.previous_eye, s.seen) = (game, twin as usize, None, [false; 2]);
    } else {
        log!("fsr: no twin context ({made:#x}); the game's upscaling runs as it is");
        (s.game, s.twin) = (0, 0);
    }
}

/// Before the game's `ffxDestroyContext` lets its context `handle` go: our twin of it goes too
/// (`destroy` the loader's own).
///
/// # Safety
/// The arguments are the game's own call's, `destroy` the loader's function.
pub unsafe fn destroying(destroy: DestroyFn, handle: usize, allocator: *const c_void) {
    let mut s = STATE.locked();
    if handle != 0 && handle == s.game {
        let mut twin = s.twin as *mut c_void;
        if !twin.is_null() {
            // SAFETY: our own twin context, made by the loader's create; the game is letting its
            // own go, so neither is in use.
            unsafe { destroy(&mut twin, allocator) };
        }
        (s.game, s.twin, s.previous, s.previous_eye) = (0, 0, None, None);
    }
}

/// The game's dispatch `desc` on its context `handle`: an upscale of an eye's frame run on that
/// eye's context with that eye's motion vectors (`dispatch` the loader's own), its result; `None`
/// to pass it through as it is.
///
/// # Safety
/// The arguments are the game's own call's (its description alive for the call), `dispatch` the
/// loader's function.
pub unsafe fn dispatch(dispatch: DispatchFn, handle: usize, desc: *const c_void) -> Option<u32> {
    let eye_of_frame = (*EYE.locked())?;
    let eye = eye_of_frame().filter(|&eye| eye < 2)?;
    let mut s = STATE.locked();
    if s.broken || handle == 0 || handle != s.game || s.twin == 0 {
        return None;
    }
    // SAFETY: the game's own description, alive for the call.
    let game = unsafe { read_game_dispatch(desc) }?;
    // SAFETY: as above.
    let mut copy = unsafe { GameDispatchCopy::of(desc) }?;
    let (raw_list, raw_motion) = (game.command_list as *mut c_void, game.motion_vectors.resource as *mut c_void);
    // SAFETY: the game's command list being recorded and its motion vectors, live for this frame.
    let (Some(list), Some(motion)) = (unsafe { ID3D12GraphicsCommandList::from_raw_borrowed(&raw_list) }, unsafe { ID3D12Resource::from_raw_borrowed(&raw_motion) }) else {
        log_first(3, format_args!("fsr: the game's upscale has no command list or motion vectors"));
        return None;
    };
    let game_history = GAME_HISTORY.load(Relaxed);
    let motion_state = state::d3d12(game.motion_vectors.state).unwrap_or(motion::READABLE);
    let chained = if game_history { None } else { chain(&mut s, list, motion, motion_state, eye, [game.motion_vector_scale.x, game.motion_vector_scale.y]) };
    let first = !std::mem::replace(&mut s.seen[eye], true);
    let reset = game.reset || if game_history { first } else { chained.is_none() };
    if let Some(output) = &chained {
        copy.set_motion_vectors(Resource::texture(output, state::PIXEL_COMPUTE_READ, None));
    }
    copy.set_reset(reset);
    if reset {
        RESETS.fetch_add(1, Relaxed);
    }
    let mut target = (if eye == 0 { s.game } else { s.twin }) as *mut c_void;
    drop(s);
    let n = UPSCALED[eye].fetch_add(1, Relaxed);
    if n < 3 {
        let vectors = if game_history { "the game's vectors" } else if chained.is_some() { "chained" } else { "not chained" };
        log!("fsr: eye {eye} upscaled on its own context ({vectors}{})", if reset { ", reset" } else { "" });
    }
    // SAFETY: the loader's dispatch on the eye's context (the game's or our twin of it), with a copy
    // of the game's description that lives through the call.
    Some(unsafe { dispatch(&mut target, copy.as_desc()) })
}

/// A dispatch of the game's went to the loader as the game made it.
pub(crate) fn passed() {
    PASSED.fetch_add(1, Relaxed);
}

/// Records, on the game's `list`, the game's `motion` vectors (in `motion_state`, scaled to render
/// pixels by `scale`) chained onto `eye`'s previous frame, and keeps this frame's for the next. The
/// chained vectors, or None when the chain is broken.
fn chain(s: &mut State, list: &ID3D12GraphicsCommandList, motion: &ID3D12Resource, motion_state: D3D12_RESOURCE_STATES, eye: usize, scale: [f32; 2]) -> Option<ID3D12Resource> {
    let device: ID3D12Device = d3d12::device_of(motion).ok()?;
    if s.composer.is_none() {
        match MotionFix::composer(&device) {
            Ok(composer) => s.composer = Some(composer),
            Err(why) => {
                log!("fsr: per-eye motion vectors: {why}; the game's upscaling runs as it is");
                s.broken = true;
                return None;
            }
        }
    }
    // SAFETY: reads a descriptor.
    let desc = unsafe { motion.GetDesc() };
    // The frame before was the other eye's: chained through it, this eye's previous frame.
    let previous = s.previous.clone().filter(|_| s.previous_eye == Some(1 - eye)).filter(|p| {
        // SAFETY: reads a descriptor.
        let d = unsafe { p.GetDesc() };
        d.Width == desc.Width && d.Height == desc.Height && d.Format == desc.Format
    });
    let chained = match (previous, s.composer.as_mut()) {
        (Some(previous), Some(composer)) => {
            d3d12::barrier(list, &previous, D3D12_RESOURCE_STATE_COMMON, motion::READABLE);
            let recorded = composer.record(list, &[&previous, motion], &motion::Constants::chaining(scale));
            d3d12::barrier(list, &previous, motion::READABLE, D3D12_RESOURCE_STATE_COMMON);
            match recorded.and_then(|()| composer.output(desc.Width as u32, desc.Height)) {
                Ok(output) => Some(output),
                Err(why) => {
                    log!("fsr: per-eye motion vectors: {why}; the game's upscaling runs as it is");
                    s.broken = true;
                    None
                }
            }
        }
        _ => None,
    };
    // This frame's vectors, for the next frame's chain.
    if let Some(keep) = d3d12::fitting(&mut s.previous, &device, desc.Width, desc.Height, desc.Format, D3D12_RESOURCE_FLAG_NONE) {
        d3d12::copy_whole(list, &keep, motion, motion_state);
        s.previous_eye = Some(eye);
    } else {
        s.previous_eye = None;
    }
    chained
}

/// At stop, once the hooks are out: our twin context goes (`destroy` the loader's own, when it was
/// hooked), after the GPU has had time to finish the frames that used it.
pub fn release(destroy: Option<DestroyFn>) {
    let twin = {
        let mut s = STATE.locked();
        let twin = s.twin;
        (s.game, s.twin, s.previous, s.previous_eye, s.composer) = (0, 0, None, None, None);
        twin
    };
    let Some(destroy) = destroy.filter(|_| twin != 0) else { return };
    std::thread::sleep(std::time::Duration::from_millis(250));
    let mut handle = twin as *mut c_void;
    // SAFETY: our own context, made by the loader's create; nothing dispatches on it any more.
    let result = unsafe { destroy(&mut handle, std::ptr::null()) };
    log!("fsr: twin context {twin:#x} let go ({result:#x})");
}

pub fn summary() -> String {
    format!(
        "fsr: {} left and {} right upscales on their own contexts ({} reset), {} passed through",
        UPSCALED[0].load(Relaxed),
        UPSCALED[1].load(Relaxed),
        RESETS.load(Relaxed),
        PASSED.load(Relaxed)
    )
}
