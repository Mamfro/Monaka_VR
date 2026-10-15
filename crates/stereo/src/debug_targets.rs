//! One of the game's render targets for the debug views (its normals, its albedo, whatever it
//! draws into besides the picture), for the viewer's "Game buffer" view. The targets each context
//! draws into are followed with the depth ([`crate::debug_depth`]: the same bind and draw hooks),
//! their draws counted; the viewer picks one from the list ([`debug::share_targets`]), or takes
//! the most drawn (weighed by size). At each present it is copied, turned into 16-bit floats as
//! stored (an sRGB target read as plain values) and shared with its eye
//! ([`monaka_channel::debug_image`]), while the viewer wants it ([`Category::Targets`]).
//!
//! A swapchain's buffers are never followed (a resize fails while anything holds one), and the
//! targets are let go when no view wants them, the picture resizes, or the run stops.

use monaka_channel::ChannelName;
use monaka_channel::d3d12::compute::{blob_bytes, compile};
use monaka_channel::debug_image::ImageWriter;
use monaka_contract::debug_draw::{Category, DEPTH_CANDIDATES, DebugDepth, DepthCandidate};
use monaka_core::sync::Locked;
use monaka_producer::{debug, log};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use windows::Win32::Graphics::Direct3D::D3D_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{DXGI_USAGE_BACK_BUFFER, IDXGIResource, IDXGISwapChain};
use windows::core::{Interface, s};

#[cfg(test)]
mod tests;

const SOURCE: &str = include_str!("debug_targets.hlsl");
/// Render targets a bind may set at once.
const SLOTS: usize = 8;
/// Contexts and targets followed at most.
const FOLLOWED: usize = 128;
/// The list goes to the viewer every this many presents; tables logged at most.
const LIST_EVERY: u32 = 10;
const LOGGED_TABLES: u32 = 6;

static STATE: Mutex<Option<State>> = Mutex::new(None);
static SHARED: AtomicU64 = AtomicU64::new(0);
static TABLES: AtomicU32 = AtomicU32::new(0);

struct State {
    channel: ChannelName,
    /// The size of the game's picture, as of the last present.
    view: (u32, u32),
    /// The targets each context draws into now (context, up to [`SLOTS`] textures).
    bound: Vec<(usize, [usize; SLOTS])>,
    /// The targets drawn into, in the order first seen.
    drawn: Vec<Drawn>,
    made: Option<Made>,
    generation: u32,
    frames: u32,
    tabled: usize,
}

/// A render target drawn into: its texture, its shape, its draws this frame and lately.
struct Drawn {
    texture: ID3D11Texture2D,
    desc: D3D11_TEXTURE2D_DESC,
    draws: u64,
    average: f32,
}

/// What sharing one target needs, on the game's device.
struct Made {
    shape: (u32, u32, DXGI_FORMAT),
    copy: ID3D11Texture2D,
    read: ID3D11ShaderResourceView,
    floats: ID3D11Texture2D,
    write: ID3D11UnorderedAccessView,
    shader: ID3D11ComputeShader,
    writer: ImageWriter,
}

/// Starts following the targets, for the run on `channel` (the debug depth's install).
pub(crate) fn start(channel: ChannelName) {
    *STATE.locked() = Some(State { channel, view: (0, 0), bound: Vec::new(), drawn: Vec::new(), made: None, generation: 0, frames: 0, tabled: 0 });
    debug::offer(Category::Targets);
}

pub(crate) fn stop() {
    *STATE.locked() = None;
}

impl State {
    /// Lets go of the game's targets held, and what was made to share one.
    fn let_go(&mut self) {
        if self.made.is_none() && self.drawn.is_empty() {
            return;
        }
        self.made = None;
        self.drawn.clear();
        self.bound.clear();
    }
}

/// Whether `texture` is a swapchain's buffer.
fn back_buffer(texture: &ID3D11Texture2D) -> bool {
    // SAFETY: a plain query on a live texture.
    texture.cast::<IDXGIResource>().ok().and_then(|r| unsafe { r.GetUsage() }.ok()).is_some_and(|usage| usage.0 & DXGI_USAGE_BACK_BUFFER.0 != 0)
}

pub(crate) fn summary() -> String {
    format!("debug targets: {} frames shared", SHARED.load(Relaxed))
}

/// A bind of `count` render targets (`views`) on `context`: what it draws into from now.
pub(crate) fn bound(context: *mut c_void, count: u32, views: *const *mut c_void) {
    if !debug::on(Category::Targets) {
        return;
    }
    let mut textures = [0usize; SLOTS];
    let mut found: Vec<ID3D11Texture2D> = Vec::new();
    if !views.is_null() {
        for (slot, texture) in textures.iter_mut().enumerate().take((count as usize).min(SLOTS)) {
            // SAFETY: the game passes an array of `count` views (each may be null).
            let view = unsafe { *views.add(slot) };
            // SAFETY: the game's live render target view, borrowed for the call.
            let resource = unsafe { ID3D11RenderTargetView::from_raw_borrowed(&view) }.and_then(|v| unsafe { v.GetResource() }.ok()).and_then(|r| r.cast::<ID3D11Texture2D>().ok());
            if let Some(resource) = resource {
                *texture = resource.as_raw() as usize;
                found.push(resource);
            }
        }
    }
    let Ok(mut state) = STATE.try_lock() else { return };
    let Some(state) = state.as_mut() else { return };
    match state.bound.iter().position(|(c, _)| *c == context as usize) {
        Some(at) => state.bound[at].1 = textures,
        None if state.bound.len() < FOLLOWED => state.bound.push((context as usize, textures)),
        None => return,
    }
    for texture in found {
        if state.drawn.len() < FOLLOWED && !state.drawn.iter().any(|d| d.texture == texture) && !back_buffer(&texture) {
            let desc = monaka_channel::d3d::texture_desc(&texture);
            state.drawn.push(Drawn { texture, desc, draws: 0, average: 0.0 });
        }
    }
}

/// A draw on `context`: one more for each target it draws into.
pub(crate) fn drew(context: *mut c_void) {
    if !debug::on(Category::Targets) {
        return;
    }
    let Ok(mut state) = STATE.try_lock() else { return };
    let Some(state) = state.as_mut() else { return };
    let Some(&(_, textures)) = state.bound.iter().find(|(c, _)| *c == context as usize) else { return };
    for texture in textures.into_iter().filter(|t| *t != 0) {
        if let Some(drawn) = state.drawn.iter_mut().find(|d| d.texture.as_raw() as usize == texture) {
            drawn.draws += 1;
        }
    }
}

/// A target as listed for the viewer.
fn candidate(drawn: &Drawn) -> DepthCandidate {
    let desc = &drawn.desc;
    DepthCandidate {
        width: desc.Width,
        height: desc.Height,
        format: desc.Format.0 as u32,
        draws: drawn.average.round() as u32,
        samples: desc.SampleDesc.Count as u16,
        layers: desc.ArraySize as u16,
        usable: u32::from(shareable(desc)),
    }
}

/// A target that can be shared: single-sample, one layer, of a format read as plain values.
fn shareable(desc: &D3D11_TEXTURE2D_DESC) -> bool {
    desc.SampleDesc.Count == 1 && desc.ArraySize == 1 && formats(desc.Format).is_some()
}

/// The typeless format a target is copied as, and the format it is read as (its stored values: an
/// sRGB target as plain ones).
pub(crate) fn formats(format: DXGI_FORMAT) -> Option<(DXGI_FORMAT, DXGI_FORMAT)> {
    Some(match format {
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => (DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM),
        DXGI_FORMAT_R8G8B8A8_SNORM => (DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_SNORM),
        DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => (DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM),
        DXGI_FORMAT_B8G8R8X8_TYPELESS | DXGI_FORMAT_B8G8R8X8_UNORM | DXGI_FORMAT_B8G8R8X8_UNORM_SRGB => (DXGI_FORMAT_B8G8R8X8_TYPELESS, DXGI_FORMAT_B8G8R8X8_UNORM),
        DXGI_FORMAT_R10G10B10A2_TYPELESS | DXGI_FORMAT_R10G10B10A2_UNORM => (DXGI_FORMAT_R10G10B10A2_TYPELESS, DXGI_FORMAT_R10G10B10A2_UNORM),
        DXGI_FORMAT_R11G11B10_FLOAT => (DXGI_FORMAT_R11G11B10_FLOAT, DXGI_FORMAT_R11G11B10_FLOAT),
        DXGI_FORMAT_R16G16B16A16_TYPELESS | DXGI_FORMAT_R16G16B16A16_FLOAT => (DXGI_FORMAT_R16G16B16A16_TYPELESS, DXGI_FORMAT_R16G16B16A16_FLOAT),
        DXGI_FORMAT_R16G16B16A16_UNORM => (DXGI_FORMAT_R16G16B16A16_TYPELESS, DXGI_FORMAT_R16G16B16A16_UNORM),
        DXGI_FORMAT_R16G16B16A16_SNORM => (DXGI_FORMAT_R16G16B16A16_TYPELESS, DXGI_FORMAT_R16G16B16A16_SNORM),
        DXGI_FORMAT_R16G16_TYPELESS | DXGI_FORMAT_R16G16_FLOAT => (DXGI_FORMAT_R16G16_TYPELESS, DXGI_FORMAT_R16G16_FLOAT),
        DXGI_FORMAT_R16G16_UNORM => (DXGI_FORMAT_R16G16_TYPELESS, DXGI_FORMAT_R16G16_UNORM),
        DXGI_FORMAT_R16G16_SNORM => (DXGI_FORMAT_R16G16_TYPELESS, DXGI_FORMAT_R16G16_SNORM),
        DXGI_FORMAT_R32G32B32A32_TYPELESS | DXGI_FORMAT_R32G32B32A32_FLOAT => (DXGI_FORMAT_R32G32B32A32_TYPELESS, DXGI_FORMAT_R32G32B32A32_FLOAT),
        DXGI_FORMAT_R32G32_TYPELESS | DXGI_FORMAT_R32G32_FLOAT => (DXGI_FORMAT_R32G32_TYPELESS, DXGI_FORMAT_R32G32_FLOAT),
        DXGI_FORMAT_R32_TYPELESS | DXGI_FORMAT_R32_FLOAT => (DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_R32_FLOAT),
        DXGI_FORMAT_R16_TYPELESS | DXGI_FORMAT_R16_FLOAT => (DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_R16_FLOAT),
        DXGI_FORMAT_R16_UNORM => (DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_R16_UNORM),
        DXGI_FORMAT_R8G8_TYPELESS | DXGI_FORMAT_R8G8_UNORM => (DXGI_FORMAT_R8G8_TYPELESS, DXGI_FORMAT_R8G8_UNORM),
        DXGI_FORMAT_R8G8_SNORM => (DXGI_FORMAT_R8G8_TYPELESS, DXGI_FORMAT_R8G8_SNORM),
        DXGI_FORMAT_R8_TYPELESS | DXGI_FORMAT_R8_UNORM => (DXGI_FORMAT_R8_TYPELESS, DXGI_FORMAT_R8_UNORM),
        _ => return None,
    })
}

/// At each present of `chain`, before it: the counts of the frame folded in, the list sent now and
/// then, and the wanted target shared with the eye `eye` (2: not known) of the frame shown, `key`.
/// Not wanted, or the picture resized (its targets made anew), the targets held are let go.
pub(crate) fn at_present(chain: &IDXGISwapChain, key: u64, eye: u32) {
    let Ok(mut state) = STATE.try_lock() else { return };
    let Some(state) = state.as_mut() else { return };
    if !debug::on(Category::Targets) {
        state.let_go();
        return;
    }
    let view = crate::debug_depth::view_size(chain);
    if state.view != view {
        state.let_go();
        state.view = view;
    }
    state.frames += 1;
    for drawn in &mut state.drawn {
        drawn.average = drawn.average * 0.9 + drawn.draws as f32 * 0.1;
        drawn.draws = 0;
    }
    // The targets listed: shareable, drawn into lately; the first few seen.
    let listed: Vec<usize> = (0..state.drawn.len())
        .filter(|&i| {
            let d = &state.drawn[i];
            shareable(&d.desc) && d.average >= 0.5
        })
        .take(DEPTH_CANDIDATES)
        .collect();
    let weight = |d: &Drawn| d.average * d.desc.Width as f32 * d.desc.Height as f32;
    let picked = match debug::wanted_target() {
        0 => listed.iter().copied().max_by(|&a, &b| weight(&state.drawn[a]).total_cmp(&weight(&state.drawn[b]))),
        wanted => listed.get(wanted as usize - 1).copied(),
    };
    if state.frames.is_multiple_of(LIST_EVERY) {
        let candidates: Vec<DepthCandidate> = listed.iter().map(|&i| candidate(&state.drawn[i])).collect();
        let chosen = picked.and_then(|p| listed.iter().position(|&i| i == p)).map_or(0, |at| at as u32 + 1);
        debug::share_targets(&candidates, chosen);
    }
    if state.frames >= 30 && state.drawn.len() != state.tabled && TABLES.fetch_add(1, Relaxed) < LOGGED_TABLES {
        state.tabled = state.drawn.len();
        let rows: Vec<String> = state
            .drawn
            .iter()
            .map(|d| {
                let c = candidate(d);
                let note = if c.usable == 1 { "" } else { ", not shareable" };
                format!("{}x{} {} ({:?}) {} samples {} layers: {:.0} draws a frame{note}", c.width, c.height, c.colour_format_name(), d.desc.Format, c.samples, c.layers, d.average)
            })
            .collect();
        log!("debug targets: the render targets drawn into, in the order seen: {}", rows.join("; "));
    }
    let Some(picked) = picked else { return };
    let (texture, desc) = (state.drawn[picked].texture.clone(), state.drawn[picked].desc);
    // SAFETY: as above.
    let Ok(device) = (unsafe { chain.GetDevice::<ID3D11Device>() }) else { return };
    if monaka_channel::d3d::texture_device(&texture).ok().as_ref() != Some(&device) {
        return;
    }
    // SAFETY: as above.
    let Ok(context) = (unsafe { device.GetImmediateContext() }) else { return };
    let shape = (desc.Width, desc.Height, desc.Format);
    if state.made.as_ref().is_none_or(|m| m.shape != shape) {
        state.made = None;
        match make(&device, &state.channel, &desc) {
            Ok(made) => {
                state.generation += 1;
                log!("debug targets: sharing a {}x{} {:?} render target", desc.Width, desc.Height, desc.Format);
                state.made = Some(made);
            }
            Err(e) => {
                log!("debug targets: cannot share a {}x{} {:?} render target: {e}", desc.Width, desc.Height, desc.Format);
                return;
            }
        }
    }
    let Some(made) = state.made.as_mut() else { return };
    // SAFETY: our own resources on the game's device, used on its immediate context; the compute
    // bindings changed are put back.
    unsafe {
        context.CopyResource(&made.copy, &texture);
        crate::debug_depth::dispatch(&context, &made.shader, &made.read, &made.write, (desc.Width, desc.Height));
    }
    if made.writer.write(&context, &made.floats) {
        debug::share_target(DebugDepth { generation: state.generation, reversed: 0, width: desc.Width, height: desc.Height, key, tick: monaka_channel::tick(), passes: 0, pass: 0, eye, _pad: 0 });
        SHARED.fetch_add(1, Relaxed);
    }
}

fn make(device: &ID3D11Device, channel: &ChannelName, game: &D3D11_TEXTURE2D_DESC) -> Result<Made, String> {
    let (copy_format, read_format) = formats(game.Format).ok_or("not a format read as plain values")?;
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
    unsafe { device.CreateShaderResourceView(&copy, Some(&read_desc), Some(&mut read)) }.map_err(failed("target view"))?;
    let mut floats_desc = monaka_channel::d3d::simple_texture_desc(game.Width, game.Height, DXGI_FORMAT_R16G16B16A16_FLOAT);
    floats_desc.BindFlags = (D3D11_BIND_UNORDERED_ACCESS.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let floats = monaka_channel::d3d::create_texture(device, &floats_desc).map_err(|e| format!("floats: {e}"))?;
    let mut write = None;
    // SAFETY: a view of our own texture in its own format.
    unsafe { device.CreateUnorderedAccessView(&floats, None, Some(&mut write)) }.map_err(failed("floats view"))?;
    let code = compile(SOURCE, "debug targets", s!("main"))?;
    let mut shader = None;
    // SAFETY: bytecode the compiler just made, and a local out-pointer.
    unsafe { device.CreateComputeShader(blob_bytes(&code), None, Some(&mut shader)) }.map_err(failed("compute shader"))?;
    let writer = ImageWriter::create(&channel.debug_target(), device, game.Width, game.Height, DXGI_FORMAT_R16G16B16A16_FLOAT).map_err(|e| format!("shared texture: {e}"))?;
    Ok(Made {
        shape: (game.Width, game.Height, game.Format),
        copy,
        read: read.ok_or("no target view")?,
        floats,
        write: write.ok_or("no floats view")?,
        shader: shader.ok_or("no compute shader")?,
        writer,
    })
}
