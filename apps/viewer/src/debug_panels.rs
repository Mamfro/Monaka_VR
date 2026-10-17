//! The debug menu's panels in the headset ([`monaka_overlay::panels`]): each panel switched on in
//! the Panels tab is a page of the debug ring, after the menu ([`crate::debug_views`],
//! [`crate::ring`]), drawn the size of every ring panel. A page is drawn again a few times a second,
//! and at once when the pointer does anything on it or it comes to or leaves the front.

use monaka_overlay::canvas::{Canvas, Cursor};
use monaka_overlay::egui;
use monaka_overlay::home::place;
use monaka_overlay::panels::{self, Asked, Content, PanelId, Panels};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

#[cfg(test)]
mod tests;

/// How often a panel's page is drawn again.
const REDRAW: Duration = Duration::from_millis(250);

struct Shown {
    id: PanelId,
    canvas: Canvas,
    /// When it was last drawn, and whether it was in front then.
    drawn_at: Option<Instant>,
    front: bool,
    cursor: Cursor,
}

pub struct HeadsetPanels {
    format: DXGI_FORMAT,
    /// The panels switched on, in the Panels tab's order: the ring's pages after the menu.
    shown: Vec<Shown>,
}

impl HeadsetPanels {
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self { format, shown: Vec::new() }
    }

    /// Follows the Panels tab: a panel switched on is shown (in the tab's order), one switched off
    /// goes away.
    pub fn follow(&mut self, wanted: Panels) {
        if self.shown.iter().map(|s| s.id).eq(wanted.iter()) {
            return;
        }
        let mut before = std::mem::take(&mut self.shown);
        for id in wanted.iter() {
            let kept = before.iter().position(|s| s.id == id).map(|at| before.remove(at));
            self.shown.push(kept.unwrap_or_else(|| Shown { id, canvas: Canvas::new(self.format, place::PIXELS), drawn_at: None, front: false, cursor: Cursor::default() }));
        }
    }

    /// How many panels are shown.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    /// Draws panel `index`'s page (`content`: what it shows) when it is due, came to or left the
    /// `front`, the pointer did anything on it (`events`) or its image went somewhere new
    /// (`fresh`); its image when drawn, and what was asked on it, if anything.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, index: usize, device: &ID3D11Device, context: &ID3D11DeviceContext, content: &Content<'_>, front: bool, events: Vec<egui::Event>, now: Instant, fresh: bool) -> Result<(Option<ID3D11Texture2D>, Option<Asked>), String> {
        let Some(shown) = self.shown.get_mut(index) else { return Ok((None, None)) };
        let due = shown.drawn_at.is_none_or(|at| now.duration_since(at) >= REDRAW);
        if !(due || fresh || front != shown.front || !events.is_empty()) {
            return Ok((None, None));
        }
        shown.drawn_at = Some(now);
        shown.front = front;
        shown.cursor.follow(&events);
        let (id, cursor) = (shown.id, shown.cursor);
        let mut asked = None;
        let image = shown
            .canvas
            .draw_with(device, context, events, |ui| {
                asked = panels::ring_page(ui, id, content, front);
                cursor.paint(ui.ctx(), monaka_overlay::ring_page::DEBUG.accent);
            })
            .map_err(|e| format!("{} could not be drawn ({e})", id.label()))?;
        Ok((image.cloned(), asked))
    }

    /// Panel `index`'s latest image, for captures with the drawings.
    pub fn image(&self, index: usize) -> Option<&ID3D11Texture2D> {
        self.shown.get(index)?.canvas.image()
    }
}
