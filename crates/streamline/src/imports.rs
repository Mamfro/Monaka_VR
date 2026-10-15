//! The renderer's Streamline imports hooked (its import table: no code patched): the calls through
//! which it gives DLSS its inputs each frame, `slSetTagForFrame` (the tagged buffers) and
//! `slSetConstants` (the camera), and `slEvaluateFeature`. Each call runs the game's own handler
//! first ([`Handlers`]: probes, labels), then the provider's ([`provide`]: each eye's inputs for
//! the features that asked), then per-eye DLSS's rewrite when it is armed ([`crate::per_eye`],
//! which also hooks the game's `slDLSSSetOptions`), then Streamline. The provider so always sees
//! the game's own constants, tags and motion vectors, and records its copies on the game's list
//! before per-eye DLSS records its rebase: frame generation and per-eye DLSS each take the game's
//! vectors and carry them in their own way.

use crate::{EvaluateFn, FreeFn, INTERPOSER, SetConstantsFn, SetOptionsFn, SetTagFn, Tag, feature, per_eye, read_tags};
use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight, Original};
use monaka_producer::log;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use windows::Win32::Graphics::Direct3D12::ID3D12GraphicsCommandList;

/// `slSetTagForFrame`'s handler: the call's number (from 1), the frame token, the viewport handle,
/// the tags read and the command list.
pub type TagsHandler = fn(u64, usize, *const u8, &[Tag], *mut c_void);
/// `slSetConstants`' handler: the call's number, the constants, the frame token and the viewport
/// handle. Returns the frame's eye (0 left, 1 right) when the game knows it (per-eye DLSS needs it).
pub type ConstantsHandler = fn(u64, *const u8, usize, *const u8) -> Option<usize>;
/// The constants passed on as the game made them (no rewrite): as [`ConstantsHandler`]'s.
pub type UnchangedHandler = fn(u64, *const u8, usize, *const u8);
/// `slEvaluateFeature`'s handler: the call's number, the feature, the frame token, the inputs and
/// their count, and the command list.
pub type EvaluateHandler = fn(u64, u32, *const u8, *const *const u8, u32, *mut c_void);

/// What the game does in each hooked call, before it goes on.
#[derive(Clone, Copy, Default)]
pub struct Handlers {
    pub tags: Option<TagsHandler>,
    pub constants: Option<ConstantsHandler>,
    pub unchanged: Option<UnchangedHandler>,
    pub evaluate: Option<EvaluateHandler>,
}

/// Per-eye DLSS ([`crate::per_eye`]): where the renderer keeps its pointer to `slDLSSSetOptions`
/// (filled the first time DLSS runs), what rebases the motion vectors, and whether it does.
#[derive(Clone, Copy)]
pub struct PerEye {
    pub options_slot: usize,
    pub rebase: per_eye::MakeRebase,
    pub fix_motion: bool,
}

/// What to hook and do.
#[derive(Clone, Copy, Default)]
pub struct Install {
    pub handlers: Handlers,
    pub per_eye: Option<PerEye>,
}

static HANDLERS: OnceLock<Handlers> = OnceLock::new();
/// The frame-inputs provider's handlers (`provide`), after the game's.
static PROVIDER: Mutex<Handlers> = Mutex::new(Handlers { tags: None, constants: None, unchanged: None, evaluate: None });
static SET_TAG: Original<SetTagFn> = Original::new();
static SET_CONSTANTS: Original<SetConstantsFn> = Original::new();
static EVALUATE: Original<EvaluateFn> = Original::new();
static SET_OPTIONS: Original<SetOptionsFn> = Original::new();
static GET_NATIVE: AtomicUsize = AtomicUsize::new(0);
static TAG_CALLS: AtomicU64 = AtomicU64::new(0);
static CONSTANT_CALLS: AtomicU64 = AtomicU64::new(0);
static EVALUATE_CALLS: AtomicU64 = AtomicU64::new(0);

fn handlers() -> Handlers {
    HANDLERS.get().copied().unwrap_or_default()
}

fn provider() -> Handlers {
    PROVIDER.lock().map(|p| *p).unwrap_or_default()
}

/// The frame-inputs provider's handlers, run in each hooked call after the game's own (whichever
/// starts first); its constants handler's eye is not used. `Handlers::default()` takes them out.
pub fn provide(handlers: Handlers) {
    if let Ok(mut provider) = PROVIDER.lock() {
        *provider = handlers;
    }
}

/// `slSetTagForFrame` calls so far.
pub fn tag_calls() -> u64 {
    TAG_CALLS.load(Relaxed)
}

unsafe extern "system" fn set_tag(frame: usize, viewport: *const u8, tags: *const u8, count: u32, commands: *mut c_void) -> i32 {
    let flight = InFlight::enter();
    let n = TAG_CALLS.fetch_add(1, Relaxed) + 1;
    let all = read_tags(tags, count);
    if let Some(handle) = handlers().tags {
        handle(n, frame, viewport, &all, commands);
    }
    if let Some(handle) = provider().tags {
        handle(n, frame, viewport, &all, commands);
    }
    if let Some((ours, eye_viewport)) = per_eye::tags(viewport, tags, count) {
        // SAFETY: the original, with this eye's viewport and tags (the motion vectors ours), which
        // live until the frame's next tags.
        return unsafe { SET_TAG.get(&flight)(frame, eye_viewport, ours, count, commands) };
    }
    // SAFETY: the original, with the game's arguments.
    unsafe { SET_TAG.get(&flight)(frame, viewport, tags, count, commands) }
}

unsafe extern "system" fn set_constants(constants: *const u8, frame: usize, viewport: *const u8) -> i32 {
    let flight = InFlight::enter();
    let n = CONSTANT_CALLS.fetch_add(1, Relaxed) + 1;
    let handlers = handlers();
    let eye = handlers.constants.and_then(|handle| handle(n, constants, frame, viewport));
    if let Some(handle) = provider().constants {
        handle(n, constants, frame, viewport);
    }
    if !constants.is_null()
        && let Some((ours, eye_viewport)) = per_eye::constants(constants, viewport, eye)
    {
        // SAFETY: the original, with the game's frame and this eye's constants and viewport, which
        // live until the frame's next constants.
        return unsafe { SET_CONSTANTS.get(&flight)(ours, frame, eye_viewport) };
    }
    if let Some(handle) = handlers.unchanged {
        handle(n, constants, frame, viewport);
    }
    // SAFETY: the original, with the game's arguments.
    unsafe { SET_CONSTANTS.get(&flight)(constants, frame, viewport) }
}

unsafe extern "system" fn evaluate(feature: u32, frame: *const u8, inputs: *const *const u8, count: u32, commands: *mut c_void) -> i32 {
    let flight = InFlight::enter();
    let n = EVALUATE_CALLS.fetch_add(1, Relaxed) + 1;
    if let Some(handle) = handlers().evaluate {
        handle(n, feature, frame, inputs, count, commands);
    }
    if let Some(handle) = provider().evaluate {
        handle(n, feature, frame, inputs, count, commands);
    }
    if feature == feature::DLSS
        && !inputs.is_null()
        // SAFETY: the command list the game's call was given.
        && let Some(ours) = per_eye::evaluate(inputs, count, unsafe { native_list(commands) })
    {
        // SAFETY: the original, with the game's frame and command list and this eye's viewport (the
        // motion-vector rebase recorded just before on the same list).
        return unsafe { EVALUATE.get(&flight)(feature, frame, ours.as_ptr(), count, commands) };
    }
    // SAFETY: the original, with the game's arguments.
    unsafe { EVALUATE.get(&flight)(feature, frame, inputs, count, commands) }
}

/// `slDLSSSetOptions` through the pointer the renderer keeps: the game's call, then the same
/// options for both eye viewports.
unsafe extern "system" fn set_options(viewport: *const u8, options: *const u8) -> i32 {
    let flight = InFlight::enter();
    // SAFETY: the original, with the game's arguments.
    let result = unsafe { SET_OPTIONS.get(&flight)(viewport, options) };
    if result == 0 {
        // SAFETY: Streamline's own function, with our eye viewport copies and the game's options.
        per_eye::on_options(viewport, options, |v, o| unsafe { SET_OPTIONS.get(&flight)(v, o) });
    }
    result
}

/// The game's command list behind Streamline's proxy ([`crate::native_list`]), once
/// `slGetNativeInterface` was found at install.
///
/// # Safety
/// `commands` is the command list (or null) a Streamline call of the game's was given.
pub unsafe fn native_list(commands: *mut c_void) -> Option<ID3D12GraphicsCommandList> {
    // SAFETY: GET_NATIVE is Streamline's export (or 0), and `commands` the list the game's
    // Streamline call was given (the caller's guarantee).
    unsafe { crate::native_list(GET_NATIVE.load(Relaxed), commands) }
}

/// Hooks `renderer`'s Streamline imports as `install` says; returns the interposer (none when the
/// game did not load it: logged, nothing hooked). Missing pieces are logged, not fatal.
///
/// # Safety
/// `renderer` must be the game's renderer DLL, importing these from [`INTERPOSER`].
pub unsafe fn install(hooks: &mut Hooks, renderer: &Module, install: Install) -> Option<Module> {
    let interposer = crate::interposer()?;
    if HANDLERS.set(install.handlers).is_err() {
        log!("Streamline imports: handlers already set; the first stay");
    }
    match interposer.export("slGetNativeInterface") {
        Some(native) => GET_NATIVE.store(native, Relaxed),
        None => log!("{INTERPOSER} does not export slGetNativeInterface: nothing is recorded on the game's own command lists"),
    }
    if let Some(per_eye) = install.per_eye {
        let slot = renderer.at(per_eye.options_slot);
        let native = interposer.export("slGetNativeInterface");
        // SAFETY: the slot holds Streamline's slDLSSSetOptions(const ViewportHandle&, const DLSSOptions&).
        match (native, unsafe { hooks.pointer(&SET_OPTIONS, "slDLSSSetOptions", slot, set_options as SetOptionsFn) }) {
            (Some(native), Ok(())) => {
                GET_NATIVE.store(native, Relaxed);
                per_eye::arm(per_eye.rebase, per_eye.fix_motion);
                log!("per-eye DLSS armed: it starts once the game sets its DLSS options");
            }
            (None, _) => log!("per-eye DLSS off: {INTERPOSER} does not export slGetNativeInterface"),
            (_, Err(e)) => log!("per-eye DLSS off: {e}"),
        }
    }
    // SAFETY: the caller's guarantee: the renderer imports these from the interposer, with these
    // signatures (sl_core_api.h).
    unsafe {
        crate::hook_inputs(hooks, renderer, (&SET_TAG, set_tag as SetTagFn), (&SET_CONSTANTS, set_constants as SetConstantsFn));
        if let Err(e) = hooks.import(&EVALUATE, "slEvaluateFeature", renderer, INTERPOSER, "slEvaluateFeature", evaluate as EvaluateFn) {
            log!("Streamline evaluate not hooked: {e}");
        }
    }
    Some(interposer)
}

/// After the hooks are out: Streamline frees the DLSS resources it holds for the eye viewports.
pub fn release() {
    let (Some(viewports), Some(free)) = (per_eye::eye_viewports(), Module::find(INTERPOSER).and_then(|m| m.export("slFreeResources"))) else { return };
    // SAFETY: Streamline's exported slFreeResources(Feature, const ViewportHandle&).
    let free: FreeFn = unsafe { std::mem::transmute(free) };
    // SAFETY: frees resources for our own viewport copies.
    let results = viewports.map(|v| unsafe { free(feature::DLSS, v.as_ptr()) });
    log!("DLSS resources of the eye viewports freed (results {results:?})");
}

/// Logs the calls' rates over the last `seconds` and per-eye DLSS's counts.
pub fn report(seconds: f64) {
    static LAST: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
    let now = [TAG_CALLS.load(Relaxed), CONSTANT_CALLS.load(Relaxed)];
    let rate = monaka_producer::rates(now, &LAST, seconds);
    log!("streamline: tag calls {:.1}/s, constants {:.1}/s; {}", rate[0], rate[1], per_eye::report());
}
