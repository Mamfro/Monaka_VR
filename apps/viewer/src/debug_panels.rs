//! The debug menu's panels in the headset ([`monaka_overlay::panels`]): the panels switched on in
//! the Panels tab float in a row with the menu, the menu first ([`strip`]). While the menu is open
//! LB / RB slide the row so another panel is where the menu opened; with the menu closed the panels
//! stay where they were until switched off. A controller points at them ([`crate::pointer`]). A
//! page is drawn again a few times a second, and at once when the pointer does anything on it.

use monaka_channel::d3d;
use monaka_core::math::{self, Quat, Vec3};

use monaka_openxr::{Extent2Df, Layer, Posef, Session, Space, Swapchain};
use monaka_overlay::canvas::{Canvas, Cursor};
use monaka_overlay::egui;
use monaka_overlay::panels::{self, Asked, Content, PanelId, Panels};
use monaka_overlay::quad::Rectangle;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

#[cfg(test)]
mod tests;

/// A panel's width in the room (metres); its height follows its image.
const WIDTH: f32 = 0.5;
/// The gap between panels side by side (metres).
const GAP: f32 = 0.03;
/// How often a panel's page is drawn again.
const REDRAW: Duration = Duration::from_millis(250);

/// A rectangle facing the same way as the menu: its centre and orientation (tracking space) and
/// its width and height (metres).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub orientation: Quat,
    pub centre: Vec3,
    pub size: [f32; 2],
}

/// The menu and the panels in a row (`sizes`, the menu's first), left to right a gap apart, their
/// tops level with the menu's, slid along it so the centre of `active` is where the menu opened
/// (`menu`, facing the way it faces).
pub fn strip(menu: &Place, sizes: &[[f32; 2]], active: usize) -> Vec<Place> {
    let right = math::rotate(menu.orientation, [1.0, 0.0, 0.0]);
    let up = math::rotate(menu.orientation, [0.0, 1.0, 0.0]);
    let mut along = 0.0;
    let centres: Vec<f32> = sizes
        .iter()
        .map(|size| {
            let centre = along + size[0] * 0.5;
            along += size[0] + GAP;
            centre
        })
        .collect();
    let slide = centres.get(active.min(sizes.len().saturating_sub(1))).copied().unwrap_or(0.0);
    sizes
        .iter()
        .zip(centres)
        .map(|(size, centre)| {
            let (x, y) = (centre - slide, (menu.size[1] - size[1]) * 0.5);
            Place { orientation: menu.orientation, centre: [0, 1, 2].map(|k| menu.centre[k] + right[k] * x + up[k] * y), size: *size }
        })
        .collect()
}

/// Panel `id`'s size in the room (metres).
pub fn size(id: PanelId) -> [f32; 2] {
    let [w, h] = id.pixels();
    [WIDTH, WIDTH * h as f32 / w as f32]
}

struct Shown<'s> {
    id: PanelId,
    place: Place,
    canvas: Canvas,
    swapchain: Option<Swapchain<'s>>,
    drawn_at: Option<Instant>,
    cursor: Cursor,
    failed: bool,
}

pub struct HeadsetPanels<'s> {
    format: DXGI_FORMAT,
    shown: Vec<Shown<'s>>,
}

impl<'s> HeadsetPanels<'s> {
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self { format, shown: Vec::new() }
    }

    /// Follows the Panels tab: a panel switched on is shown, one switched off goes away. `places`:
    /// where each of `wanted` goes in the row ([`strip`], in `wanted`'s order); a panel already shown
    /// moves there only when `slide` (the menu open), a new one goes there at once.
    pub fn follow(&mut self, wanted: Panels, places: &[Place], slide: bool) {
        self.shown.retain(|s| wanted.has(s.id));
        for (id, place) in wanted.iter().zip(places) {
            match self.shown.iter_mut().find(|s| s.id == id) {
                Some(shown) if slide => shown.place = *place,
                Some(_) => {}
                None => self.shown.push(Shown { id, place: *place, canvas: Canvas::new(self.format, id.pixels()), swapchain: None, drawn_at: None, cursor: Cursor::default(), failed: false }),
            }
        }
    }

    /// Draws each panel's page when it is due or the pointer did anything on it (`events`, by
    /// panel), and hands it to the headset; what was asked on one, if anything.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, content: &Content<'_>, now: Instant, mut events: impl FnMut(PanelId) -> Vec<egui::Event>) -> Option<Asked> {
        let mut asked = None;
        for shown in self.shown.iter_mut().filter(|s| !s.failed) {
            let pointed = events(shown.id);
            if pointed.is_empty() && shown.drawn_at.is_some_and(|at| now.duration_since(at) < REDRAW) {
                continue;
            }
            shown.drawn_at = Some(now);
            shown.cursor.follow(&pointed);
            if shown.swapchain.is_none() {
                let [w, h] = shown.id.pixels();
                match session.create_swapchain(self.format.0 as i64, w, h) {
                    Ok(swapchain) => shown.swapchain = Some(swapchain),
                    Err(e) => {
                        println!("Debug panels: no swapchain for {} ({e}); it is off this run", shown.id.label());
                        shown.failed = true;
                        continue;
                    }
                }
            }
            let (id, cursor) = (shown.id, shown.cursor);
            let image = match shown.canvas.draw_with(device, context, pointed, |ui| {
                asked = panels::headset_page(ui, id, content).or(asked.take());
                cursor.paint(ui.ctx());
            }) {
                Ok(Some(image)) => image.clone(),
                Ok(None) => continue,
                Err(e) => {
                    println!("Debug panels: {} could not be drawn ({e}); it is off this run", id.label());
                    shown.failed = true;
                    continue;
                }
            };
            if let Some(swapchain) = &shown.swapchain
                && let Err(e) = swapchain.fill(|target| {
                    d3d::copy_texture(context, target, &image);
                })
            {
                println!("Debug panels: {} could not be shown ({e}); it is off this run", id.label());
                shown.failed = true;
            }
        }
        asked
    }

    /// The panels' layers.
    pub fn layers<'a>(&'a self, space: &'a Space<'a>) -> impl Iterator<Item = Layer<'a>> + 'a {
        self.shown.iter().filter(|s| !s.failed && s.drawn_at.is_some()).filter_map(move |s| {
            let swapchain = s.swapchain.as_ref()?;
            Some(Layer::Overlay { space, swapchain, pose: Posef::new(s.place.orientation, s.place.centre), size: Extent2Df { width: s.place.size[0], height: s.place.size[1] } })
        })
    }

    /// Each panel's latest image and where it floats, for captures with the drawings.
    pub fn images(&self) -> impl Iterator<Item = (&ID3D11Texture2D, Rectangle)> {
        self.shown.iter().filter(|s| !s.failed && s.swapchain.is_some()).filter_map(|s| {
            let image = s.canvas.image()?;
            Some((image, Rectangle { orientation: s.place.orientation, position: s.place.centre, size: s.place.size }))
        })
    }
}
