//! A game module's imports of AMD's FidelityFX loader hooked (its import table: no code patched):
//! the calls through which it runs its FSR upscaler, `ffxCreateContext`, `ffxDispatch` and
//! `ffxDestroyContext`. Each call runs the game's own handler first ([`Handlers`]: probes, labels),
//! then the frame-inputs provider's on an upscale dispatch ([`provide`]: each eye's inputs for the
//! features that asked, read from the game's own description), then per-eye upscaling's rewrite
//! when it is armed ([`crate::upscale_per_eye`]), then the loader. The provider always sees the
//! game's own motion vectors, whatever per-eye upscaling gives the upscaler in their place.

use monaka_ffx::upscale::{GameDispatch, create_flags, read_game_dispatch};
use crate::upscale_per_eye;
use monaka_core::sync::Locked;
use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight, Original, mem};
use monaka_producer::log;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// `ffxCreateContext(context, desc, allocator)`.
pub type CreateFn = unsafe extern "C" fn(*mut *mut c_void, *const c_void, *const c_void) -> u32;
/// `ffxDispatch(context, desc)`.
pub type DispatchFn = unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> u32;
/// `ffxDestroyContext(context, allocator)`.
pub type DestroyFn = unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> u32;

/// What the game does in each hooked call, before anything else: the create's context slot,
/// description and result (after the loader made it); the dispatch's context handle and
/// description; the destroyed context's handle.
#[derive(Clone, Copy, Default)]
pub struct Handlers {
    pub create: Option<fn(*mut *mut c_void, *const c_void, u32)>,
    pub dispatch: Option<fn(usize, *const c_void)>,
    pub destroy: Option<fn(usize)>,
}

/// Per-eye upscaling ([`crate::upscale_per_eye`]): which eye the frame being upscaled is, the
/// game's answer during its dispatch (after its own handler ran).
#[derive(Clone, Copy)]
pub struct PerEye {
    pub eye: upscale_per_eye::EyeOfFrame,
}

/// What to hook and do.
#[derive(Clone, Copy, Default)]
pub struct Install {
    pub handlers: Handlers,
    pub per_eye: Option<PerEye>,
}

/// The frame-inputs provider's handler: an upscale dispatch of the game's, as the game made it.
pub type Provider = fn(&GameDispatch);

const LOADER: &str = monaka_ffx::LOADER;

static CREATE: Original<CreateFn> = Original::new();
static DISPATCH: Original<DispatchFn> = Original::new();
static DESTROY: Original<DestroyFn> = Original::new();
static HANDLERS: Mutex<Handlers> = Mutex::new(Handlers { create: None, dispatch: None, destroy: None });
static PROVIDER: Mutex<Option<Provider>> = Mutex::new(None);
/// The flags of the game's newest upscale context (the high bit set once one was made).
static UPSCALE_FLAGS: AtomicU64 = AtomicU64::new(0);
const MADE: u64 = 1 << 63;

fn handlers() -> Handlers {
    *HANDLERS.locked()
}

/// The frame-inputs provider's handler, run on each upscale dispatch after the game's own
/// (whichever starts first); `None` takes it out.
pub fn provide(provider: Option<Provider>) {
    *PROVIDER.locked() = provider;
}

/// The create flags (`monaka_ffx::upscale::create`) of the game's newest upscale context, once it made
/// one while the hooks were in.
pub fn upscale_flags() -> Option<u32> {
    let flags = UPSCALE_FLAGS.load(Relaxed);
    (flags & MADE != 0).then_some(flags as u32)
}

unsafe extern "C" fn create(context: *mut *mut c_void, desc: *const c_void, allocator: *const c_void) -> u32 {
    let flight = InFlight::enter();
    // SAFETY: the original, with the game's own arguments.
    let result = unsafe { CREATE.get(&flight)(context, desc, allocator) };
    if let Some(handle) = handlers().create {
        handle(context, desc, result);
    }
    if result == 0
        && let Some(flags) = create_flags(desc)
    {
        UPSCALE_FLAGS.store(MADE | flags as u64, Relaxed);
    }
    // SAFETY: the game's own call's arguments; CREATE is the loader's function.
    unsafe { upscale_per_eye::created(CREATE.get(&flight), context, desc, allocator, result) };
    result
}

unsafe extern "C" fn destroy(context: *mut *mut c_void, allocator: *const c_void) -> u32 {
    let flight = InFlight::enter();
    let handle = mem::read::<usize>(context as usize).unwrap_or(0);
    if let Some(handle_destroyed) = handlers().destroy {
        handle_destroyed(handle);
    }
    // SAFETY: the game's own call's arguments; DESTROY is the loader's function.
    unsafe { upscale_per_eye::destroying(DESTROY.get(&flight), handle, allocator) };
    // SAFETY: the original, with the game's own arguments.
    unsafe { DESTROY.get(&flight)(context, allocator) }
}

unsafe extern "C" fn dispatch(context: *mut *mut c_void, desc: *const c_void) -> u32 {
    let flight = InFlight::enter();
    let provider = *PROVIDER.locked();
    // SAFETY: the game's own arguments; DISPATCH is the loader's function.
    unsafe { layered(handlers(), provider, DISPATCH.get(&flight), context, desc) }
}

/// One dispatch of the game's through the layers: its own handler, the provider's (an upscale
/// dispatch, read as the game made it), per-eye upscaling's rewrite, or else the loader (`original`)
/// as the game called it.
///
/// # Safety
/// The arguments are the game's own call's, `original` the loader's dispatch.
unsafe fn layered(handlers: Handlers, provider: Option<Provider>, original: DispatchFn, context: *mut *mut c_void, desc: *const c_void) -> u32 {
    let handle = mem::read::<usize>(context as usize).unwrap_or(0);
    if let Some(handle_dispatch) = handlers.dispatch {
        handle_dispatch(handle, desc);
    }
    // SAFETY: the game's own description, alive for the call.
    if let Some(provide) = provider
        && let Some(game) = unsafe { read_game_dispatch(desc) }
    {
        provide(&game);
    }
    // SAFETY: the game's own call's arguments and the loader's dispatch.
    if upscale_per_eye::armed()
        && let Some(result) = unsafe { upscale_per_eye::dispatch(original, handle, desc) }
    {
        return result;
    }
    upscale_per_eye::passed();
    // SAFETY: the original, with the game's own arguments.
    unsafe { original(context, desc) }
}

/// Hooks `module`'s imports of the FidelityFX loader as `install` says, per-eye upscaling armed
/// with it (or not, for this run). True when hooked; a module that imports no `ffxDispatch` is
/// logged and left as it is.
///
/// # Safety
/// `module` must import these from AMD's loader with their FFX API signatures (`ffx_api.h`).
pub unsafe fn install(hooks: &mut Hooks, module: &Module, install: Install) -> monaka_hook::Result<bool> {
    match install.per_eye {
        Some(per_eye) => upscale_per_eye::arm(per_eye.eye),
        None => upscale_per_eye::stop(),
    }
    if module.import_slot(LOADER, "ffxDispatch").is_none() {
        log!("fsr: {} imports no ffxDispatch; the game's upscaling runs as it is", module.base());
        return Ok(false);
    }
    *HANDLERS.locked() = install.handlers;
    // SAFETY: the module's own import slots of the loader's exports, whose signatures (ffx_api.h)
    // the detours have (the caller's guarantee).
    unsafe {
        hooks.import(&CREATE, "ffxCreateContext", module, LOADER, "ffxCreateContext", create as CreateFn)?;
        hooks.import(&DISPATCH, "ffxDispatch", module, LOADER, "ffxDispatch", dispatch as DispatchFn)?;
        hooks.import(&DESTROY, "ffxDestroyContext", module, LOADER, "ffxDestroyContext", destroy as DestroyFn)?;
    }
    Ok(true)
}

/// At stop, once the hooks are out: per-eye upscaling's twin context goes.
pub fn release() {
    let flight = monaka_hook::InFlight::enter();
    upscale_per_eye::release(DESTROY.is_set().then(|| DESTROY.get(&flight)));
}

#[cfg(test)]
mod tests;
