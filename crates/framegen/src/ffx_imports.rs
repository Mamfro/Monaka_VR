//! A game module's imports of AMD's FidelityFX loader hooked (its import table: no code patched):
//! the calls through which it runs its FSR upscaler, `ffxCreateContext`, `ffxDispatch` and
//! `ffxDestroyContext`. Each call runs the game's own handler first ([`Handlers`]: probes, labels),
//! then the frame-inputs provider's on an upscale dispatch ([`provide`]: each eye's inputs for the
//! features that asked, read from the game's own description), then per-eye upscaling's rewrite
//! when it is armed ([`crate::upscale_per_eye`]), then the loader. The provider always sees the
//! game's own motion vectors, whatever per-eye upscaling gives the upscaler in their place.

use crate::upscale::{GameDispatch, create_flags, read_game_dispatch};
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

const LOADER: &str = crate::ffx::LOADER;

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

/// The create flags (`crate::upscale::create`) of the game's newest upscale context, once it made
/// one while the hooks were in.
pub fn upscale_flags() -> Option<u32> {
    let flags = UPSCALE_FLAGS.load(Relaxed);
    (flags & MADE != 0).then_some(flags as u32)
}

unsafe extern "C" fn create(context: *mut *mut c_void, desc: *const c_void, allocator: *const c_void) -> u32 {
    let _flight = InFlight::enter();
    // SAFETY: the original, with the game's own arguments.
    let result = unsafe { CREATE.get()(context, desc, allocator) };
    if let Some(handle) = handlers().create {
        handle(context, desc, result);
    }
    if result == 0
        && let Some(flags) = create_flags(desc)
    {
        UPSCALE_FLAGS.store(MADE | flags as u64, Relaxed);
    }
    // SAFETY: the game's own call's arguments; CREATE is the loader's function.
    unsafe { upscale_per_eye::created(CREATE.get(), context, desc, allocator, result) };
    result
}

unsafe extern "C" fn destroy(context: *mut *mut c_void, allocator: *const c_void) -> u32 {
    let _flight = InFlight::enter();
    let handle = mem::read::<usize>(context as usize).unwrap_or(0);
    if let Some(handle_destroyed) = handlers().destroy {
        handle_destroyed(handle);
    }
    // SAFETY: the game's own call's arguments; DESTROY is the loader's function.
    unsafe { upscale_per_eye::destroying(DESTROY.get(), handle, allocator) };
    // SAFETY: the original, with the game's own arguments.
    unsafe { DESTROY.get()(context, allocator) }
}

unsafe extern "C" fn dispatch(context: *mut *mut c_void, desc: *const c_void) -> u32 {
    let _flight = InFlight::enter();
    let provider = *PROVIDER.locked();
    // SAFETY: the game's own arguments; DISPATCH is the loader's function.
    unsafe { layered(handlers(), provider, DISPATCH.get(), context, desc) }
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
    upscale_per_eye::release(DESTROY.is_set().then(|| DESTROY.get()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffx::{Float2, Resource, Size2, state};
    use crate::motion;
    use crate::upscale::{Frame, GameDispatchCopy};
    use monaka_channel::d3d12::{self, Recorder};
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16G16_FLOAT, DXGI_FORMAT_R32_FLOAT};
    use windows::core::Interface;

    /// The game's upscale context, and the twin the loader makes.
    const GAME: usize = 0x1000;
    const TWIN: usize = 0x2000;
    /// What the loader was asked: each dispatch's context, motion vectors and reset; contexts let go.
    static DISPATCHED: Mutex<Vec<(usize, usize, bool)>> = Mutex::new(Vec::new());
    static DESTROYED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static TWINS: AtomicU64 = AtomicU64::new(0);
    /// The motion vectors the provider saw.
    static SEEN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static EYE: Mutex<Option<usize>> = Mutex::new(None);

    unsafe extern "C" fn loader_create(context: *mut *mut c_void, _: *const c_void, _: *const c_void) -> u32 {
        TWINS.fetch_add(1, Relaxed);
        // SAFETY: the test's own handle slot.
        unsafe { *context = TWIN as *mut c_void };
        0
    }

    unsafe extern "C" fn loader_dispatch(context: *mut *mut c_void, desc: *const c_void) -> u32 {
        // SAFETY: the context slot and description the layers passed on.
        let (handle, game) = unsafe { (*context as usize, read_game_dispatch(desc)) };
        if let Some(game) = game {
            DISPATCHED.locked().push((handle, game.motion_vectors.resource, game.reset));
        }
        0
    }

    unsafe extern "C" fn loader_destroy(context: *mut *mut c_void, _: *const c_void) -> u32 {
        // SAFETY: the context slot passed in.
        DESTROYED.locked().push(unsafe { *context } as usize);
        0
    }

    fn provider(game: &GameDispatch) {
        SEEN.locked().push(game.motion_vectors.resource);
    }

    fn eye() -> Option<usize> {
        *EYE.locked()
    }

    /// What KCD's own per-eye FSR did with a frame of `eye` after a frame of `previous_eye`: the
    /// context it went to, whether its motion vectors were chained, whether its history was reset.
    fn kcd_rules(previous_eye: Option<usize>, eye: usize, game_reset: bool) -> (usize, bool, bool) {
        let chained = previous_eye == Some(1 - eye);
        (if eye == 0 { GAME } else { TWIN }, chained, game_reset || !chained)
    }

    /// The per-eye upscaler is the run's: one test walks it.
    #[test]
    fn each_eyes_upscale_goes_to_its_own_context_with_its_own_vectors() {
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: creates a device on the default adapter.
        unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
        let device = device.unwrap();
        // SAFETY: creates a direct queue on a live device.
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }) }.unwrap();
        let none = D3D12_RESOURCE_FLAG_NONE;
        let depth = d3d12::texture(&device, 16, 8, DXGI_FORMAT_R32_FLOAT, none).unwrap();
        let vectors = d3d12::texture(&device, 16, 8, DXGI_FORMAT_R16G16_FLOAT, none).unwrap();
        let game_vectors = vectors.as_raw() as usize;
        let mut recorder = Recorder::new(&device, 1).unwrap();
        let list = recorder.begin().unwrap().unwrap();
        // The game's vectors are readable by shaders, as KCD2 gives them.
        d3d12::barrier(&list, &vectors, D3D12_RESOURCE_STATE_COMMON, motion::READABLE);
        let read = state::PIXEL_COMPUTE_READ;
        let frame = |reset: bool| Frame {
            color: Resource::default(),
            depth: Resource::texture(&depth, read, None),
            motion_vectors: Resource::texture(&vectors, read, None),
            reactive: None,
            output: Resource::default(),
            render_size: Size2 { width: 16, height: 8 },
            jitter: Float2::default(),
            motion_vector_scale: Float2 { x: 16.0, y: 8.0 },
            sharpness: None,
            frame_time_ms: 16.0,
            reset,
            near: 0.05,
            far: 8000.0,
            fov_vertical: 1.0,
            flags: 0,
        };
        let desc = |reset: bool| GameDispatchCopy::for_tests(&list, Resource::texture(&depth, read, None), Resource::texture(&vectors, read, None), &frame(reset));
        let mut context = GAME as *mut c_void;
        let run = |context: &mut *mut c_void, desc: *const c_void| {
            // SAFETY: the test's context slot and description; the test loader's dispatch.
            unsafe { layered(Handlers::default(), Some(provider), loader_dispatch, context, desc) }
        };

        // The game's upscale context gets its twin; another kind, or a failed create, none.
        upscale_per_eye::arm(eye);
        let upscale = [0x10000u64, 0, 9, 0, 0, 0];
        let generation = [0x20001u64, 0, 0, 0, 0, 0];
        // SAFETY: the test's create descriptions and the test loader.
        unsafe {
            upscale_per_eye::created(loader_create, &mut context, upscale.as_ptr().cast(), std::ptr::null(), 0);
            upscale_per_eye::created(loader_create, &mut context, generation.as_ptr().cast(), std::ptr::null(), 0);
            upscale_per_eye::created(loader_create, &mut context, upscale.as_ptr().cast(), std::ptr::null(), 1);
        }
        assert_eq!(TWINS.load(Relaxed), 1, "one twin, for the game's upscale context");
        assert!(upscale_per_eye::running());
        assert_eq!(crate::upscale::create_flags(upscale.as_ptr().cast()), Some(9));
        assert_eq!(crate::upscale::create_flags(generation.as_ptr().cast()), None);

        // Each eye's frame as KCD's own per-eye FSR ran it: its context, its chain, its reset. The
        // provider always sees the game's own vectors.
        let mut previous_eye = None;
        for (eye, game_reset) in [(0, false), (1, false), (0, false), (0, false), (1, false), (0, true), (1, false)] {
            *EYE.locked() = Some(eye);
            let game = desc(game_reset);
            assert_eq!(run(&mut context, game.as_desc()), 0);
            let (target, chained, reset) = kcd_rules(previous_eye, eye, game_reset);
            let &(sent_to, sent_vectors, sent_reset) = DISPATCHED.locked().last().unwrap();
            assert_eq!((sent_to, sent_reset), (target, reset), "eye {eye} after {previous_eye:?}");
            assert_eq!(sent_vectors != game_vectors, chained, "eye {eye} after {previous_eye:?}: chained");
            assert_eq!(SEEN.locked().last(), Some(&game_vectors), "the provider sees the game's vectors");
            previous_eye = Some(eye);
        }

        // Passed through as the game made it: another context, a frame of no eye, a dispatch that
        // is not an upscale (and not the provider's), per-eye upscaling stopped.
        let passed = |context: usize| DISPATCHED.locked().last() == Some(&(context, game_vectors, false));
        let mut other = 0x3000 as *mut c_void;
        run(&mut other, desc(false).as_desc());
        assert!(passed(0x3000));
        *EYE.locked() = None;
        run(&mut context, desc(false).as_desc());
        assert!(passed(GAME));
        let (dispatches, seen) = (DISPATCHED.locked().len(), SEEN.locked().len());
        let generate = [0x20003u64, 0, 0, 0];
        run(&mut context, generate.as_ptr().cast());
        assert_eq!((DISPATCHED.locked().len(), SEEN.locked().len()), (dispatches, seen));
        *EYE.locked() = Some(1);
        upscale_per_eye::stop();
        run(&mut context, desc(false).as_desc());
        assert!(passed(GAME) && !upscale_per_eye::running());

        // The game lets its context go: the twin goes with it.
        // SAFETY: the test loader's destroy.
        unsafe { upscale_per_eye::destroying(loader_destroy, GAME, std::ptr::null()) };
        assert_eq!(*DESTROYED.locked(), [TWIN]);
        upscale_per_eye::release(Some(loader_destroy));
        assert_eq!(DESTROYED.locked().len(), 1, "nothing left to let go");
        assert!(upscale_per_eye::summary().contains("4 left and 3 right upscales on their own contexts (3 reset)"), "{}", upscale_per_eye::summary());

        d3d12::barrier(&list, &vectors, motion::READABLE, D3D12_RESOURCE_STATE_COMMON);
        recorder.submit(&queue).unwrap();
        assert!(recorder.idle(2000));
    }
}
