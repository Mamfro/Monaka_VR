//! The dynamic HUD's panels drawn with OpenGL, for a game that draws its 2D layer once per eye in
//! one frame. On the game's render thread, its GL context current: in the left eye's layer a
//! piece's draw runs into two canvases of our own (`monaka_gl::Canvas`), onto black and onto white,
//! with the viewport scaled and moved so the piece's rectangle fills them; in the other eye's
//! layer it runs nowhere. After the left eye's layer each panel is captured into D3D11
//! (`monaka_gl::Bridge`, on a device of our own) and published.

use super::{PanelOut, Piece};
use crate::hud_panels::panel::Panel;
use monaka_channel::{ChannelName, d3d};
use monaka_contract::protocol::LEFT_HAND;
use monaka_core::sync::Locked;
use monaka_gl::{Bridge, Canvas};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};
use windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

/// One piece: the canvases it is drawn into (onto black, onto white) and its panel.
struct GlPiece {
    canvases: [Canvas; 2],
    panel: Panel,
}

struct Gpu {
    bridge: Bridge,
    context: ID3D11DeviceContext,
    /// By piece, made when it first shows.
    pieces: Vec<Option<GlPiece>>,
}

struct Backend {
    gpu: Option<Gpu>,
    /// The panels could not be made: the pieces stay in the HUD.
    broken: bool,
    /// The eye whose layer is being drawn.
    eye: Option<usize>,
    /// Which panels are drawn into this frame, and went out shown.
    states: super::PanelStates,
}

// SAFETY: used only under the mutex, and its GL objects only on the render thread (made, drawn
// into and captured inside the game's 2D layer there).
unsafe impl Send for Backend {}

static BACKEND: Mutex<Option<Backend>> = Mutex::new(None);
static DRAWS: AtomicU64 = AtomicU64::new(0);

pub(super) fn release() {
    BACKEND.locked().take();
}

fn make_gpu(pieces: usize) -> Result<Gpu, String> {
    let (device, context, _) = d3d::create_device(None).map_err(|e| e.to_string())?;
    let bridge = Bridge::new(&device).map_err(|e| e.to_string())?;
    Ok(Gpu { bridge, context, pieces: (0..pieces).map(|_| None).collect() })
}

impl Gpu {
    /// Piece `index`'s canvases and panel, made when first asked for; none when they cannot be
    /// made (logged).
    fn piece(&mut self, channel: &ChannelName, pieces: &[Piece], index: usize) -> Option<&GlPiece> {
        let piece = pieces.get(index)?;
        let slot = self.pieces.get_mut(index)?;
        if slot.is_none() {
            let canvas = || Canvas::new(piece.size.0, piece.size.1).map_err(|e| e.to_string());
            let made = (|| Ok::<_, String>(GlPiece { canvases: [canvas()?, canvas()?], panel: Panel::new(self.bridge.device(), channel, piece.name, piece.size, DXGI_FORMAT_R8G8B8A8_UNORM, false)? }))();
            match made {
                Ok(made) => {
                    log!("HUD panel {}: {}x{}", piece.name, piece.size.0, piece.size.1);
                    *slot = Some(made);
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

/// Before eye `eye`'s 2D layer (0 left), on the render thread.
pub fn layer_begins(eye: usize) {
    if super::active() {
        BACKEND.locked().get_or_insert_with(|| Backend { gpu: None, broken: false, eye: None, states: super::PanelStates::default() }).eye = Some(eye);
    }
}

/// After eye `eye`'s 2D layer: after the left eye's, every shown panel is captured and published,
/// and one no longer shown published cleared, once.
pub fn layer_ends(eye: usize) {
    let mut guard = BACKEND.locked();
    let Some(backend) = guard.as_mut() else { return };
    backend.eye = None;
    if eye != LEFT_HAND {
        return;
    }
    let outs = backend.states.end();
    let Some(gpu) = backend.gpu.as_mut() else { return };
    for (index, out) in outs.into_iter().enumerate() {
        let (Some(out), Some(Some(piece))) = (out, gpu.pieces.get_mut(index)) else { continue };
        if out == PanelOut::Shown {
            for (canvas, target) in piece.canvases.iter().zip(piece.panel.targets()) {
                let (w, h) = canvas.size();
                match gpu.bridge.capture(canvas.framebuffer(), w, h) {
                    // SAFETY: a copy between two of our own textures of one size and format family.
                    Ok(captured) => unsafe { gpu.context.CopyResource(target, captured) },
                    Err(e) => note_failure(&format!("capture: {e}")),
                }
            }
        }
        if piece.panel.publish(&gpu.context, out) && backend.states.went_out(index, out) {
            super::note_change(piece.panel.piece(), out);
        }
    }
}

fn note_failure(why: &str) {
    static LOGGED: AtomicU64 = AtomicU64::new(0);
    if LOGGED.fetch_add(1, Relaxed) < 5 {
        log!("HUD panels: {why}");
    }
}

/// A draw of piece `piece` in an eye's 2D layer (`draw`, the game's own call), on the render thread
/// with the game's GL context current: run into the piece's canvases in the left eye's layer and
/// nowhere in the other's, and true. False, nothing run, outside a layer or when the piece does
/// not show (or its panel cannot be made): the game draws it its own way.
pub fn draw(piece: usize, draw: &dyn Fn()) -> bool {
    if !super::active() {
        return false;
    }
    let Some(frame) = super::frame() else { return false };
    let Some(rect) = frame.rects.get(piece).copied().flatten() else { return false };
    let mut guard = BACKEND.locked();
    let Some(backend) = guard.as_mut() else { return false };
    let Some(eye) = backend.eye.filter(|_| !backend.broken) else { return false };
    if eye != LEFT_HAND {
        return true;
    }
    if backend.gpu.is_none() {
        match make_gpu(frame.pieces.len()) {
            Ok(gpu) => {
                log!("HUD panels: open on the render thread");
                backend.gpu = Some(gpu);
            }
            Err(e) => {
                log!("HUD panels unavailable: {e}");
                backend.broken = true;
                return false;
            }
        }
    }
    let Backend { gpu, states, .. } = backend;
    let gpu = gpu.as_mut().expect("made above");
    if states.start(frame.pieces.len()) {
        for (index, shown) in frame.rects.iter().enumerate() {
            if shown.is_some()
                && let Some(piece) = gpu.piece(&frame.channel, frame.pieces, index)
            {
                piece.canvases[0].clear([0.0, 0.0, 0.0, 0.0]);
                piece.canvases[1].clear([1.0, 1.0, 1.0, 1.0]);
                states.ready(index);
            }
        }
    }
    let Some(piece) = gpu.pieces.get(piece).and_then(Option::as_ref).filter(|_| states.is_ready(piece)) else { return false };
    let Some(viewport) = viewport_for(rect, piece.canvases[0].size()) else { return false };
    for canvas in &piece.canvases {
        canvas.draw(viewport, draw);
    }
    DRAWS.fetch_add(1, Relaxed);
    true
}

/// Each piece's panel made so far, for a probe.
pub fn each_panel(mut f: impl FnMut(&Panel)) {
    if let Some(gpu) = BACKEND.locked().as_ref().and_then(|b| b.gpu.as_ref()) {
        gpu.pieces.iter().flatten().for_each(|piece| f(&piece.panel));
    }
}

/// Piece draws run into the panels so far.
pub fn draws() -> u64 {
    DRAWS.load(Relaxed)
}

/// The viewport (x, y, width, height, GL's from the bottom) that maps the 2D layer so its
/// rectangle `rect` (x, y, width, height as fractions of the layout, from the top) fills a canvas
/// of `size`.
pub fn viewport_for(rect: [f32; 4], size: (u32, u32)) -> Option<[i32; 4]> {
    let (w, h) = (size.0 as f32 / rect[2], size.1 as f32 / rect[3]);
    let (x, y) = (-rect[0] * w, size.1 as f32 - h * (1.0 - rect[1]));
    let viewport = [x, y, w, h];
    viewport.iter().all(|v| v.is_finite() && v.abs() < 32768.0).then(|| viewport.map(|v| v.round() as i32))
}

#[cfg(test)]
mod tests;
