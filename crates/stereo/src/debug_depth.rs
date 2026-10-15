//! The game's depth for the debug views, for a D3D11 game whose pairs carry none: a ready-made
//! component ([`debug_depth`]) any D3D11 adapter may list. It knows nothing of the game: the scene's
//! depth is the largest single-sample buffer the game clears that is not square (shadow maps are)
//! unless it is the picture's size, on any context (immediate or deferred); whether depth 0 or 1
//! is near comes from the clear value. A game may reuse its depth in a frame (the scene, then the
//! HUD), clearing it between: the viewer picks which clear the depth is copied just before
//! ([`debug::wanted_pass`]; 1 the first, so the scene's), on the clearing context in its order, or
//! what it holds at the present (0). At each present ([`at_present`], from the driver) the copy is
//! turned into 32-bit floats and shared ([`monaka_channel::debug_image`]).
//!
//! A game may never clear its scene's depth (its first pass writing every texel instead). Where
//! the HUD layer follows the game's draws ([`crate::hud_layer::d3d11`]), it tells this which depth each
//! context draws with ([`bound`], [`drew`]), as does this component's own hook of
//! `OMSetRenderTargetsAndUnorderedAccessViews` (which the HUD layer does not follow): the scene's
//! depth is then the one most draws use in a frame (weighed by its size), whatever is cleared. The
//! depths drawn with are listed for the viewer ([`debug::share_candidates`], and in the log), which
//! may pick one to share instead ([`debug::wanted_buffer`]).
//!
//! Idle (the viewer shows no depth view) the hooks cost a flag check per clear, bind, draw and
//! present. While wanted, each new kind of depth clear is logged once (a few at most).
//!
//! The game's textures followed are held only while a view wants them and the picture keeps its
//! size, and never past the stop's first phase: a swapchain resize fails while anything holds a
//! back buffer (the game's own, and the eye size's put back in the stop), and a game quits on it.

use monaka_channel::ChannelName;
use monaka_channel::d3d12::compute::{blob_bytes, compile};
use monaka_channel::debug_image::ImageWriter;
use monaka_contract::debug_draw::{Category, DEPTH_CANDIDATES, DebugDepth, DepthCandidate};
use monaka_core::sync::Locked;
use monaka_hook::Original;
use monaka_producer::game::{Component, Game, Lack, Start};
use monaka_producer::{Rejection, debug, log};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use windows::Win32::Graphics::Direct3D::D3D_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, s};

#[cfg(test)]
mod tests;

type ClearDepthFn = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, f32, u8);
type BindWithViewsFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void, *mut c_void, u32, u32, *const *mut c_void, *const u32);

/// `ID3D11DeviceContext::ClearDepthStencilView`'s vtable slot.
const CLEAR_DEPTH_SLOT: usize = 53;
/// `ID3D11DeviceContext::OMSetRenderTargetsAndUnorderedAccessViews`'s.
const BIND_WITH_VIEWS_SLOT: usize = 34;
/// Its count meaning "the render targets and depth stay as bound".
const KEEP_TARGETS: u32 = 0xffff_ffff;
const SOURCE: &str = include_str!("debug_depth.hlsl");

/// The clear of immediate contexts, and of deferred ones where their code is another.
static ORIGINAL: Original<ClearDepthFn> = Original::new();
static ORIGINAL_DEFERRED: Original<ClearDepthFn> = Original::new();
static BIND_ORIGINAL: Original<BindWithViewsFn> = Original::new();
static BIND_ORIGINAL_DEFERRED: Original<BindWithViewsFn> = Original::new();
static STATE: Mutex<Option<State>> = Mutex::new(None);
static SHARED: AtomicU64 = AtomicU64::new(0);
static CLEARS: AtomicU64 = AtomicU64::new(0);
/// Clears of the scene's depth since the last present (each starts a pass), and the pass copied
/// in this frame (0: none).
static PASSES: AtomicU32 = AtomicU32::new(0);
static COPIED: AtomicU32 = AtomicU32::new(0);
/// Frames the most-drawn depth was another texture than the frame before (a game cycling several).
static SWITCHES: AtomicU64 = AtomicU64::new(0);
/// Kinds of depth clear logged (at most [`LOGGED_KINDS`]), and tables of the depths drawn with.
const LOGGED_KINDS: usize = 12;
const LOGGED_TABLES: u32 = 10;
static TABLES: AtomicU32 = AtomicU32::new(0);
/// The list goes to the viewer every this many presents.
const LIST_EVERY: u32 = 10;

/// The depth sharing for game `G`'s D3D11 runs.
pub const fn debug_depth<G: Game>() -> Component<G> {
    Component { name: "debug depth", needs_channel: true, install: install::<G>, quiet: release, release, summary: Some(summary), ..Component::NONE }
}

struct State {
    channel: ChannelName,
    /// The size of the game's picture (its swapchain's), as of the last present.
    view: (u32, u32),
    /// The scene's depth buffer (the largest seen), its area, and whether its clear made it
    /// reversed (cleared to 0; none: never seen cleared).
    scene: Option<(ID3D11Texture2D, u64, Option<bool>)>,
    made: Option<Made>,
    generation: u32,
    /// Kinds of depth clear seen (size, format, samples, layers, deferred), for the log.
    kinds: Vec<(u32, u32, i32, u32, u32, bool)>,
    /// The depth each context draws with now (context, texture), and the depths drawn with this
    /// frame, with their draws.
    bound: Vec<(usize, usize)>,
    drawn: Vec<Drawn>,
    /// The shape (size, format) of the most-drawn depth last taken as the scene's.
    most_drawn: Option<(u32, u32, i32)>,
    /// Depths seen cleared, and whether to 0 (reversed), by texture.
    cleared: Vec<(usize, bool)>,
    /// Presents since wanted (for the list and the table), and the depths listed in the last table.
    frames: u32,
    tabled: usize,
}

/// A depth drawn with, in the order first seen: its texture, its shape, its draws this frame and
/// lately (a running average).
struct Drawn {
    texture: ID3D11Texture2D,
    desc: D3D11_TEXTURE2D_DESC,
    draws: u64,
    average: f32,
}

/// Contexts and depths followed at most (a game has a few of each).
const FOLLOWED: usize = 64;

/// What sharing one depth size needs, on the game's device.
struct Made {
    size: (u32, u32),
    format: DXGI_FORMAT,
    copy: ID3D11Texture2D,
    read: ID3D11ShaderResourceView,
    floats: ID3D11Texture2D,
    write: ID3D11UnorderedAccessView,
    shader: ID3D11ComputeShader,
    writer: ImageWriter,
}

fn install<G: Game>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let channel = start.channel()?.clone();
    let (device, context, _) = monaka_channel::d3d::create_device(None).map_err(|e| Lack::Missing(format!("no D3D11 device to find the clear on ({e})")))?;
    let mut deferred = None;
    // SAFETY: a deferred context of our own device, into a local.
    let _ = unsafe { device.CreateDeferredContext(0, Some(&mut deferred)) };
    let slot_of = |context: &ID3D11DeviceContext| monaka_hook::mem::read::<usize>(context.as_raw() as usize).and_then(|table| monaka_hook::mem::read::<usize>(table + CLEAR_DEPTH_SLOT * size_of::<usize>()));
    let refused = |e: monaka_hook::Error| Lack::Fatal(Rejection::unsupported(format!("ClearDepthStencilView: {e}")));
    // SAFETY: our own live contexts, whose slot 53 is ClearDepthStencilView with the detours'
    // type; the code it points at is the one every context of the kind uses.
    unsafe {
        if let monaka_hook::Placement::Vtable(why) = start.hooks.method(&ORIGINAL, "ClearDepthStencilView", context.as_raw(), CLEAR_DEPTH_SLOT, clear_depth as ClearDepthFn).map_err(refused)? {
            log!("debug depth: {why}; hooked in our own context's vtable, which the game's may not share");
        }
        let deferred = deferred.filter(|d| slot_of(d).is_some() && slot_of(d) != slot_of(&context));
        if let Some(deferred) = &deferred
            && let Err(e) = start.hooks.method(&ORIGINAL_DEFERRED, "ClearDepthStencilView (deferred)", deferred.as_raw(), CLEAR_DEPTH_SLOT, clear_depth_deferred as ClearDepthFn)
        {
            log!("debug depth: deferred contexts' clear not hooked ({e})");
        }
        if let Err(e) = start.hooks.method(&BIND_ORIGINAL, "OMSetRenderTargetsAndUnorderedAccessViews", context.as_raw(), BIND_WITH_VIEWS_SLOT, bind_with_views as BindWithViewsFn) {
            log!("debug depth: binds with views not followed ({e})");
        }
        if let Some(deferred) = &deferred
            && let Err(e) = start.hooks.method(&BIND_ORIGINAL_DEFERRED, "OMSetRenderTargetsAndUnorderedAccessViews (deferred)", deferred.as_raw(), BIND_WITH_VIEWS_SLOT, bind_with_views_deferred as BindWithViewsFn)
        {
            log!("debug depth: deferred binds with views not followed ({e})");
        }
    }
    crate::debug_targets::start(channel.clone());
    *STATE.locked() = Some(State { channel, view: (0, 0), scene: None, made: None, generation: 0, kinds: Vec::new(), bound: Vec::new(), drawn: Vec::new(), most_drawn: None, cleared: Vec::new(), frames: 0, tabled: 0 });
    debug::offer(Category::Depth);
    Ok(())
}

/// The stop's first phase (and its end): the game's textures let go, before anything gives back.
fn release() {
    *STATE.locked() = None;
    crate::debug_targets::stop();
}

impl State {
    /// Lets go of the game's textures held, and what was made to share one.
    fn let_go(&mut self) {
        if self.scene.is_none() && self.made.is_none() && self.drawn.is_empty() {
            return;
        }
        self.scene = None;
        self.made = None;
        self.drawn.clear();
        self.bound.clear();
        self.cleared.clear();
        self.most_drawn = None;
    }
}

/// The size of `chain`'s buffers: the game's picture.
pub(crate) fn view_size(chain: &windows::Win32::Graphics::Dxgi::IDXGISwapChain) -> (u32, u32) {
    // SAFETY: a plain query on the game's live swapchain.
    unsafe { chain.GetDesc() }.map_or((0, 0), |d| (d.BufferDesc.Width, d.BufferDesc.Height))
}

fn summary() -> String {
    let depth = format!("debug depth: {} depth clears seen while wanted, {} frames shared, the most-drawn depth changed texture {} times", CLEARS.load(Relaxed), SHARED.load(Relaxed), SWITCHES.load(Relaxed));
    format!("{depth}; {}", crate::debug_targets::summary())
}

unsafe extern "system" fn clear_depth(context: *mut c_void, view: *mut c_void, flags: u32, depth: f32, stencil: u8) {
    let flight = monaka_hook::InFlight::enter();
    if flags & D3D11_CLEAR_DEPTH.0 != 0 && debug::on(Category::Depth) {
        // SAFETY: the game's own live context and view, borrowed for the call.
        unsafe { note(context, view, depth, false) };
    }
    // SAFETY: forwards the game's own call.
    unsafe { ORIGINAL.get(&flight)(context, view, flags, depth, stencil) }
}

unsafe extern "system" fn clear_depth_deferred(context: *mut c_void, view: *mut c_void, flags: u32, depth: f32, stencil: u8) {
    let flight = monaka_hook::InFlight::enter();
    if flags & D3D11_CLEAR_DEPTH.0 != 0 && debug::on(Category::Depth) {
        // SAFETY: the game's own live context and view, borrowed for the call.
        unsafe { note(context, view, depth, true) };
    }
    // SAFETY: forwards the game's own call.
    unsafe { ORIGINAL_DEFERRED.get(&flight)(context, view, flags, depth, stencil) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn bind_with_views(context: *mut c_void, targets: u32, views: *const *mut c_void, depth: *mut c_void, first: u32, count: u32, uavs: *const *mut c_void, counts: *const u32) {
    let flight = monaka_hook::InFlight::enter();
    if targets != KEEP_TARGETS {
        bound(context, targets, views, depth);
    }
    // SAFETY: forwards the game's own call.
    unsafe { BIND_ORIGINAL.get(&flight)(context, targets, views, depth, first, count, uavs, counts) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn bind_with_views_deferred(context: *mut c_void, targets: u32, views: *const *mut c_void, depth: *mut c_void, first: u32, count: u32, uavs: *const *mut c_void, counts: *const u32) {
    let flight = monaka_hook::InFlight::enter();
    if targets != KEEP_TARGETS {
        bound(context, targets, views, depth);
    }
    // SAFETY: forwards the game's own call.
    unsafe { BIND_ORIGINAL_DEFERRED.get(&flight)(context, targets, views, depth, first, count, uavs, counts) }
}

/// A depth clear of `view` to `clear` (0: reversed depth) on `context`: logged if of a new kind,
/// and taken as the scene's depth if it is the largest single-sample one that is not square. A
/// clear of the scene's depth ends a pass: the wanted one is copied first, on this context.
///
/// # Safety
/// `context` and `view` are the game's live context (on the thread recording it) and depth view.
unsafe fn note(context: *mut c_void, view: *mut c_void, clear: f32, deferred: bool) {
    CLEARS.fetch_add(1, Relaxed);
    // SAFETY: the caller's guarantee; borrowed, so nothing is released.
    let Some(view) = (unsafe { ID3D11DepthStencilView::from_raw_borrowed(&view) }) else { return };
    // SAFETY: a plain query on a live object.
    let Ok(texture) = (unsafe { view.GetResource() }).and_then(|r| r.cast::<ID3D11Texture2D>()) else { return };
    let desc = monaka_channel::d3d::texture_desc(&texture);
    let Ok(mut state) = STATE.try_lock() else { return };
    let Some(state) = state.as_mut() else { return };
    let kind = (desc.Width, desc.Height, desc.Format.0, desc.SampleDesc.Count, desc.ArraySize, deferred);
    if !state.kinds.contains(&kind) && state.kinds.len() < LOGGED_KINDS {
        state.kinds.push(kind);
        let context = if deferred { "deferred" } else { "immediate" };
        log!("debug depth: a depth clear on a {context} context: {}x{} {:?}, {} samples, {} layers, to {clear}", desc.Width, desc.Height, desc.Format, desc.SampleDesc.Count, desc.ArraySize);
    }
    if !usable(&desc, state.view) {
        return;
    }
    let texture_key = texture.as_raw() as usize;
    if !state.cleared.iter().any(|(t, _)| *t == texture_key) && state.cleared.len() < FOLLOWED {
        state.cleared.push((texture_key, clear == 0.0));
    }
    let area = u64::from(desc.Width) * u64::from(desc.Height);
    let (larger, scene) = match &state.scene {
        Some((scene, _, _)) if scene == &texture => (false, true),
        // Once the draws say which depth is the scene's, clears no longer choose.
        _ if state.most_drawn.is_some() => (false, false),
        Some((_, largest, _)) => (area > *largest, false),
        None => (true, false),
    };
    if larger {
        log!("debug depth: the scene's depth is taken to be the {}x{} {:?} one", desc.Width, desc.Height, desc.Format);
        state.scene = Some((texture.clone(), area, Some(clear == 0.0)));
        PASSES.store(0, Relaxed);
    }
    if !scene && !larger {
        return;
    }
    // This is clear `number` of the frame: the depth is copied just before it when wanted.
    let number = PASSES.fetch_add(1, Relaxed) + 1;
    let wanted = debug::wanted_pass();
    // SAFETY: the caller's guarantee; borrowed, so nothing is released.
    let Some(context) = (unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }) else { return };
    if wanted != 0 && number == wanted
        && let Some(made) = state.made.as_ref().filter(|m| m.size == (desc.Width, desc.Height) && m.format == desc.Format)
    {
        // SAFETY: our own texture of the depth's shape on its device, copied on the context that
        // is about to clear it, in its order.
        unsafe { context.CopyResource(&made.copy, &texture) };
        COPIED.store(wanted, Relaxed);
    }
}

/// `OMSetRenderTargets` on `context` with `count` render targets (`views`) and depth view `view`
/// (null: none), from the HUD layer's hook: the targets and the depth that context draws with from
/// now.
pub fn bound(context: *mut c_void, count: u32, views: *const *mut c_void, view: *mut c_void) {
    crate::debug_targets::bound(context, count, views);
    if !debug::on(Category::Depth) {
        return;
    }
    // SAFETY: the game's live depth view, borrowed for the call.
    let texture = unsafe { ID3D11DepthStencilView::from_raw_borrowed(&view) }.and_then(|v| unsafe { v.GetResource() }.ok()).and_then(|r| r.cast::<ID3D11Texture2D>().ok());
    let Ok(mut state) = STATE.try_lock() else { return };
    let Some(state) = state.as_mut() else { return };
    let key = texture.as_ref().map_or(0, |t| t.as_raw() as usize);
    match state.bound.iter().position(|(c, _)| *c == context as usize) {
        Some(at) => state.bound[at].1 = key,
        None if state.bound.len() < FOLLOWED => state.bound.push((context as usize, key)),
        None => return,
    }
    if let Some(texture) = texture
        && !state.drawn.iter().any(|d| d.texture.as_raw() as usize == key)
        && state.drawn.len() < FOLLOWED
    {
        let desc = monaka_channel::d3d::texture_desc(&texture);
        state.drawn.push(Drawn { texture, desc, draws: 0, average: 0.0 });
    }
}

/// A draw on `context`, from the HUD layer's hook: one more for the depth and the targets it draws
/// with.
pub fn drew(context: *mut c_void) {
    crate::debug_targets::drew(context);
    if !debug::on(Category::Depth) {
        return;
    }
    let Ok(mut state) = STATE.try_lock() else { return };
    let Some(state) = state.as_mut() else { return };
    let Some(&(_, key)) = state.bound.iter().find(|(c, _)| *c == context as usize) else { return };
    if let Some(drawn) = state.drawn.iter_mut().find(|d| d.texture.as_raw() as usize == key) {
        drawn.draws += 1;
    }
}

/// A depth as listed for the viewer, the picture being `view` in size.
fn candidate(drawn: &Drawn, view: (u32, u32)) -> DepthCandidate {
    let desc = &drawn.desc;
    DepthCandidate {
        width: desc.Width,
        height: desc.Height,
        format: desc.Format.0 as u32,
        draws: drawn.average.round() as u32,
        samples: desc.SampleDesc.Count as u16,
        layers: desc.ArraySize as u16,
        usable: u32::from(usable(desc, view)),
    }
}

/// A depth that can be shared: single-sample, one layer, of a depth format, and not square (shadow
/// maps are) unless it is the size of the picture (`view`; a headset's eyes may be square).
fn usable(desc: &D3D11_TEXTURE2D_DESC, view: (u32, u32)) -> bool {
    desc.SampleDesc.Count == 1 && desc.ArraySize == 1 && (desc.Width != desc.Height || (desc.Width, desc.Height) == view) && formats(desc.Format).is_some()
}

/// The depth taken as the scene's this frame: the viewer's pick, or the one the frame's draws used
/// most, weighed by its size (a half-size buffer of many small draws loses to the scene's). Logged
/// when its shape changes, a game cycling several of one shape counted; the depths drawn with are
/// listed for the viewer now and then, and logged as a table when new ones appear. The frame's
/// counts start over.
fn most_drawn(state: &mut State) {
    state.frames += 1;
    for drawn in &mut state.drawn {
        drawn.average = drawn.average * 0.9 + drawn.draws as f32 * 0.1;
    }
    let shareable: Vec<usize> = (0..state.drawn.len()).filter(|&i| usable(&state.drawn[i].desc, state.view)).take(DEPTH_CANDIDATES).collect();
    // Draws lately (a frame's alone come and go as the game records its work), weighed by size.
    let weight = |d: &Drawn| d.average * d.desc.Width as f32 * d.desc.Height as f32;
    let picked = match debug::wanted_buffer() {
        0 => shareable.iter().copied().filter(|&i| state.drawn[i].average >= 1.0).max_by(|&a, &b| weight(&state.drawn[a]).total_cmp(&weight(&state.drawn[b]))),
        wanted => shareable.get(wanted as usize - 1).copied(),
    };
    if state.frames.is_multiple_of(LIST_EVERY) {
        let listed: Vec<DepthCandidate> = shareable.iter().map(|&i| candidate(&state.drawn[i], state.view)).collect();
        let chosen = picked.and_then(|p| shareable.iter().position(|&i| i == p)).map_or(0, |at| at as u32 + 1);
        debug::share_candidates(&listed, chosen);
    }
    if state.frames >= 30 && state.drawn.len() != state.tabled && TABLES.fetch_add(1, Relaxed) < LOGGED_TABLES {
        state.tabled = state.drawn.len();
        let rows: Vec<String> = state
            .drawn
            .iter()
            .map(|d| {
                let c = candidate(d, state.view);
                let note = if c.usable == 1 { "" } else { ", not shareable" };
                format!("{}x{} {} ({:?}) {} samples {} layers: {:.0} draws a frame{note}", c.width, c.height, c.format_name(), d.desc.Format, c.samples, c.layers, d.average)
            })
            .collect();
        log!("debug depth: the depths drawn with, in the order seen: {}", rows.join("; "));
    }
    let best = picked.map(|i| (state.drawn[i].texture.clone(), state.drawn[i].draws));
    for drawn in &mut state.drawn {
        drawn.draws = 0;
    }
    let Some((texture, draws)) = best else { return };
    let desc = monaka_channel::d3d::texture_desc(&texture);
    let shape = (desc.Width, desc.Height, desc.Format.0);
    if state.most_drawn != Some(shape) {
        log!("debug depth: the scene's depth is taken to be a {}x{} {:?} one, drawn with most ({draws} draws this frame)", desc.Width, desc.Height, desc.Format);
        state.most_drawn = Some(shape);
    }
    if state.scene.as_ref().is_some_and(|(scene, _, _)| scene != &texture) {
        SWITCHES.fetch_add(1, Relaxed);
    }
    let key = texture.as_raw() as usize;
    let reversed = state.cleared.iter().find(|(t, _)| *t == key).map(|(_, reversed)| *reversed);
    state.scene = Some((texture, u64::from(desc.Width) * u64::from(desc.Height), reversed));
}

/// At each present of `chain` (the game's swapchain), before it: the scene's depth, whole for the
/// frame about to show, shared, while the viewer wants it. Not wanted, or the picture resized (its
/// buffers made anew), the textures held are let go.
pub fn at_present(chain: &windows::Win32::Graphics::Dxgi::IDXGISwapChain) {
    // The frame this present shows: drawn from the camera chosen `latency` presents before, the
    // key its eye is published under (`Schedule::pair_keys`), and that eye. A loop without the
    // driver says no frames: the frame being drawn stands in, its eye not known.
    let (key, eye) = match (crate::feature::DRIVER.drawing_present(), crate::feature::DRIVER.schedule()) {
        (Some(n), Some(schedule)) => (schedule.chosen_at(n).wrapping_add(1), schedule.shown_eye(n).map_or(2, |eye| eye as u32)),
        _ => (debug::drawing(), 2),
    };
    crate::debug_targets::at_present(chain, key, eye);
    let Ok(mut state) = STATE.try_lock() else { return };
    let Some(state) = state.as_mut() else { return };
    if !debug::on(Category::Depth) {
        state.let_go();
        return;
    }
    let view = view_size(chain);
    if state.view != view {
        state.let_go();
        state.view = view;
    }
    most_drawn(state);
    let Some((texture, _, reversed)) = state.scene.clone() else { return };
    // SAFETY: plain queries on the game's live swapchain.
    let Ok(device) = (unsafe { chain.GetDevice::<ID3D11Device>() }) else { return };
    if monaka_channel::d3d::texture_device(&texture).ok().as_ref() != Some(&device) {
        return;
    }
    // SAFETY: as above.
    let Ok(context) = (unsafe { device.GetImmediateContext() }) else { return };
    let context = &context;
    let desc = monaka_channel::d3d::texture_desc(&texture);
    if state.made.as_ref().is_none_or(|m| m.size != (desc.Width, desc.Height) || m.format != desc.Format) {
        state.made = None;
        match make(&device, &state.channel, &desc) {
            Ok(made) => {
                state.generation += 1;
                log!("debug depth: sharing the scene's depth, {}x{} {:?}", desc.Width, desc.Height, desc.Format);
                state.made = Some(made);
            }
            Err(e) => {
                log!("debug depth: cannot share {}x{} {:?}: {e}", desc.Width, desc.Height, desc.Format);
                state.scene = None;
                return;
            }
        }
    }
    let Some(made) = state.made.as_mut() else { return };
    let passes = PASSES.swap(0, Relaxed);
    let copied = COPIED.swap(0, Relaxed);
    // Copied before the wanted clear; else what the depth holds now.
    let pass = if copied != 0 && copied == debug::wanted_pass() { copied } else { 0 };
    // SAFETY: our own resources on the game's device, used on its immediate context; the compute
    // bindings changed are put back.
    unsafe {
        if pass == 0 {
            context.CopyResource(&made.copy, &texture);
        }
        dispatch(context, &made.shader, &made.read, &made.write, made.size);
    }
    if made.writer.write(context, &made.floats) {
        debug::share_depth(DebugDepth { generation: state.generation, reversed: reversed.map_or(2, u32::from), width: desc.Width, height: desc.Height, key, tick: monaka_channel::tick(), passes, pass, eye, _pad: 0 });
        SHARED.fetch_add(1, Relaxed);
    }
}

/// The typeless format a depth texture is copied as, and the format its depth is read as.
fn formats(format: DXGI_FORMAT) -> Option<(DXGI_FORMAT, DXGI_FORMAT)> {
    match format {
        DXGI_FORMAT_D24_UNORM_S8_UINT | DXGI_FORMAT_R24G8_TYPELESS => Some((DXGI_FORMAT_R24G8_TYPELESS, DXGI_FORMAT_R24_UNORM_X8_TYPELESS)),
        DXGI_FORMAT_D32_FLOAT | DXGI_FORMAT_R32_TYPELESS | DXGI_FORMAT_R32_FLOAT => Some((DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_R32_FLOAT)),
        DXGI_FORMAT_D16_UNORM | DXGI_FORMAT_R16_TYPELESS => Some((DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_R16_UNORM)),
        DXGI_FORMAT_D32_FLOAT_S8X24_UINT | DXGI_FORMAT_R32G8X24_TYPELESS => Some((DXGI_FORMAT_R32G8X24_TYPELESS, DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS)),
        _ => None,
    }
}

fn make(device: &ID3D11Device, channel: &ChannelName, game: &D3D11_TEXTURE2D_DESC) -> Result<Made, String> {
    let (copy_format, read_format) = formats(game.Format).ok_or("not a depth format")?;
    let failed = |what: &'static str| move |e: windows::core::Error| format!("{what}: {e}");
    let mut copy_desc = monaka_channel::d3d::simple_texture_desc(game.Width, game.Height, copy_format);
    copy_desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    let copy = monaka_channel::d3d::create_texture(device, &copy_desc).map_err(|e| format!("copy: {e}"))?;
    let read_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: read_format,
        ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
    };
    let mut read = None;
    // SAFETY: a view of our own texture in a format of its family.
    unsafe { device.CreateShaderResourceView(&copy, Some(&read_desc), Some(&mut read)) }.map_err(failed("depth view"))?;
    let mut floats_desc = monaka_channel::d3d::simple_texture_desc(game.Width, game.Height, DXGI_FORMAT_R32_FLOAT);
    floats_desc.BindFlags = (D3D11_BIND_UNORDERED_ACCESS.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let floats = monaka_channel::d3d::create_texture(device, &floats_desc).map_err(|e| format!("floats: {e}"))?;
    let mut write = None;
    // SAFETY: a view of our own texture in its own format.
    unsafe { device.CreateUnorderedAccessView(&floats, None, Some(&mut write)) }.map_err(failed("floats view"))?;
    let code = compile(SOURCE, "debug depth", s!("main"))?;
    let mut shader = None;
    // SAFETY: bytecode the compiler just made, and a local out-pointer.
    unsafe { device.CreateComputeShader(blob_bytes(&code), None, Some(&mut shader)) }.map_err(failed("compute shader"))?;
    let writer = ImageWriter::create(&channel.debug_depth(), device, game.Width, game.Height, DXGI_FORMAT_R32_FLOAT).map_err(|e| format!("shared texture: {e}"))?;
    Ok(Made {
        size: (game.Width, game.Height),
        format: game.Format,
        copy,
        read: read.ok_or("no depth view")?,
        floats,
        write: write.ok_or("no floats view")?,
        shader: shader.ok_or("no compute shader")?,
        writer,
    })
}

/// Runs `shader` once per texel of `size` (8x8 groups), reading `read` and writing `write`: a copy
/// turned into floats. The game's compute bindings it changes are put back.
///
/// # Safety
/// `context` is the immediate context of the device the views and the shader are on.
pub(crate) unsafe fn dispatch(context: &ID3D11DeviceContext, compute: &ID3D11ComputeShader, read: &ID3D11ShaderResourceView, write: &ID3D11UnorderedAccessView, size: (u32, u32)) {
    let mut shader = None;
    let mut reads = [None];
    let mut writes = [None];
    // SAFETY: the caller's guarantee; reads the bindings into locals, sets ours, dispatches, and
    // sets the game's back.
    unsafe {
        context.CSGetShader(&mut shader, None, None);
        context.CSGetShaderResources(0, Some(&mut reads));
        context.CSGetUnorderedAccessViews(0, Some(&mut writes));
        context.CSSetShader(compute, None);
        context.CSSetShaderResources(0, Some(&[Some(read.clone())]));
        context.CSSetUnorderedAccessViews(0, 1, Some(&Some(write.clone())), None);
        let (width, height) = size;
        context.Dispatch(width.div_ceil(8), height.div_ceil(8), 1);
        context.CSSetShader(shader.as_ref(), None);
        context.CSSetShaderResources(0, Some(&reads));
        context.CSSetUnorderedAccessViews(0, 1, Some(&writes[0]), None);
    }
}
