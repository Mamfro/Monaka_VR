//! A D3D11 game's HUD and menus placed in front of each eye. The rule (as Dying Light draws,
//! whose producer has its own copy): the finished 3D image reaches the back buffer as one
//! unblended full-screen triangle, and every later draw to the back buffer
//! in that frame is HUD or menu. Those draws get a viewport shrunk toward where straight ahead
//! lands in the eye's image and moved toward the nose, so the HUD floats readable in front of the
//! world instead of filling the periphery at infinity. Each draw's scissor rectangle moves with
//! its viewport (the HUD's text boxes are clipped by one; left in place they hide the text). Frames without such a scene copy (menus,
//! loading) are left alone. A game whose 3D image reaches the back buffer some other way (a copy
//! or a compute pass) is installed with `scene_copy` off: every back-buffer draw is HUD.
//!
//! Draws are followed on every context: the immediate one and the deferred ones a game records
//! its frame on from worker threads (Dishonored 2 records the scene copy and its HUD on a deferred
//! context; its immediate context draws only two invisible quads there).
//! Deferred contexts record in any order and ahead of the present, so with `scene_copy` the copy
//! is recognised by its shape (an unblended triangle, never moved) and, once one has been seen,
//! every other back-buffer draw is HUD: draws before the copy in a frame are covered by it.
//! (Tracking the copy per context does not work: in Dishonored 2 it places nothing.) Each context's bound target is kept in a small
//! lock-free table, so the scene's draws cost one scan of it. The methods are hooked in their
//! code, found on an immediate and a deferred context of our own (Direct3D rewrites the immediate
//! context's vtable while a game runs, which would undo a vtable hook). The game crate says where
//! each frame's HUD goes ([`install`]'s `place`, a [`HudPlacement`]) and calls [`frame_end`] at each present.

use monaka_hook::{Hooks, InFlight, Original, Placement, slot};
use monaka_producer::{Rejection, log};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BLEND_DESC, D3D11_CREATE_DEVICE_FLAG, D3D11_DEVICE_CONTEXT_IMMEDIATE, D3D11_SDK_VERSION, D3D11_VIEWPORT, D3D11CreateDevice,
    ID3D11BlendState, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
use windows::core::Interface;

pub use monaka_core::hud::{Placement as HudPlacement, Rect};

type DrawIndexedFn = unsafe extern "system" fn(*mut c_void, u32, u32, i32);
type DrawFn = unsafe extern "system" fn(*mut c_void, u32, u32);
type DrawIndexedInstancedFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32, i32, u32);
type DrawInstancedFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32, u32);
type TargetsFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void, *mut c_void);

/// Set while HUD draws are placed.
pub static ACTIVE: AtomicBool = AtomicBool::new(false);

struct Setup {
    scale: f32,
    /// Wait for the scene copy draw; otherwise every back-buffer draw is HUD.
    scene_copy: bool,
    place: fn() -> Option<HudPlacement>,
    /// Log the back-buffer draws of this many frames, from the 300th after the first.
    probe_frames: u32,
}

static SETUP: OnceLock<Setup> = OnceLock::new();
/// The current back buffer (its resource pointer).
static BACKBUFFER: AtomicUsize = AtomicUsize::new(0);

// ---- Which contexts draw to the back buffer: a fixed table, scanned without a lock.

const CONTEXT_SLOTS: usize = 64;
#[allow(clippy::declare_interior_mutable_const)]
const NO_CONTEXT: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const OFF: AtomicBool = AtomicBool::new(false);
static CONTEXT_KEYS: [AtomicUsize; CONTEXT_SLOTS] = [NO_CONTEXT; CONTEXT_SLOTS];
static CONTEXT_ON: [AtomicBool; CONTEXT_SLOTS] = [OFF; CONTEXT_SLOTS];
/// A scene copy has been seen since the run began: back-buffer draws are HUD from then on.
static SCENE_EVER: AtomicBool = AtomicBool::new(false);
/// A scene copy was seen since the last present (for the summary).
static SCENE_SEEN: AtomicBool = AtomicBool::new(false);

/// The table slot of `context`, claimed on first sight.
fn context_slot(context: usize) -> Option<usize> {
    if let Some(i) = CONTEXT_KEYS.iter().position(|k| k.load(Acquire) == context) {
        return Some(i);
    }
    (0..CONTEXT_SLOTS).find(|&i| CONTEXT_KEYS[i].compare_exchange(0, context, AcqRel, Acquire).is_ok() || CONTEXT_KEYS[i].load(Acquire) == context)
}

fn drawing_to_backbuffer(context: usize) -> Option<usize> {
    let i = CONTEXT_KEYS.iter().position(|k| k.load(Acquire) == context)?;
    CONTEXT_ON[i].load(Acquire).then_some(i)
}

/// What a context's back-buffer pass has done to its viewport.
#[derive(Clone, Copy)]
struct Moved {
    base: D3D11_VIEWPORT,
    placed: D3D11_VIEWPORT,
    /// The game's scissor rectangle and ours, when one was bound.
    scissor: Option<(RECT, RECT)>,
}

/// `r` (pixels, in `from`'s frame) moved the way `from` became `to`.
pub fn map_rect(r: &RECT, from: &Rect, to: &Rect) -> RECT {
    let (sx, sy) = (to.width / from.width, to.height / from.height);
    let x = |v: i32| to.x + (v as f32 - from.x) * sx;
    let y = |v: i32| to.y + (v as f32 - from.y) * sy;
    RECT { left: x(r.left).floor() as i32, top: y(r.top).floor() as i32, right: x(r.right).ceil() as i32, bottom: y(r.bottom).ceil() as i32 }
}

fn scissor(context: &ID3D11DeviceContext) -> Option<RECT> {
    let mut count = 1u32;
    let mut rect = RECT::default();
    // SAFETY: reads one scissor rectangle into a local.
    unsafe { context.RSGetScissorRects(&mut count, Some(&mut rect)) };
    (count > 0).then_some(rect)
}

/// Gives a context back what we moved.
fn put_back(context: &ID3D11DeviceContext, moved: &Moved) {
    // SAFETY: sets the game's own viewport and scissor back on its context.
    unsafe {
        context.RSSetViewports(Some(&[moved.base]));
        if let Some((base, _)) = moved.scissor {
            context.RSSetScissorRects(Some(&[base]));
        }
    }
}

/// Per table slot: the viewport we moved (to put back), under one lock taken by HUD draws only.
static MOVED: Mutex<[Option<Moved>; CONTEXT_SLOTS]> = Mutex::new([None; CONTEXT_SLOTS]);

/// The frame's probe counts.
struct Frame {
    on_draws: u32,
    deferred_on_draws: u32,
}

static FRAME: Mutex<Frame> = Mutex::new(Frame { on_draws: 0, deferred_on_draws: 0 });
static FRAMES: AtomicU64 = AtomicU64::new(0);
static FRAMES_WITH_SCENE: AtomicU64 = AtomicU64::new(0);
static PLACED_DRAWS: AtomicU64 = AtomicU64::new(0);
static DEFERRED_PLACED: AtomicU64 = AtomicU64::new(0);
static PROBE_LINES: AtomicU32 = AtomicU32::new(0);

pub fn summary() -> String {
    format!(
        "HUD: {} frames, {} with a scene copy to the back buffer, {} HUD draws placed ({} on deferred contexts)",
        FRAMES.load(Relaxed),
        FRAMES_WITH_SCENE.load(Relaxed),
        PLACED_DRAWS.load(Relaxed),
        DEFERRED_PLACED.load(Relaxed)
    )
}

fn probing() -> bool {
    let Some(setup) = SETUP.get() else { return false };
    let frame = FRAMES.load(Relaxed);
    setup.probe_frames > 0 && (300..300 + setup.probe_frames as u64).contains(&frame)
}

/// At each present, before it: the next frame starts. Remembers the swapchain's back buffer and
/// gives the immediate context its own viewport back.
pub fn frame_end(chain: &IDXGISwapChain) {
    if SETUP.get().is_none() {
        return;
    }
    // SAFETY: COM calls on the game's live swapchain.
    unsafe {
        if let Ok(back) = chain.GetBuffer::<ID3D11Texture2D>(0) {
            BACKBUFFER.store(back.as_raw() as usize, Release);
        }
        if let Ok(device) = chain.GetDevice::<ID3D11Device>()
            && let Ok(context) = device.GetImmediateContext()
            && let Some(i) = context_slot(context.as_raw() as usize)
            && let Some(moved) = MOVED.lock().unwrap_or_else(|e| e.into_inner())[i].take()
        {
            put_back(&context, &moved);
        }
    }
    let mut frame = FRAME.lock().unwrap_or_else(|e| e.into_inner());
    let scene = SCENE_SEEN.swap(false, AcqRel);
    if probing() {
        log!(
            "HUD probe: frame {} ended: {} back-buffer draws ({} on deferred contexts), scene copy {scene}",
            FRAMES.load(Relaxed),
            frame.on_draws,
            frame.deferred_on_draws
        );
    }
    if scene {
        FRAMES_WITH_SCENE.fetch_add(1, Relaxed);
    }
    FRAMES.fetch_add(1, Relaxed);
    *frame = Frame { on_draws: 0, deferred_on_draws: 0 };
}

fn immediate(context: &ID3D11DeviceContext) -> bool {
    // SAFETY: COM call on a live context.
    unsafe { context.GetType() == D3D11_DEVICE_CONTEXT_IMMEDIATE }
}

/// `OMSetRenderTargets` on any context: records whether it now draws to the back buffer, and puts
/// back a viewport we moved when it leaves it.
fn before_targets(context: *mut c_void, count: u32, views: *const *mut c_void) {
    if !ACTIVE.load(Relaxed) {
        return;
    }
    // SAFETY: the game passes its live context and an array of `count` views.
    unsafe {
        let Some(ctx) = ID3D11DeviceContext::from_raw_borrowed(&context) else { return };
        let mut on = false;
        let backbuffer = BACKBUFFER.load(Acquire);
        if (1..=8).contains(&count) && !views.is_null() && backbuffer != 0 {
            let first = *views;
            if let Some(view) = ID3D11RenderTargetView::from_raw_borrowed(&first) {
                let resource: Option<ID3D11Resource> = view.GetResource().ok();
                on = resource.is_some_and(|r| r.as_raw() as usize == backbuffer);
            }
        }
        let Some(i) = context_slot(context as usize) else { return };
        let was_on = CONTEXT_ON[i].swap(on, AcqRel);
        if was_on
            && !on
            && let Some(moved) = MOVED.lock().unwrap_or_else(|e| e.into_inner())[i].take()
        {
            put_back(ctx, &moved);
        }
    }
}

/// Whether the bound blend state blends into target 0.
fn blending(context: &ID3D11DeviceContext) -> bool {
    let mut blend: Option<ID3D11BlendState> = None;
    let mut desc = D3D11_BLEND_DESC::default();
    // SAFETY: reads the bound blend state into locals.
    unsafe {
        context.OMGetBlendState(Some(&mut blend), None, None);
        if let Some(blend) = &blend {
            blend.GetDesc(&mut desc);
        }
    }
    desc.RenderTarget[0].BlendEnable.as_bool()
}

fn viewport(context: &ID3D11DeviceContext) -> Option<D3D11_VIEWPORT> {
    let mut count = 1u32;
    let mut viewport = D3D11_VIEWPORT::default();
    // SAFETY: reads one viewport into a local.
    unsafe { context.RSGetViewports(&mut count, Some(&mut viewport)) };
    (count > 0).then_some(viewport)
}

fn same(a: &D3D11_VIEWPORT, b: &D3D11_VIEWPORT) -> bool {
    (a.TopLeftX - b.TopLeftX).abs() < 0.01 && (a.TopLeftY - b.TopLeftY).abs() < 0.01 && (a.Width - b.Width).abs() < 0.01 && (a.Height - b.Height).abs() < 0.01
}

/// Before each draw on any context: places the HUD's draws.
fn before_draw(context: *mut c_void, vertices: u32, kind: &str) {
    if !ACTIVE.load(Relaxed) {
        return;
    }
    let Some(i) = drawing_to_backbuffer(context as usize) else { return };
    // SAFETY: the game passes its live context.
    let Some(ctx) = (unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }) else { return };
    let Some(setup) = SETUP.get() else { return };
    let deferred = !immediate(ctx);
    {
        let mut frame = FRAME.lock().unwrap_or_else(|e| e.into_inner());
        frame.on_draws += 1;
        if deferred {
            frame.deferred_on_draws += 1;
        }
        if probing() && PROBE_LINES.fetch_add(1, Relaxed) < 400 {
            log!(
                "HUD probe: back-buffer draw {} ({kind}, {vertices} vertices, {} context, blend {}) viewport {:?}",
                frame.on_draws,
                if deferred { "deferred" } else { "immediate" },
                blending(ctx),
                viewport(ctx).map(|v| (v.TopLeftX, v.TopLeftY, v.Width, v.Height))
            );
        }
        if setup.scene_copy {
            // The scene copy: an unblended full-screen triangle. It stays; the rest waits for one.
            if vertices == 3 && !blending(ctx) {
                SCENE_EVER.store(true, Release);
                SCENE_SEEN.store(true, Release);
                return;
            }
            if !SCENE_EVER.load(Acquire) {
                return;
            }
        }
    }
    let Some(at) = (setup.place)() else { return };
    let Some(current) = viewport(ctx) else { return };
    let mut moved = MOVED.lock().unwrap_or_else(|e| e.into_inner());
    // A viewport we did not set is the game's own: it becomes the base.
    let previous = moved[i];
    let base = match previous {
        Some(m) if same(&current, &m.placed) => m.base,
        _ => current,
    };
    let from = Rect::new(base.TopLeftX, base.TopLeftY, base.Width, base.Height);
    let rect = at.rect(from, from.aspect(), setup.scale);
    let placed = D3D11_VIEWPORT { TopLeftX: rect.x, TopLeftY: rect.y, Width: rect.width, Height: rect.height, ..base };
    // The scissor: one we did not set is the game's own, moved like the viewport.
    let scissor = scissor(ctx).map(|current| {
        let game = match previous.and_then(|m| m.scissor) {
            Some((game, ours)) if current == ours => game,
            _ => current,
        };
        (game, map_rect(&game, &from, &rect))
    });
    moved[i] = Some(Moved { base, placed, scissor });
    // SAFETY: sets one viewport and scissor on the context the game is drawing with, on its own thread.
    unsafe {
        ctx.RSSetViewports(Some(&[placed]));
        if let Some((_, ours)) = scissor {
            ctx.RSSetScissorRects(Some(&[ours]));
        }
    }
    PLACED_DRAWS.fetch_add(1, Relaxed);
    if deferred {
        DEFERRED_PLACED.fetch_add(1, Relaxed);
    }
}

/// The detours for one implementation of the context methods (the immediate and the deferred
/// context may run different code).
macro_rules! context_hooks {
    ($targets:ident, $di:ident, $d:ident, $dii:ident, $dinst:ident, $targets_fn:ident, $di_fn:ident, $d_fn:ident, $dii_fn:ident, $dinst_fn:ident) => {
        static $targets: Original<TargetsFn> = Original::new();
        static $di: Original<DrawIndexedFn> = Original::new();
        static $d: Original<DrawFn> = Original::new();
        static $dii: Original<DrawIndexedInstancedFn> = Original::new();
        static $dinst: Original<DrawInstancedFn> = Original::new();
        unsafe extern "system" fn $targets_fn(context: *mut c_void, count: u32, views: *const *mut c_void, depth: *mut c_void) {
            let _flight = InFlight::enter();
            before_targets(context, count, views);
            // SAFETY: forwards the game's own call.
            unsafe { $targets.get()(context, count, views, depth) }
        }
        unsafe extern "system" fn $di_fn(context: *mut c_void, count: u32, start: u32, base: i32) {
            let _flight = InFlight::enter();
            before_draw(context, count, "DrawIndexed");
            // SAFETY: forwards the game's own call.
            unsafe { $di.get()(context, count, start, base) }
        }
        unsafe extern "system" fn $d_fn(context: *mut c_void, count: u32, start: u32) {
            let _flight = InFlight::enter();
            before_draw(context, count, "Draw");
            // SAFETY: forwards the game's own call.
            unsafe { $d.get()(context, count, start) }
        }
        unsafe extern "system" fn $dii_fn(context: *mut c_void, count: u32, instances: u32, start: u32, base: i32, first: u32) {
            let _flight = InFlight::enter();
            before_draw(context, count, "DrawIndexedInstanced");
            // SAFETY: forwards the game's own call.
            unsafe { $dii.get()(context, count, instances, start, base, first) }
        }
        unsafe extern "system" fn $dinst_fn(context: *mut c_void, count: u32, instances: u32, start: u32, first: u32) {
            let _flight = InFlight::enter();
            before_draw(context, count, "DrawInstanced");
            // SAFETY: forwards the game's own call.
            unsafe { $dinst.get()(context, count, instances, start, first) }
        }
    };
}

context_hooks!(TARGETS_1, DRAW_INDEXED_1, DRAW_1, DRAW_INDEXED_INSTANCED_1, DRAW_INSTANCED_1, targets_1, draw_indexed_1, draw_1, draw_indexed_instanced_1, draw_instanced_1);
context_hooks!(TARGETS_2, DRAW_INDEXED_2, DRAW_2, DRAW_INDEXED_INSTANCED_2, DRAW_INSTANCED_2, targets_2, draw_indexed_2, draw_2, draw_indexed_instanced_2, draw_instanced_2);

const SLOTS: [usize; 5] = [
    slot::CONTEXT_OM_SET_RENDER_TARGETS,
    slot::CONTEXT_DRAW_INDEXED,
    slot::CONTEXT_DRAW,
    slot::CONTEXT_DRAW_INDEXED_INSTANCED,
    slot::CONTEXT_DRAW_INSTANCED,
];

fn methods(context: &ID3D11DeviceContext) -> Option<[usize; 5]> {
    let table = monaka_hook::mem::read::<usize>(context.as_raw() as usize)?;
    let mut out = [0; 5];
    for (o, s) in out.iter_mut().zip(SLOTS) {
        *o = monaka_hook::mem::read::<usize>(table + s * 8)?;
    }
    Some(out)
}

/// Hooks one implementation of the five methods (set 0 or 1), through `context`, one of our own.
///
/// # Safety
/// `context` is a live D3D11 context whose slots have the detours' types.
unsafe fn hook_set(hooks: &mut Hooks, set: usize, context: &ID3D11DeviceContext, label: &str) -> Result<usize, Rejection> {
    let raw = context.as_raw();
    let name = |method: &str| format!("{method} ({label})");
    // SAFETY: the caller's guarantee.
    let placed = unsafe {
        if set == 0 {
            [
                hooks.method(&TARGETS_1, &name("OMSetRenderTargets"), raw, SLOTS[0], targets_1 as TargetsFn)?,
                hooks.method(&DRAW_INDEXED_1, &name("DrawIndexed"), raw, SLOTS[1], draw_indexed_1 as DrawIndexedFn)?,
                hooks.method(&DRAW_1, &name("Draw"), raw, SLOTS[2], draw_1 as DrawFn)?,
                hooks.method(&DRAW_INDEXED_INSTANCED_1, &name("DrawIndexedInstanced"), raw, SLOTS[3], draw_indexed_instanced_1 as DrawIndexedInstancedFn)?,
                hooks.method(&DRAW_INSTANCED_1, &name("DrawInstanced"), raw, SLOTS[4], draw_instanced_1 as DrawInstancedFn)?,
            ]
        } else {
            [
                hooks.method(&TARGETS_2, &name("OMSetRenderTargets"), raw, SLOTS[0], targets_2 as TargetsFn)?,
                hooks.method(&DRAW_INDEXED_2, &name("DrawIndexed"), raw, SLOTS[1], draw_indexed_2 as DrawIndexedFn)?,
                hooks.method(&DRAW_2, &name("Draw"), raw, SLOTS[2], draw_2 as DrawFn)?,
                hooks.method(&DRAW_INDEXED_INSTANCED_2, &name("DrawIndexedInstanced"), raw, SLOTS[3], draw_indexed_instanced_2 as DrawIndexedInstancedFn)?,
                hooks.method(&DRAW_INSTANCED_2, &name("DrawInstanced"), raw, SLOTS[4], draw_instanced_2 as DrawInstancedFn)?,
            ]
        }
    };
    let mut in_vtable = 0;
    for p in placed {
        if let Placement::Vtable(why) = p {
            in_vtable += 1;
            log!("HUD: {why}; hooked in the vtable of our own {label} context, which the game's may not share");
        }
    }
    Ok(5 - in_vtable)
}

/// Prepares the hooks (enabled with the producer's others; draws are placed only while
/// [`ACTIVE`]). `scale` is the HUD's size as a share of the image; `scene_copy` waits for the
/// game's scene copy draw before placing (see the module notes); `place` says where each frame's
/// HUD goes (`None` leaves it alone); `probe_frames` logs that many frames' back-buffer draws.
pub fn install(hooks: &mut Hooks, scale: f32, scene_copy: bool, place: fn() -> Option<HudPlacement>, probe_frames: u32) -> Result<(), Rejection> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    // SAFETY: a device of our own, only to find the contexts' methods; released at return.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|e| Rejection::unsupported(format!("could not make a D3D11 device to find the draw methods: {e}")))?;
    let device = device.ok_or_else(|| Rejection::unsupported("no device"))?;
    let context = context.ok_or_else(|| Rejection::unsupported("no immediate context"))?;
    let mut deferred: Option<ID3D11DeviceContext> = None;
    // SAFETY: a deferred context of our own device.
    unsafe { device.CreateDeferredContext(0, Some(&mut deferred)) }.map_err(|e| Rejection::unsupported(format!("no deferred context: {e}")))?;
    let deferred = deferred.ok_or_else(|| Rejection::unsupported("no deferred context"))?;
    let (immediate_methods, deferred_methods) = (methods(&context), methods(&deferred));
    // SAFETY: both are live contexts of our own device, whose slots have the detours' types.
    let in_code = unsafe {
        let mut n = hook_set(hooks, 0, &context, "immediate")?;
        if deferred_methods.is_some() && deferred_methods != immediate_methods {
            n += hook_set(hooks, 1, &deferred, "deferred")?;
        } else {
            log!("HUD: the deferred context runs the immediate context's methods");
        }
        n
    };
    SETUP.set(Setup { scale, scene_copy, place, probe_frames }).map_err(|_| Rejection::unsupported("HUD installed twice"))?;
    log!("HUD: D3D11 targets and draws hooked ({in_code} in code), scale {scale}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scissor_moves_with_its_viewport() {
        let from = Rect::sized(3840.0, 2160.0);
        let wide = monaka_core::camera::Frustum { left: -16.0 / 9.0, right: 16.0 / 9.0, up: 1.0, down: -1.0 };
        let to = HudPlacement::new(wide, 0, 0.0, 2.0).rect(from, from.aspect(), 0.5);
        // A text box right of the centre: halved, around the centre.
        let text = RECT { left: 2000, top: 1100, right: 2400, bottom: 1200 };
        assert_eq!(map_rect(&text, &from, &to), RECT { left: 1960, top: 1090, right: 2160, bottom: 1140 });
        let whole = RECT { left: 0, top: 0, right: 3840, bottom: 2160 };
        assert_eq!(map_rect(&whole, &from, &to), RECT { left: 960, top: 540, right: 2880, bottom: 1620 });
    }
}
