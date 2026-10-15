//! The lay-over: a D3D12 game's HUD layer at its present request (on `HudLayer12`, run once a
//! frame by the game's present entry with [`at_present`]). In play, the features' work on the frame
//! still without its HUD comes first (`crate::providers::before_hud`); then the layer with this
//! frame's HUD is taken, the dynamic HUD's pieces are cut out of it onto the hands
//! (`crate::hud_panels::d3d12::publish`, while it takes pieces), and the eye about to be taken (the
//! back buffer, in PRESENT) gets its HUD: kept out of the frame, the layer is laid back over it
//! where `HudLayer::over` puts it with the pieces left out (`crate::hud_layer::d3d12`), and over the
//! features' own images of the frame in play (`crate::providers::lay_hud_beside`); drawn into the
//! frame as well, the pieces are taken out of it (`crate::hud_panels::d3d12::take_out`). A menu is
//! the game's whole screen, shown flat: the layer goes back where the game laid it out, and from the
//! next frame's draws on the HUD goes to the frame. The layer is given back every frame.

use crate::hud_panels::d3d12 as panels;
use super::d3d12::{self, Params};
use crate::providers::{HudLayer12, before_hud, lay_hud_beside, whole_frame};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATE_PRESENT, ID3D12CommandQueue, ID3D12Resource};

/// The frame at the present request.
pub struct Present<'a> {
    /// The game's direct queue.
    pub queue: Option<&'a ID3D12CommandQueue>,
    /// The back buffer about to present (in PRESENT).
    pub buffer: Option<&'a ID3D12Resource>,
    /// The swapchain's size.
    pub size: Option<(u32, u32)>,
    /// A menu is open (the game's whole screen, shown flat).
    pub menu: bool,
    /// The eyes are drawn (stereo runs).
    pub running: bool,
}

/// What the eye about to be taken gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// The HUD is kept out of the frame: the layer goes back over it.
    LayOver,
    /// The HUD is drawn into the frame: the pieces on the hands come out of it.
    PiecesOut,
    /// A menu with the HUD drawn in: the frame as the game drew it.
    AsDrawn,
}

pub(super) fn step(kept_out: bool, menu: bool) -> Step {
    match (kept_out, menu) {
        (true, _) => Step::LayOver,
        (false, false) => Step::PiecesOut,
        (false, true) => Step::AsDrawn,
    }
}

/// How the layer goes over a frame of `size`: where the game laid it out in a menu (or without a
/// place, `over`), else at its place with the pieces on the hands `rects` left out.
pub(super) fn over_params(size: (u32, u32), menu: bool, over: Option<Params>, rects: &[[f32; 4]]) -> Params {
    let whole = whole_frame(size);
    if menu { whole } else { over.unwrap_or(whole).leaving_out(rects) }
}

/// The lay-over's work at game `G`'s present request (see the module).
pub fn at_present<G: HudLayer12>(at: Present<'_>) {
    let in_play = !at.menu && at.running;
    if in_play && let (Some(queue), Some(buffer)) = (at.queue, at.buffer) {
        before_hud(queue, buffer);
    }
    let layer = at.queue.zip(at.size).and_then(|(queue, size)| G::take_layer(queue, size));
    if let (Some(queue), Some(layer), Some(buffer), Some(size)) = (at.queue, &layer, at.buffer, at.size) {
        let rects = panels::publish(queue, layer);
        match step(G::kept_out(), at.menu) {
            Step::LayOver => {
                d3d12::lay(queue, buffer, D3D12_RESOURCE_STATE_PRESENT, layer, &over_params(size, at.menu, G::over(size, None), &rects));
                if in_play {
                    lay_hud_beside::<G>(queue, layer, size, &rects);
                }
            }
            Step::PiecesOut => {
                panels::take_out(queue, buffer, D3D12_RESOURCE_STATE_PRESENT, layer, &rects);
            }
            Step::AsDrawn => {}
        }
    }
    G::let_through(at.menu);
    drop(layer);
    if let Some(queue) = at.queue {
        G::return_layer(queue);
    }
    G::frame_end();
}
