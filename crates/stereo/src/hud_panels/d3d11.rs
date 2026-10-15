//! The dynamic HUD's panels drawn with D3D11, on the game's own device and immediate context: a
//! piece's draw runs into its panel's two targets (black and white) bound past the game's hooked
//! `OMSetRenderTargets` ([`crate::hud_layer::d3d11::set_targets_unhooked`]), with the viewport scaled and moved
//! so the piece's rectangle fills the panel and the game's own scissor carried over (with the
//! panel's own depth-stencil for a masked HUD); then the game's targets, viewport and scissor are
//! given back. The panels are made on the game's device in the format of its HUD target, so their
//! texels are what the eye image's would be.

use super::Piece;
use crate::hud_layer::d3d11::set_targets_unhooked;
use crate::hud_panels::panel::Panel;
use monaka_channel::ChannelName;
use monaka_core::sync::Locked;
use monaka_producer::log;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM};
use windows::core::Interface;

/// What the panels are drawn with, made at the first piece draw.
struct Gpu {
    device: ID3D11Device,
    format: DXGI_FORMAT,
    /// The panels have depth-stencils (`HudPieces::MASKED`).
    masked: bool,
    /// By piece, made when it first shows.
    panels: Vec<Option<Panel>>,
}

struct Backend {
    gpu: Option<Gpu>,
    /// The panels could not be made: the pieces stay in the HUD.
    broken: bool,
    /// Which panels are drawn into this frame, and went out shown.
    states: super::PanelStates,
}

static BACKEND: Mutex<Option<Backend>> = Mutex::new(None);
static DRAWS: AtomicU64 = AtomicU64::new(0);

pub(super) fn release() {
    BACKEND.locked().take();
}

impl Gpu {
    fn new(context: &ID3D11DeviceContext, pieces: usize, masked: bool) -> Result<Self, String> {
        // SAFETY: COM calls on the game's live context.
        let device = unsafe { context.GetDevice() }.map_err(|e| e.to_string())?;
        let mut bound = [None];
        // SAFETY: reads the bound render target into a local.
        unsafe { context.OMGetRenderTargets(Some(&mut bound), None) };
        let format = bound[0]
            .as_ref()
            .map(|view| {
                let mut desc = D3D11_RENDER_TARGET_VIEW_DESC::default();
                // SAFETY: reads the view's description into a local.
                unsafe { view.GetDesc(&mut desc) };
                desc.Format
            })
            .unwrap_or(DXGI_FORMAT_R8G8B8A8_UNORM);
        Ok(Self { device, format, masked, panels: (0..pieces).map(|_| None).collect() })
    }

    /// Piece `index`'s panel, made when first asked for; none when it cannot be made (logged).
    fn panel(&mut self, channel: &ChannelName, pieces: &[Piece], index: usize) -> Option<&Panel> {
        let piece = pieces.get(index)?;
        let slot = self.panels.get_mut(index)?;
        if slot.is_none() {
            match Panel::new(&self.device, channel, piece.name, piece.size, self.format, self.masked) {
                Ok(panel) => {
                    log!("HUD panel {}: {}x{} (format {}, the game's HUD target's)", piece.name, piece.size.0, piece.size.1, self.format.0);
                    *slot = Some(panel);
                }
                Err(e) => {
                    log!("HUD panel {} unavailable: {e}", piece.name);
                    return None;
                }
            }
        }
        slot.as_ref()
    }
}

/// A HUD draw of piece `piece` (`draw`, run into whatever is bound) on the game's immediate
/// context `context`, in its hooked draw, the HUD laid out over the viewport `layout`: run into
/// the piece's panel and true. False, nothing run, when the piece does not show (or its panel
/// cannot be made): the game draws it its own way.
pub fn draw(context: *mut c_void, piece: usize, layout: D3D11_VIEWPORT, draw: &dyn Fn()) -> bool {
    if !super::active() {
        return false;
    }
    // SAFETY: the game passes its live immediate context.
    let Some(ctx) = (unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }) else { return false };
    let Some(frame) = super::frame() else { return false };
    let Some(rect) = frame.rects.get(piece).copied().flatten() else { return false };
    let mut guard = BACKEND.locked();
    let backend = guard.get_or_insert_with(|| Backend { gpu: None, broken: false, states: super::PanelStates::default() });
    if backend.broken {
        return false;
    }
    if backend.gpu.is_none() {
        match Gpu::new(ctx, frame.pieces.len(), frame.masked) {
            Ok(gpu) => backend.gpu = Some(gpu),
            Err(e) => {
                log!("HUD panels unavailable: {e}");
                backend.broken = true;
                return false;
            }
        }
    }
    let Backend { gpu, states, .. } = backend;
    let gpu = gpu.as_mut().expect("made above");
    // The frame's first piece draw: every shown piece's panel starts empty, and goes out this
    // frame drawn or not.
    if states.start(frame.pieces.len()) {
        for (index, shown) in frame.rects.iter().enumerate() {
            if shown.is_some()
                && let Some(panel) = gpu.panel(&frame.channel, frame.pieces, index)
            {
                panel.clear(ctx);
                states.ready(index);
            }
        }
    }
    let Some(panel) = gpu.panels.get(piece).and_then(Option::as_ref).filter(|_| states.is_ready(piece)) else { return false };
    draw_into(ctx, context, panel, frame.pieces[piece].size, layout, rect, draw);
    DRAWS.fetch_add(1, Relaxed);
    true
}

/// Once a frame, at the present, on the game's immediate context: every shown panel resolved and
/// published; one no longer shown published cleared, once.
pub fn frame_end(context: &ID3D11DeviceContext) {
    let mut guard = BACKEND.locked();
    let Some(backend) = guard.as_mut() else { return };
    let outs = backend.states.end();
    let Some(gpu) = backend.gpu.as_mut() else { return };
    for (index, out) in outs.into_iter().enumerate() {
        let (Some(out), Some(Some(panel))) = (out, gpu.panels.get_mut(index)) else { continue };
        if panel.publish(context, out) && backend.states.went_out(index, out) {
            super::note_change(panel.piece(), out);
        }
    }
}

/// Each piece's panel made so far, for a probe (its name and panel).
pub fn each_panel(f: impl FnMut(&Panel)) {
    if let Some(gpu) = BACKEND.locked().as_ref().and_then(|b| b.gpu.as_ref()) {
        gpu.panels.iter().flatten().for_each(f);
    }
}

/// Piece draws run into the panels so far.
pub fn draws() -> u64 {
    DRAWS.load(Relaxed)
}

/// Runs a HUD draw into a piece's panel, onto black and onto white, with the layout `base` scaled
/// and moved so `rect` (fractions of it) fills the panel of `size`; the game's bindings are given
/// back.
fn draw_into(ctx: &ID3D11DeviceContext, context: *mut c_void, panel: &Panel, size: (u32, u32), base: D3D11_VIEWPORT, rect: [f32; 4], draw: &dyn Fn()) {
    let mut bound = [None];
    let mut depth = None;
    let (mut viewport_count, mut scissor_count) = (1u32, 1u32);
    let mut viewport = [D3D11_VIEWPORT::default()];
    let mut scissors = [RECT::default()];
    // SAFETY: reads the game's bound state into locals on its own context and thread.
    let raster: Option<ID3D11RasterizerState> = unsafe { ctx.RSGetState() }.ok();
    // SAFETY: as above.
    unsafe {
        ctx.OMGetRenderTargets(Some(&mut bound), Some(&mut depth));
        ctx.RSGetViewports(&mut viewport_count, Some(viewport.as_mut_ptr()));
        ctx.RSGetScissorRects(&mut scissor_count, Some(scissors.as_mut_ptr()));
    }
    let game_scissors = scissor_count > 0
        && raster.as_ref().is_some_and(|r| {
            let mut desc = D3D11_RASTERIZER_DESC::default();
            // SAFETY: reads the state's description into a local.
            unsafe { r.GetDesc(&mut desc) };
            desc.ScissorEnable.as_bool()
        });
    let panel_view = panel_viewport(base, rect, size);
    let (sx, sy) = (panel_view.Width / base.Width, panel_view.Height / base.Height);
    // The game's own scissor (text boxes are clipped by one), set in its layout's pixels, carried
    // into the panel.
    let full = RECT { left: 0, top: 0, right: size.0 as i32, bottom: size.1 as i32 };
    let clip = if game_scissors {
        let x = |v: i32| ((v as f32 - base.TopLeftX) * sx + panel_view.TopLeftX).floor() as i32;
        let y = |v: i32| ((v as f32 - base.TopLeftY) * sy + panel_view.TopLeftY).floor() as i32;
        let s = scissors[0];
        intersect(&full, &RECT { left: x(s.left), top: y(s.top), right: x(s.right), bottom: y(s.bottom) })
    } else {
        full
    };
    // SAFETY: binds our targets through the original OMSetRenderTargets (the hook stays out of
    // it), draws, and gives back the game's targets, viewport and scissor.
    unsafe {
        if clip.right > clip.left && clip.bottom > clip.top {
            ctx.RSSetViewports(Some(&[panel_view]));
            ctx.RSSetScissorRects(Some(&[clip]));
            for (target, depth) in panel.target_views().into_iter().zip(panel.depth_views()) {
                let view = [target.as_raw()];
                set_targets_unhooked(context, 1, view.as_ptr(), depth.map_or(std::ptr::null_mut(), |d| d.as_raw()));
                draw();
            }
        }
        let game = [bound[0].as_ref().map_or(std::ptr::null_mut(), |v| v.as_raw())];
        set_targets_unhooked(context, 1, game.as_ptr(), depth.as_ref().map_or(std::ptr::null_mut(), |d| d.as_raw()));
        ctx.RSSetViewports(Some(&viewport[..viewport_count.min(1) as usize]));
        ctx.RSSetScissorRects(Some(&scissors[..scissor_count.min(1) as usize]));
    }
}

/// The viewport that maps the HUD laid out over `base` so its rectangle `rect` (x, y, width,
/// height as fractions of it) fills a panel of `size`.
fn panel_viewport(base: D3D11_VIEWPORT, rect: [f32; 4], size: (u32, u32)) -> D3D11_VIEWPORT {
    let (sx, sy) = (size.0 as f32 / (rect[2] * base.Width), size.1 as f32 / (rect[3] * base.Height));
    D3D11_VIEWPORT {
        TopLeftX: -rect[0] * base.Width * sx,
        TopLeftY: -rect[1] * base.Height * sy,
        Width: base.Width * sx,
        Height: base.Height * sy,
        MinDepth: base.MinDepth,
        MaxDepth: base.MaxDepth,
    }
}

fn intersect(a: &RECT, b: &RECT) -> RECT {
    RECT { left: a.left.max(b.left), top: a.top.max(b.top), right: a.right.min(b.right), bottom: a.bottom.min(b.bottom) }
}

#[cfg(test)]
mod tests;
