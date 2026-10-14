//! The debug menu's panels in the headset ([`monaka_overlay::panels`]): each panel switched on in
//! the Panels tab floats next to the menu, in the first spot beside it that no other panel takes,
//! and stays there in the room (menu open or not) until switched off. Its page is drawn again a few
//! times a second.

use crate::debug_menu::pose;
use monaka_channel::d3d;
use monaka_core::math::{self, Quat, Vec3};
use monaka_openxr::{Extent2Df, Layer, Session, Space, Swapchain};
use monaka_overlay::canvas::Canvas;
use monaka_overlay::panels::{self, Content, PanelId, Panels};
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

/// Where a panel of `size` goes beside `menu`: the first spot, outward from the menu's right then
/// its left, then a row above, that no rectangle of `taken` overlaps.
pub fn free_spot(menu: &Place, size: [f32; 2], taken: &[Place]) -> Place {
    let right = math::rotate(menu.orientation, [1.0, 0.0, 0.0]);
    let up = math::rotate(menu.orientation, [0.0, 1.0, 0.0]);
    let along = |p: &Place| {
        let d = [0, 1, 2].map(|k| p.centre[k] - menu.centre[k]);
        (dot(d, right), dot(d, up))
    };
    let overlaps = |x: f32, y: f32| {
        taken.iter().chain(std::iter::once(menu)).any(|other| {
            let (ox, oy) = along(other);
            (x - ox).abs() < (size[0] + other.size[0]) * 0.5 + GAP * 0.5 && (y - oy).abs() < (size[1] + other.size[1]) * 0.5 + GAP * 0.5
        })
    };
    // Top edges level with the menu's, then the same a row up.
    let level = (size[1] - menu.size[1]) * -0.5;
    let first = (menu.size[0] + size[0]) * 0.5 + GAP;
    let step = size[0] + GAP;
    let rows = [level, level + menu.size[1].max(size[1]) + GAP];
    let spots = rows.iter().flat_map(|&y| (0..4).flat_map(move |n| [1.0f32, -1.0].map(|side| (side * (first + n as f32 * step), y))));
    let (x, y) = spots.clone().find(|(x, y)| !overlaps(*x, *y)).unwrap_or((first, level));
    let centre = [0, 1, 2].map(|k| menu.centre[k] + right[k] * x + up[k] * y);
    Place { orientation: menu.orientation, centre, size }
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

struct Shown<'s> {
    id: PanelId,
    place: Place,
    canvas: Canvas,
    swapchain: Option<Swapchain<'s>>,
    drawn_at: Option<Instant>,
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

    /// Follows the Panels tab: a panel switched on goes next to the menu (`menu`, where it is now),
    /// one switched off goes away.
    pub fn follow(&mut self, wanted: Panels, menu: &Place) {
        self.shown.retain(|s| wanted.has(s.id));
        for id in wanted.iter() {
            if self.shown.iter().any(|s| s.id == id) {
                continue;
            }
            let [w, h] = id.pixels();
            let taken: Vec<Place> = self.shown.iter().map(|s| s.place).collect();
            let place = free_spot(menu, [WIDTH, WIDTH * h as f32 / w as f32], &taken);
            self.shown.push(Shown { id, place, canvas: Canvas::new(self.format, id.pixels()), swapchain: None, drawn_at: None, failed: false });
        }
    }

    /// Draws each panel's page when it is due, and hands it to the headset.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, content: &Content<'_>, now: Instant) {
        for shown in self.shown.iter_mut().filter(|s| !s.failed) {
            if shown.drawn_at.is_some_and(|at| now.duration_since(at) < REDRAW) {
                continue;
            }
            shown.drawn_at = Some(now);
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
            let id = shown.id;
            let image = match shown.canvas.draw(device, context, |ui| panels::headset_page(ui, id, content)) {
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
                    d3d::flush(context);
                })
            {
                println!("Debug panels: {} could not be shown ({e}); it is off this run", id.label());
                shown.failed = true;
            }
        }
    }

    /// The panels' layers.
    pub fn layers<'a>(&'a self, space: &'a Space<'a>) -> impl Iterator<Item = Layer<'a>> + 'a {
        self.shown.iter().filter(|s| !s.failed && s.drawn_at.is_some()).filter_map(move |s| {
            let swapchain = s.swapchain.as_ref()?;
            Some(Layer::Overlay { space, swapchain, pose: pose(s.place.orientation, s.place.centre), size: Extent2Df { width: s.place.size[0], height: s.place.size[1] } })
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
