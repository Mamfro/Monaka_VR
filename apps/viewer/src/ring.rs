//! A ring of flat panels round the player, pointed at with a controller, the same for Monaka's
//! home ([`crate::home`]) and the debug menu ([`crate::debug_views`]): where the panels stand
//! ([`monaka_overlay::home::place`]), each one's swapchain and image ([`MenuQuad`]), LB / RB
//! turning the ring with the panels going round to their new places, and the pointer on them
//! ([`crate::pointer`]). Every panel is the same: [`place::WIDTH`] wide, [`place::PIXELS`] drawn.

use crate::debug_menu::Buzz;
use crate::menu_quad::MenuQuad;
use crate::pointer::{Pointer, Target};
use monaka_contract::gamepad::Gamepad;
use monaka_contract::protocol::HandPose;
use monaka_core::math::{Quat, Vec3};
use monaka_openxr::{Extent2Df, Layer, Posef, Session, Space};
use monaka_overlay::canvas::PIXELS_PER_POINT;
use monaka_overlay::egui;
use monaka_overlay::home::place::{self, Ring, TURN_SECONDS, Turning, slot_angle};
use monaka_overlay::quad::Rectangle;
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The ring turning: how, and since when.
struct Turn {
    turning: Turning,
    started: Instant,
}

pub struct RingView<'s> {
    /// The ring's name, for the line that says a panel of it is off.
    name: &'static str,
    format: DXGI_FORMAT,
    quads: Vec<MenuQuad<'s>>,
    /// Where the ring stands (none until placed), the panel in front, each panel's angle round the
    /// ring now, and a turn under way.
    ring: Option<Ring>,
    front: usize,
    angles: Vec<f32>,
    turn: Option<Turn>,
    pointer: Pointer,
    /// What the pointer did on each panel since it was last drawn.
    events: Vec<Vec<egui::Event>>,
}

impl<'s> RingView<'s> {
    /// A ring `name` of `pages` panels drawn in `format`, panel `front` in front; it stands nowhere
    /// until placed ([`RingView::place_at`]).
    pub fn new(name: &'static str, format: DXGI_FORMAT, pages: usize, front: usize) -> Self {
        let mut ring = Self { name, format, quads: Vec::new(), ring: None, front, angles: Vec::new(), turn: None, pointer: Pointer::default(), events: Vec::new() };
        ring.set_pages(pages);
        ring
    }

    /// The panel in front.
    pub fn front(&self) -> usize {
        self.front
    }

    /// Whether the ring stands somewhere yet.
    pub fn placed(&self) -> bool {
        self.ring.is_some()
    }

    /// `pages` panels from now on, added or taken away at the end; they settle in their places, the
    /// one in front kept (or the last, when it went).
    pub fn set_pages(&mut self, pages: usize) {
        if pages == self.quads.len() && pages == self.angles.len() {
            return;
        }
        let (name, format) = (self.name, self.format);
        self.quads.resize_with(pages, || MenuQuad::new(name, format, place::PIXELS));
        self.events.resize_with(pages, Vec::new);
        self.front = self.front.min(pages.saturating_sub(1));
        self.settle();
    }

    /// Stands the ring round a head at `head` (`orientation`, `position`), the panel in front
    /// straight ahead of where it faces.
    pub fn place_at(&mut self, (orientation, position): (Quat, Vec3)) {
        self.ring = Some(Ring::at(orientation, position));
        self.settle();
    }

    /// Brings panel `page` in front at once (the ring opened on it).
    pub fn show_in_front(&mut self, page: usize) {
        self.front = page.min(self.quads.len().saturating_sub(1));
        self.settle();
    }

    /// Turns the ring `steps` (RB 1, LB -1) from `now`: another panel comes in front, every panel
    /// going round to its new place over [`TURN_SECONDS`].
    pub fn turn(&mut self, steps: isize, now: Instant) {
        let pages = self.quads.len();
        if pages < 2 {
            return;
        }
        self.front = (self.front as isize + steps).rem_euclid(pages as isize) as usize;
        self.turn = Some(Turn { turning: Turning::new(&self.angles, self.front, steps), started: now });
    }

    /// The panels where a turn under way has them at `now`, settled in their places once it is
    /// done.
    pub fn step(&mut self, now: Instant) {
        if let Some(turn) = &self.turn {
            let progress = now.duration_since(turn.started).as_secs_f32() / TURN_SECONDS;
            if progress >= 1.0 {
                return self.settle();
            }
            self.angles = turn.turning.at(progress);
        }
        self.stand();
    }

    /// Every panel in its place, no turn under way.
    fn settle(&mut self) {
        let pages = self.quads.len();
        self.turn = None;
        self.angles = (0..pages).map(|page| slot_angle(page, self.front, pages)).collect();
        self.stand();
    }

    /// The quads where their angles round the ring put them.
    fn stand(&mut self) {
        let Some(ring) = self.ring else { return };
        for (quad, angle) in self.quads.iter_mut().zip(&self.angles) {
            let (orientation, centre) = ring.at_angle(*angle);
            quad.anchor = Posef::new(orientation, centre);
        }
    }

    /// One read of the pointer over the panels (`shown`: which take it), with the pad and the
    /// controllers' aim poses: the buzzes for a click. Scrolling with nothing pointed at goes to
    /// the panel in front.
    pub fn point(&mut self, pad: &Gamepad, hands: &[HandPose; 2], shown: impl Fn(usize) -> bool) -> Vec<Buzz> {
        let points = egui::vec2(place::PIXELS[0] as f32, place::PIXELS[1] as f32) / PIXELS_PER_POINT;
        let size = [place::WIDTH, place::height()];
        // A panel not shown is where no ray meets it: far behind the head.
        let away = Target { orientation: monaka_core::math::IDENTITY, centre: [0.0, -1000.0, 0.0], size: [0.0, 0.0], points };
        let targets: Vec<Target> = match self.ring {
            Some(_) => self
                .quads
                .iter()
                .enumerate()
                .map(|(page, quad)| if shown(page) { Target { orientation: quad.anchor.orientation.xyzw(), centre: quad.anchor.position.xyz(), size, points } } else { away })
                .collect(),
            None => Vec::new(),
        };
        self.pointer.step(pad, hands, &targets, &mut self.events, self.front)
    }

    /// The pointer gone from the panels (the ring closed).
    pub fn leave(&mut self) {
        self.pointer.leave(&mut self.events);
    }

    /// What the pointer did on panel `page` since this was last asked.
    pub fn take_events(&mut self, page: usize) -> Vec<egui::Event> {
        self.events.get_mut(page).map(std::mem::take).unwrap_or_default()
    }

    /// Hands panel `page`'s image to the headset, `draw`n when it shows something new (its
    /// argument: the swapchain is new, so the image must be drawn whole).
    pub fn show(&mut self, page: usize, session: &'s Session<'_>, context: &ID3D11DeviceContext, draw: impl FnOnce(bool) -> Result<Option<ID3D11Texture2D>, String>) {
        if let Some(quad) = self.quads.get_mut(page) {
            quad.show(session, context, draw);
        }
    }

    /// Panel `page`'s layer, once the ring stands somewhere and its image is there.
    pub fn layer<'a>(&'a self, page: usize, space: &'a Space<'a>) -> Option<Layer<'a>> {
        self.ring?;
        let quad = self.quads.get(page)?;
        Some(Layer::Overlay { space, swapchain: quad.swapchain()?, pose: quad.anchor, size: Extent2Df { width: place::WIDTH, height: place::height() } })
    }

    /// Where panel `page` floats (tracking space), for captures with the drawings on them.
    pub fn rectangle(&self, page: usize) -> Option<Rectangle> {
        self.ring?;
        let quad = self.quads.get(page)?;
        Some(Rectangle { orientation: quad.anchor.orientation.xyzw(), position: quad.anchor.position.xyz(), size: [place::WIDTH, place::height()] })
    }

    #[cfg(test)]
    pub fn pointer(&self) -> &Pointer {
        &self.pointer
    }

    #[cfg(test)]
    pub fn events(&self, page: usize) -> &[egui::Event] {
        &self.events[page]
    }
}

#[cfg(test)]
mod tests;
