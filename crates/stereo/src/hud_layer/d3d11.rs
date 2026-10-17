//! A D3D11 game's HUD and menus placed in front of each eye. The rule (as Dying Light draws): the
//! finished 3D image reaches the back buffer as one unblended full-screen triangle, and every later
//! draw to the back buffer
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
//! context's vtable while a game runs, which would undo a vtable hook). Each frame's HUD goes
//! where the game's frame labels put it ([`FrameLabels`]: the eye and pose of the frame being
//! drawn, the eye's frustum and the eyes' separation) at the HUD's distance ([`super::place_eye`]); the
//! game calls [`frame_end`] at each present.
//!
//! A game that draws its frame on its immediate context alone can have more ([`install_with`], a
//! [`Setup`]): only that context followed (its own methods hooked), each frame waiting for its own
//! scene copy ([`SceneCopy::EachFrame`]), the scissor and the viewport's give-back left to the game,
//! and its [`Handlers`] around the draws: probes, a HUD draw sent into a layer of its own instead of
//! placed ([`Route::Layer`], drawn by [`draw_into_layer`]), the HUD laid out in part of its
//! viewport, and the draw itself run its own way (pieces of the HUD drawn elsewhere).

use crate::providers::FrameLabels;
use monaka_core::options::Hud;
use monaka_contract::protocol::HeadPose;
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
use monaka_core::sync::Locked;

type DrawIndexedFn = unsafe extern "system" fn(*mut c_void, u32, u32, i32);
type DrawFn = unsafe extern "system" fn(*mut c_void, u32, u32);
type DrawIndexedInstancedFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32, i32, u32);
type DrawInstancedFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32, u32);
type TargetsFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void, *mut c_void);

/// Set while HUD draws are placed.
pub static ACTIVE: AtomicBool = AtomicBool::new(false);

/// How a frame's HUD draws are told from its world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneCopy {
    /// Every back-buffer draw is HUD (the 3D image reaches the back buffer some other way).
    Off,
    /// Once one scene copy has been seen, every other back-buffer draw is HUD (contexts record in
    /// any order, see the module notes).
    Once,
    /// Each frame waits for its own scene copy: the draws before it, and frames without one
    /// (menus, loading), are left alone, and each present forgets which context drew to the back
    /// buffer.
    EachFrame,
}

/// A draw call as the game made it.
#[derive(Clone, Copy, Debug)]
pub struct Draw {
    pub kind: &'static str,
    /// `DrawIndexed` or `DrawIndexedInstanced`.
    pub indexed: bool,
    /// The vertex or index count, the first vertex or index, and the base vertex (0 unindexed).
    pub count: u32,
    pub start: u32,
    pub base: i32,
}

/// Where a HUD draw goes.
#[derive(Clone)]
pub enum Route {
    /// Placed in front of the eye.
    Place,
    /// Drawn as the game made it.
    Leave,
    /// Drawn at the game's own layout into each of a layer's two targets (black and white), the
    /// layer laid over each eye later, instead of placed.
    Layer([ID3D11RenderTargetView; 2]),
}

/// Every draw on a followed context, first (counters, probes).
pub type DrawHandler = fn(*mut c_void, &Draw);
/// `OMSetRenderTargets` on a followed context: whether it now draws to the back buffer, then the
/// call's count, views and depth view.
pub type TargetsHandler = fn(&ID3D11DeviceContext, bool, u32, *const *mut c_void, *mut c_void);
/// Each back-buffer draw before it is told apart: its vertex count, and whether this frame's scene
/// copy has been seen.
pub type BackbufferHandler = fn(&ID3D11DeviceContext, u32, bool);
/// A HUD draw (its vertex count): where it goes.
pub type HudDrawHandler = fn(&ID3D11DeviceContext, u32) -> Route;
/// The part of the game's viewport `base` the HUD is laid out in (`bound`: the viewport bound
/// now); placed as the whole viewport would be.
pub type LayoutHandler = fn(&ID3D11DeviceContext, D3D11_VIEWPORT, D3D11_VIEWPORT) -> D3D11_VIEWPORT;
/// Runs a draw (`draw`, the game's call) with its layer when it has one; by default once, or into
/// the layer ([`draw_into_layer`]).
pub type RunHandler = fn(*mut c_void, Option<&[ID3D11RenderTargetView; 2]>, &dyn Fn());

/// What a game adds around its HUD's draws; each is optional.
#[derive(Clone, Copy, Default)]
pub struct Handlers {
    /// Whether the draws are followed now (besides [`ACTIVE`]).
    pub following: Option<fn() -> bool>,
    pub draw: Option<DrawHandler>,
    pub targets: Option<TargetsHandler>,
    pub backbuffer_draw: Option<BackbufferHandler>,
    /// The scene copy, seen.
    pub scene_copied: Option<fn(&ID3D11DeviceContext)>,
    pub hud_draw: Option<HudDrawHandler>,
    pub layout: Option<LayoutHandler>,
    /// A HUD draw placed, with the layout it was placed by (the game's viewport, or the part
    /// [`Handlers::layout`] gave).
    pub placed: Option<fn(D3D11_VIEWPORT)>,
    pub run: Option<RunHandler>,
}

/// How the HUD is followed and placed.
#[derive(Clone, Copy)]
pub struct Setup {
    /// The HUD's size as a share of the image.
    pub scale: f32,
    /// How far away the HUD floats, metres; `None`: the draws are followed (for the handlers) but
    /// left where the game puts them.
    pub distance: Option<f32>,
    pub scene_copy: SceneCopy,
    /// Logs that many frames' back-buffer draws.
    pub probe_frames: u32,
    /// Each draw's scissor rectangle moves with its viewport.
    pub move_scissor: bool,
    /// A context that leaves the back buffer gets its own viewport back at once (otherwise at
    /// the present).
    pub give_back_on_leave: bool,
    pub handlers: Handlers,
}

impl Setup {
    /// The HUD placed at `scale` and `distance`, scissors moved along, viewports given back when a
    /// context leaves the back buffer, no handlers.
    pub const fn new(scale: f32, distance: Option<f32>, scene_copy: SceneCopy) -> Self {
        Self { scale, distance, scene_copy, probe_frames: 0, move_scissor: true, give_back_on_leave: true, handlers: Handlers { following: None, draw: None, targets: None, backbuffer_draw: None, scene_copied: None, hud_draw: None, layout: None, placed: None, run: None } }
    }
}

struct Installed {
    setup: Setup,
    /// Only the game's immediate context is followed.
    immediate_only: bool,
    /// Where the HUD of the frame being drawn goes at a distance ([`placement`] of the game's
    /// frame labels).
    place: fn(f32) -> Option<HudPlacement>,
}

static SETUP: OnceLock<Installed> = OnceLock::new();
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
/// A scene copy was seen since the last present (for the summary, and [`SceneCopy::EachFrame`]).
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

/// The viewport of a HUD laid out in `layout`, part of the game's viewport `base`, when `base` is
/// placed at `rect`: the part keeps its place in the whole.
fn laid_out(base: D3D11_VIEWPORT, layout: D3D11_VIEWPORT, rect: Rect) -> D3D11_VIEWPORT {
    let (sx, sy) = (rect.width / base.Width, rect.height / base.Height);
    D3D11_VIEWPORT {
        TopLeftX: rect.x + (layout.TopLeftX - base.TopLeftX) * sx,
        TopLeftY: rect.y + (layout.TopLeftY - base.TopLeftY) * sy,
        Width: layout.Width * sx,
        Height: layout.Height * sy,
        ..base
    }
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
    let Some(installed) = SETUP.get() else { return false };
    let frame = FRAMES.load(Relaxed);
    installed.setup.probe_frames > 0 && (300..300 + installed.setup.probe_frames as u64).contains(&frame)
}

/// At each present, before it: the next frame starts. Remembers the swapchain's back buffer and
/// gives the immediate context its own viewport back.
pub fn frame_end(chain: &IDXGISwapChain) {
    let Some(installed) = SETUP.get() else { return };
    // SAFETY: COM calls on the game's live swapchain.
    unsafe {
        if let Ok(back) = chain.GetBuffer::<ID3D11Texture2D>(0) {
            BACKBUFFER.store(back.as_raw() as usize, Release);
        }
        if let Ok(device) = chain.GetDevice::<ID3D11Device>()
            && let Ok(context) = device.GetImmediateContext()
            && let Some(i) = context_slot(context.as_raw() as usize)
            && let Some(moved) = MOVED.locked()[i].take()
        {
            put_back(&context, &moved);
        }
    }
    let mut frame = FRAME.locked();
    let scene = SCENE_SEEN.swap(false, AcqRel);
    if installed.setup.scene_copy == SceneCopy::EachFrame {
        for on in &CONTEXT_ON {
            on.store(false, Release);
        }
    }
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

/// Whether `context`'s draws are followed.
fn followed(installed: &Installed, context: &ID3D11DeviceContext) -> bool {
    !installed.immediate_only || immediate(context)
}

/// `OMSetRenderTargets` on any context: records whether it now draws to the back buffer, and puts
/// back a viewport we moved when it leaves it.
fn before_targets(context: *mut c_void, count: u32, views: *const *mut c_void, depth: *mut c_void) {
    crate::debug_depth::bound(context, count, views, depth);
    if !ACTIVE.load(Relaxed) {
        return;
    }
    let Some(installed) = SETUP.get() else { return };
    let setup = &installed.setup;
    if setup.handlers.following.is_some_and(|following| !following()) {
        return;
    }
    // SAFETY: the game passes its live context and an array of `count` views.
    unsafe {
        let Some(ctx) = ID3D11DeviceContext::from_raw_borrowed(&context) else { return };
        if !followed(installed, ctx) {
            return;
        }
        let mut on = false;
        let backbuffer = BACKBUFFER.load(Acquire);
        if (1..=8).contains(&count) && !views.is_null() && backbuffer != 0 {
            let first = *views;
            if let Some(view) = ID3D11RenderTargetView::from_raw_borrowed(&first) {
                let resource: Option<ID3D11Resource> = view.GetResource().ok();
                on = resource.is_some_and(|r| r.as_raw() as usize == backbuffer);
            }
        }
        if let Some(handle) = setup.handlers.targets {
            handle(ctx, on, count, views, depth);
        }
        let Some(i) = context_slot(context as usize) else { return };
        let was_on = CONTEXT_ON[i].swap(on, AcqRel);
        if setup.give_back_on_leave
            && was_on
            && !on
            && let Some(moved) = MOVED.locked()[i].take()
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

/// Before each draw on any context: places the HUD's draws. Returns the layer's targets when the
/// game routes this draw into one instead ([`Route::Layer`]).
fn before_draw(context: *mut c_void, draw: &Draw) -> Option<[ID3D11RenderTargetView; 2]> {
    crate::debug_depth::drew(context);
    let installed = SETUP.get()?;
    let setup = &installed.setup;
    if let Some(handle) = setup.handlers.draw {
        handle(context, draw);
    }
    if !ACTIVE.load(Relaxed) || setup.handlers.following.is_some_and(|following| !following()) {
        return None;
    }
    let (vertices, kind) = (draw.count, draw.kind);
    let i = drawing_to_backbuffer(context as usize)?;
    // SAFETY: the game passes its live context.
    let ctx = unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }?;
    let deferred = !immediate(ctx);
    if deferred && installed.immediate_only {
        return None;
    }
    {
        let mut frame = FRAME.locked();
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
        if let Some(handle) = setup.handlers.backbuffer_draw {
            handle(ctx, vertices, SCENE_SEEN.load(Acquire));
        }
        match setup.scene_copy {
            SceneCopy::Off => {}
            SceneCopy::Once => {
                // The scene copy: an unblended full-screen triangle. It stays; the rest waits for one.
                if vertices == 3 && !blending(ctx) {
                    SCENE_EVER.store(true, Release);
                    SCENE_SEEN.store(true, Release);
                    if let Some(handle) = setup.handlers.scene_copied {
                        handle(ctx);
                    }
                    return None;
                }
                if !SCENE_EVER.load(Acquire) {
                    return None;
                }
            }
            SceneCopy::EachFrame => {
                // Until this frame's scene copy, nothing is HUD; the copy itself stays.
                if !SCENE_SEEN.load(Acquire) {
                    if vertices == 3 && !blending(ctx) {
                        SCENE_SEEN.store(true, Release);
                        if let Some(handle) = setup.handlers.scene_copied {
                            handle(ctx);
                        }
                    }
                    return None;
                }
            }
        }
    }
    if let Some(handle) = setup.handlers.hud_draw {
        match handle(ctx, vertices) {
            Route::Place => {}
            Route::Leave => return None,
            Route::Layer(targets) => return Some(targets),
        }
    }
    let at = setup.distance.and_then(installed.place)?;
    let current = viewport(ctx)?;
    let mut moved = MOVED.locked();
    // A viewport we did not set is the game's own: it becomes the base.
    let previous = moved[i];
    let base = match previous {
        Some(m) if same(&current, &m.placed) => m.base,
        _ => current,
    };
    let from = Rect::new(base.TopLeftX, base.TopLeftY, base.Width, base.Height);
    let rect = at.rect(from, from.aspect(), setup.scale);
    let (placed, layout) = match setup.handlers.layout {
        None => (D3D11_VIEWPORT { TopLeftX: rect.x, TopLeftY: rect.y, Width: rect.width, Height: rect.height, ..base }, base),
        Some(layout) => {
            let layout = layout(ctx, current, base);
            (laid_out(base, layout, rect), layout)
        }
    };
    // The scissor: one we did not set is the game's own, moved like the viewport.
    let scissor = scissor(ctx).filter(|_| setup.move_scissor).map(|current| {
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
    drop(moved);
    PLACED_DRAWS.fetch_add(1, Relaxed);
    if deferred {
        DEFERRED_PLACED.fetch_add(1, Relaxed);
    }
    if let Some(handle) = setup.handlers.placed {
        handle(layout);
    }
    None
}

/// Runs the game's draw: the game's own way ([`Handlers::run`]), or once, or into the layer.
fn run(context: *mut c_void, layer: Option<[ID3D11RenderTargetView; 2]>, draw: &dyn Fn()) {
    if let Some(run) = SETUP.get().and_then(|installed| installed.setup.handlers.run) {
        return run(context, layer.as_ref(), draw);
    }
    match &layer {
        // SAFETY: the context the game is drawing with, in its hooked call.
        Some(targets) if unsafe { draw_into_layer(context, targets, draw) } => {}
        _ => draw(),
    }
}

/// One draw into each of a layer's two targets (black and white), then the game's own binding
/// back, both bound past the hook ([`set_targets_unhooked`]). False (nothing drawn) without a
/// context.
///
/// # Safety
/// `context` is the game's live context drawing now, its methods hooked here.
pub unsafe fn draw_into_layer(context: *mut c_void, targets: &[ID3D11RenderTargetView; 2], draw: &dyn Fn()) -> bool {
    // SAFETY: the game's live context (the caller's guarantee); reads and restores its own
    // bindings around our two draws.
    unsafe {
        let Some(ctx) = ID3D11DeviceContext::from_raw_borrowed(&context) else { return false };
        let mut bound = [None];
        let mut depth = None;
        ctx.OMGetRenderTargets(Some(&mut bound), Some(&mut depth));
        let depth_raw = depth.as_ref().map_or(std::ptr::null_mut(), |d| d.as_raw());
        for target in targets {
            let view = [target.as_raw()];
            set_targets_unhooked(context, 1, view.as_ptr(), depth_raw);
            draw();
        }
        let game = [bound[0].as_ref().map_or(std::ptr::null_mut(), |v| v.as_raw())];
        set_targets_unhooked(context, 1, game.as_ptr(), depth_raw);
    }
    true
}

static TARGETS: [Original<TargetsFn>; 2] = [Original::new(), Original::new()];
static DRAW_INDEXED: [Original<DrawIndexedFn>; 2] = [Original::new(), Original::new()];
static DRAW: [Original<DrawFn>; 2] = [Original::new(), Original::new()];
static DRAW_INDEXED_INSTANCED: [Original<DrawIndexedInstancedFn>; 2] = [Original::new(), Original::new()];
static DRAW_INSTANCED: [Original<DrawInstancedFn>; 2] = [Original::new(), Original::new()];

/// `OMSetRenderTargets` on `context` past the hook: a binding of the producer's own, which the HUD's
/// tracking does not see.
///
/// # Safety
/// `context` is a live context whose methods are hooked here, and `views` holds `count` views.
pub unsafe fn set_targets_unhooked(context: *mut c_void, count: u32, views: *const *mut c_void, depth: *mut c_void) {
    let flight = monaka_hook::InFlight::enter();
    // A deferred context runs the second set of methods when it has its own.
    // SAFETY: the caller's guarantee.
    let deferred = unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }.is_some_and(|c| !immediate(c));
    let set = if deferred && TARGETS[1].is_set() { 1 } else { 0 };
    // Without the HUD layer's hooks (a game drawing only the dynamic HUD's panels) the method is
    // the context's own.
    if !TARGETS[set].is_set() {
        // SAFETY: the caller's guarantee.
        let Some(ctx) = (unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }) else { return };
        // SAFETY: the method of the context's own vtable, with the caller's arguments.
        return unsafe { (Interface::vtable(ctx).OMSetRenderTargets)(context, count, views, depth) };
    }
    // SAFETY: the original method, with the caller's arguments.
    unsafe { TARGETS[set].get(&flight)(context, count, views, depth) }
}

// The detours for one implementation of the context methods (`SET` 0 or 1: the immediate and the
// deferred context may run different code).

unsafe extern "system" fn targets<const SET: usize>(context: *mut c_void, count: u32, views: *const *mut c_void, depth: *mut c_void) {
    let flight = InFlight::enter();
    before_targets(context, count, views, depth);
    // SAFETY: forwards the game's own call.
    unsafe { TARGETS[SET].get(&flight)(context, count, views, depth) }
}

unsafe extern "system" fn draw_indexed<const SET: usize>(context: *mut c_void, count: u32, start: u32, base: i32) {
    let flight = InFlight::enter();
    let layer = before_draw(context, &Draw { kind: "DrawIndexed", indexed: true, count, start, base });
    // SAFETY: forwards the game's own call.
    run(context, layer, &|| unsafe { DRAW_INDEXED[SET].get(&flight)(context, count, start, base) });
}

unsafe extern "system" fn draw<const SET: usize>(context: *mut c_void, count: u32, start: u32) {
    let flight = InFlight::enter();
    let layer = before_draw(context, &Draw { kind: "Draw", indexed: false, count, start, base: 0 });
    // SAFETY: forwards the game's own call.
    run(context, layer, &|| unsafe { DRAW[SET].get(&flight)(context, count, start) });
}

unsafe extern "system" fn draw_indexed_instanced<const SET: usize>(context: *mut c_void, count: u32, instances: u32, start: u32, base: i32, first: u32) {
    let flight = InFlight::enter();
    let layer = before_draw(context, &Draw { kind: "DrawIndexedInstanced", indexed: true, count, start, base });
    // SAFETY: forwards the game's own call.
    run(context, layer, &|| unsafe { DRAW_INDEXED_INSTANCED[SET].get(&flight)(context, count, instances, start, base, first) });
}

unsafe extern "system" fn draw_instanced<const SET: usize>(context: *mut c_void, count: u32, instances: u32, start: u32, first: u32) {
    let flight = InFlight::enter();
    let layer = before_draw(context, &Draw { kind: "DrawInstanced", indexed: false, count, start, base: 0 });
    // SAFETY: forwards the game's own call.
    run(context, layer, &|| unsafe { DRAW_INSTANCED[SET].get(&flight)(context, count, instances, start, first) });
}

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

/// The detours of set `SET` for the five methods, through `context`.
///
/// # Safety
/// `context` is a live D3D11 context whose slots have the detours' types.
unsafe fn hook_methods<const SET: usize>(hooks: &mut Hooks, context: *mut c_void, name: impl Fn(&str) -> String) -> Result<[Placement; 5], Rejection> {
    // SAFETY: the caller's guarantee.
    unsafe {
        Ok([
            hooks.method(&TARGETS[SET], &name("OMSetRenderTargets"), context, SLOTS[0], targets::<SET> as TargetsFn)?,
            hooks.method(&DRAW_INDEXED[SET], &name("DrawIndexed"), context, SLOTS[1], draw_indexed::<SET> as DrawIndexedFn)?,
            hooks.method(&DRAW[SET], &name("Draw"), context, SLOTS[2], draw::<SET> as DrawFn)?,
            hooks.method(&DRAW_INDEXED_INSTANCED[SET], &name("DrawIndexedInstanced"), context, SLOTS[3], draw_indexed_instanced::<SET> as DrawIndexedInstancedFn)?,
            hooks.method(&DRAW_INSTANCED[SET], &name("DrawInstanced"), context, SLOTS[4], draw_instanced::<SET> as DrawInstancedFn)?,
        ])
    }
}

/// Hooks one implementation of the five methods (set 0 or 1), through `context`, one of our own
/// (`label` says which), or the game's own (`label` none).
///
/// # Safety
/// `context` is a live D3D11 context whose slots have the detours' types.
unsafe fn hook_set(hooks: &mut Hooks, set: usize, context: &ID3D11DeviceContext, label: Option<&str>) -> Result<usize, Rejection> {
    let raw = context.as_raw();
    let name = |method: &str| match label {
        Some(label) => format!("{method} ({label})"),
        None => method.to_owned(),
    };
    // SAFETY: the caller's guarantee.
    let placed = unsafe { if set == 0 { hook_methods::<0>(hooks, raw, name)? } else { hook_methods::<1>(hooks, raw, name)? } };
    let mut in_vtable = 0;
    for p in placed {
        if let Placement::Vtable(why) = p {
            in_vtable += 1;
            match label {
                Some(label) => log!("HUD: {why}; hooked in the vtable of our own {label} context, which the game's may not share"),
                None => log!("{why}; hooking its vtable slot instead"),
            }
        }
    }
    Ok(5 - in_vtable)
}

/// Hooks the methods found on an immediate and a deferred context of a device of our own.
fn hook_own_contexts(hooks: &mut Hooks) -> Result<usize, Rejection> {
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
    unsafe {
        let mut n = hook_set(hooks, 0, &context, Some("immediate"))?;
        if deferred_methods.is_some() && deferred_methods != immediate_methods {
            n += hook_set(hooks, 1, &deferred, Some("deferred"))?;
        } else {
            log!("HUD: the deferred context runs the immediate context's methods");
        }
        Ok(n)
    }
}

/// Prepares the hooks for game `G`'s HUD, placed by its frame labels (enabled with the producer's
/// others; draws are placed only while [`ACTIVE`]). `hud` gives the HUD's size as a share of the
/// image and its distance; `scene_copy` waits for the game's scene copy draw before placing (see
/// the module notes); `probe_frames` logs that many frames' back-buffer draws.
pub fn install<G: FrameLabels>(hooks: &mut Hooks, hud: &Hud, scene_copy: bool, probe_frames: u32) -> Result<(), Rejection> {
    let scene_copy = if scene_copy { SceneCopy::Once } else { SceneCopy::Off };
    install_with::<G>(hooks, Setup { probe_frames, ..Setup::new(hud.scale, Some(hud.distance), scene_copy) }, None)
}

/// [`install`] as `setup` says. With `game_context` (the game's immediate context), only that
/// context's draws are followed, through its own methods; otherwise every context's, through the
/// methods of a device of our own.
pub fn install_with<G: FrameLabels>(hooks: &mut Hooks, setup: Setup, game_context: Option<&ID3D11DeviceContext>) -> Result<(), Rejection> {
    let in_code = match game_context {
        // SAFETY: the game's live immediate context, whose slots have the detours' types.
        Some(context) => unsafe { hook_set(hooks, 0, context, None)? },
        None => hook_own_contexts(hooks)?,
    };
    let scale = setup.scale;
    SETUP.set(Installed { setup, immediate_only: game_context.is_some(), place: placement::<G> }).map_err(|_| Rejection::unsupported("HUD installed twice"))?;
    log!("HUD: D3D11 targets and draws hooked ({in_code} in code), scale {scale}");
    Ok(())
}

/// Where the HUD of the frame being drawn goes, `distance` metres away: in its eye, by `G`'s frame
/// labels.
fn placement<G: FrameLabels>(distance: f32) -> Option<HudPlacement> {
    let frame = G::labelled()?;
    super::place_eye::<G>(&frame.pose, frame.eye, distance)
}

/// Where the HUD goes in each eye of a frame drawn from `pose`, as (x, y, width, height) in pixels
/// of a `width` x `height` image, as its draws are placed (for a HUD layer laid over each eye): the
/// whole image while the HUD is not placed (not installed, or no distance) or an eye has no place.
pub fn eye_rects<G: FrameLabels>(pose: &HeadPose, width: u32, height: u32) -> [[f32; 4]; 2] {
    let setup = SETUP.get().map(|installed| &installed.setup);
    rects::<G>(setup.and_then(|s| s.distance.map(|distance| (s.scale, distance))), pose, width, height)
}

/// [`eye_rects`] with the HUD's scale and distance (`None`: not placed).
fn rects<G: FrameLabels>(hud: Option<(f32, f32)>, pose: &HeadPose, width: u32, height: u32) -> [[f32; 4]; 2] {
    let base = Rect::sized(width as f32, height as f32);
    std::array::from_fn(|eye| match hud.and_then(|(scale, distance)| Some((scale, super::place_eye::<G>(pose, eye, distance)?))) {
        Some((scale, at)) => at.rect(base, base.aspect(), scale).array(),
        None => base.array(),
    })
}

#[cfg(test)]
mod tests;
