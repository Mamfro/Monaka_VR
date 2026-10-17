//! The D3D12 HUD lay-over: a game's UI layer laid back over an eye frame (alternate-eye stereo with
//! the HUD kept out of the frame, its draws redirected into a layer of their own). The game's HUD,
//! menus and wheels, drawn for the whole screen, come back in a rectangle of the eye at the HUD's
//! distance and size ([`monaka_core::hud`]'s placement), so they float in front of the player instead of covering the eye. The crosshair is
//! drawn at the game's aim point, the dynamic HUD's pieces (on the hands) are left out, and a dim
//! the game lays over the whole screen (behind a wheel) stays over the whole eye instead of
//! shrinking into a box (`hudover12.hlsl`). The frame is a swapchain back buffer (no unordered
//! access), so it is copied out, laid over into a scratch texture and copied back, on the game
//! queue before the eye is taken: the eye and the monitor both show it.

use crate::framepass12::{FramePass, Lazy};
use monaka_channel::d3d12::{self, barrier};
use monaka_producer::log;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::s;

const SOURCE: &str = include_str!("hudover12.hlsl");
/// Most pieces left out.
const PIECES: usize = 5;
/// The share of the frame's height around the HUD's centre drawn at the aim point (the crosshair,
/// the prompts beside it).
pub const CROSS_RADIUS: f32 = 0.03;

/// How the layer goes over one frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Params {
    /// Where the layer goes (x, y, width, height, the frame's pixels).
    pub rect: [f32; 4],
    /// The game's aim point from the frame's centre, pixels (x right, y down).
    pub aim: [f32; 2],
    /// Radius around the layer's centre drawn at the aim point instead (crosshair, prompts), pixels.
    pub cross_radius: f32,
    /// Rectangles of the layer (x, y, width, height; frame pixels) left out: the pieces on the hands.
    pub pieces: [[f32; 4]; PIECES],
    pub piece_count: usize,
    /// The layer's alpha is how much of what is behind shows through (1 where nothing is drawn, as
    /// id Tech 6 keeps its GUI), not its coverage.
    pub see_through: bool,
}

impl Params {
    /// The pieces `rects` (at most five) left out.
    pub fn leaving_out(mut self, rects: &[[f32; 4]]) -> Self {
        self.piece_count = rects.len().min(PIECES);
        self.pieces[..self.piece_count].copy_from_slice(&rects[..self.piece_count]);
        self
    }
}

/// The shader's constants (root constants; `Piece` starts on a 16-byte boundary).
#[repr(C)]
#[derive(Clone, Copy)]
struct Constants {
    width: u32,
    height: u32,
    cross_radius: f32,
    pieces: f32,
    rect: [f32; 4],
    aim: [f32; 2],
    see_through: f32,
    pad: f32,
    piece_rects: [[f32; 4]; PIECES],
}

static OVER: Lazy<FramePass> = Lazy::new();
static LAID: AtomicU64 = AtomicU64::new(0);

fn make(device: &ID3D12Device) -> Result<FramePass, String> {
    FramePass::new(device, (size_of::<Constants>() / 4) as u32, SOURCE, "hudover12.hlsl", s!("Over"), true)
}

/// Lays the UI layer `ui` (in COMMON, the frame's size) over `frame` (in `state`, a back buffer)
/// as `params` say, on `queue`. Returns whether it was recorded (otherwise the frame has no HUD).
pub fn lay(queue: &ID3D12CommandQueue, frame: &ID3D12Resource, state: D3D12_RESOURCE_STATES, ui: &ID3D12Resource, params: &Params) -> bool {
    OVER.with(queue, make, "HUD lay-over unavailable", |over| lay_with(over, queue, frame, state, ui, params)).unwrap_or(false)
}

/// [`lay`] recorded on the caller's open `list` instead (several frames on one list, in order),
/// `queue` only naming the device the pass is made on. The pass and its textures are shared with
/// [`lay`], so the frames have the back buffer's size and format.
pub fn lay_on(queue: &ID3D12CommandQueue, list: &ID3D12GraphicsCommandList, frame: &ID3D12Resource, state: D3D12_RESOURCE_STATES, ui: &ID3D12Resource, params: &Params) -> bool {
    OVER.with(queue, make, "HUD lay-over unavailable", |over| {
        let ready = ready(over, frame, ui);
        if ready {
            record(over, list, frame, state, ui, params);
            counted(over, params);
        }
        ready
    })
    .unwrap_or(false)
}

fn lay_with(over: &mut FramePass, queue: &ID3D12CommandQueue, frame: &ID3D12Resource, state: D3D12_RESOURCE_STATES, ui: &ID3D12Resource, params: &Params) -> bool {
    if !ready(over, frame, ui) {
        return false;
    }
    let Ok(Some(list)) = over.recorder.begin() else { return false };
    record(over, &list, frame, state, ui, params);
    match over.recorder.submit(queue) {
        Ok(_) => {
            counted(over, params);
            true
        }
        Err(e) => {
            monaka_producer::log_first(3, format_args!("HUD lay-over not run: {e}"));
            false
        }
    }
}

/// Whether the pass is ready for `frame` with the layer `ui` (its textures and views made).
fn ready(over: &mut FramePass, frame: &ID3D12Resource, ui: &ID3D12Resource) -> bool {
    // SAFETY: reads resource descriptors.
    let (desc, ui_desc) = unsafe { (frame.GetDesc(), ui.GetDesc()) };
    if ui_desc.Width != desc.Width || ui_desc.Height != desc.Height {
        return false;
    }
    if let Err(why) = over.ensure(&desc) {
        monaka_producer::log_first(3, format_args!("HUD lay-over not run: {why}"));
        return false;
    }
    over.view(ui);
    over.source.is_some() && over.out.is_some()
}

/// Records the lay-over on `list` (the pass [`ready`] for the frame).
fn record(over: &FramePass, list: &ID3D12GraphicsCommandList, frame: &ID3D12Resource, state: D3D12_RESOURCE_STATES, ui: &ID3D12Resource, params: &Params) {
    let (Some(source), Some(out)) = (over.source.clone(), over.out.clone()) else { return };
    let mut piece_rects = [[0.0; 4]; PIECES];
    piece_rects[..params.piece_count].copy_from_slice(&params.pieces[..params.piece_count]);
    let constants = Constants {
        width: over.size.0,
        height: over.size.1,
        cross_radius: params.cross_radius,
        pieces: params.piece_count as f32,
        rect: params.rect,
        aim: params.aim,
        see_through: if params.see_through { 1.0 } else { 0.0 },
        pad: 0.0,
        piece_rects,
    };
    const SRV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
    d3d12::copy_whole(list, &source, frame, state);
    // SAFETY: records copies and compute work on an open list with our own heap, root signature
    // and pipeline; every resource is live until the list has executed (its recorder's fence).
    unsafe {
        barrier(list, &source, D3D12_RESOURCE_STATE_COMMON, SRV);
        barrier(list, ui, D3D12_RESOURCE_STATE_COMMON, SRV);
        barrier(list, &out, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        over.bind(list);
        list.SetComputeRoot32BitConstants(0, (size_of::<Constants>() / 4) as u32, &constants as *const Constants as *const c_void, 0);
        list.Dispatch(over.size.0.div_ceil(8), over.size.1.div_ceil(8), 1);
        barrier(list, &out, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        barrier(list, frame, state, D3D12_RESOURCE_STATE_COPY_DEST);
        list.CopyResource(frame, &out);
        barrier(list, frame, D3D12_RESOURCE_STATE_COPY_DEST, state);
        barrier(list, &out, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON);
        barrier(list, ui, SRV, D3D12_RESOURCE_STATE_COMMON);
        barrier(list, &source, SRV, D3D12_RESOURCE_STATE_COMMON);
    }
}

/// Counts a frame laid over (logged now and then).
fn counted(over: &FramePass, params: &Params) {
    let n = LAID.fetch_add(1, Relaxed);
    if n == 0 || n.is_multiple_of(600) {
        log!("HUD lay-over {n}: the layer in {:.0?} of {}x{}, {} pieces left out", params.rect, over.size.0, over.size.1, params.piece_count);
    }
}

/// The end of a run: waits for the GPU and lets the textures go.
pub fn close() {
    if let Some(over) = OVER.take() {
        over.close();
    }
    let laid = LAID.load(Relaxed);
    if laid > 0 {
        log!("HUD lay-over: {laid} frames");
    }
}

#[cfg(test)]
mod tests;
