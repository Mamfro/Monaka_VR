//! The debug menu in the headset ([`monaka_overlay::menu`]): opened and closed by holding Select
//! (Back) for half a second. A short press still reaches the game, as a tap when it is let go;
//! Select with Start is left to the capture chord. While open the menu takes the pad (the game
//! gets it idle) and floats where it opened, a metre ahead, in a row with the panels switched on
//! ([`crate::debug_panels`]): a controller points at them as at Monaka's home ([`crate::pointer`]),
//! LB / RB slide the row, B closes it. Its Tools tab saves a capture with the debug drawing (and the
//! menu) on it.

use crate::debug_panels::Place;
use crate::menu_quad::{self, MenuQuad};
use monaka_core::math::{Quat, Vec3};
use monaka_core::hold::HoldToggle;
use monaka_contract::{gamepad::Gamepad, gamepad::button};
use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_openxr::{Extent2Df, Layer, Posef, Session, Space};
use monaka_overlay::menu::panel::{PIXELS, Panel};
use monaka_overlay::quad::Rectangle;
use monaka_overlay::egui;
use monaka_overlay::menu::{Click, Menu, Settings, Status};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The panel's width (metres).
const WIDTH: f32 = 0.7;

/// A buzz: the hand, its strength (0..1) and length (seconds).
pub type Buzz = (usize, f32, f32);

/// What one read of the pad asked for: the buzzes, and the steps the row of panels slid.
#[derive(Default)]
pub struct Asked {
    pub buzzes: Vec<Buzz>,
    pub focus_step: i32,
}

pub struct HeadsetMenu<'s> {
    menu: Menu,
    /// Where it floats in the row now (none: where it opened).
    shown_at: Option<Place>,
    select: HoldToggle,
    /// Buttons held at the last read, for presses.
    held: u16,
    panel: Panel,
    quad: MenuQuad<'s>,
}

impl<'s> HeadsetMenu<'s> {
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self {
            menu: Menu::default(),
            shown_at: None,
            select: HoldToggle::new(button::BACK),
            held: 0,
            panel: Panel::new(format),
            quad: MenuQuad::new("Debug menu", format, PIXELS),
        }
    }

    pub fn open(&self) -> bool {
        self.menu.open
    }

    /// Opens the menu without its button (asked from Monaka's home, for a controller with no
    /// button for Back), ahead of `head`.
    pub fn open_now(&mut self, head: Option<(Quat, Vec3)>, status: &Status) {
        if self.menu.open {
            return;
        }
        self.menu.open = true;
        self.menu.opened(status);
        if head.is_some() {
            self.quad.open_at(head);
            self.shown_at = None;
        }
    }

    /// The pad as the controllers hold it, before anything else reads it: Select held opens or
    /// closes the menu; while it is open B closes it and LB / RB slide the row of panels. The
    /// pointer ([`crate::pointer`]) does the rest, so the pad is left for it to read.
    pub fn pad(&mut self, pad: &mut Gamepad, head: Option<(Quat, Vec3)>, now: Instant, status: &Status) -> Asked {
        let mut asked = Asked::default();
        // With Start: the capture chord's, as they come.
        if self.select.toggled(pad, now) {
            self.menu.open = !self.menu.open;
            if self.menu.open {
                self.menu.opened(status);
            }
            if self.menu.open && head.is_some() {
                self.quad.open_at(head);
                self.shown_at = None;
            }
            asked.buzzes.extend(menu_quad::TOGGLED);
        }
        if !self.menu.open {
            self.held = pad.buttons;
            return asked;
        }
        let pressed = pad.buttons & !self.held;
        self.held = pad.buttons;
        if pressed & button::B != 0 {
            self.menu.open = false;
            asked.buzzes.extend(menu_quad::TOGGLED);
            return asked;
        }
        for (bit, step) in [(button::LEFT_SHOULDER, -1), (button::RIGHT_SHOULDER, 1)] {
            if pressed & bit != 0 {
                asked.focus_step += step;
                asked.buzzes.push((if step < 0 { LEFT_HAND } else { RIGHT_HAND }, 0.25, 0.03));
            }
        }
        asked
    }

    /// Draws the panel when it shows something new or the pointer did anything on it (`events`),
    /// and hands it to the headset; the click the pointer made on it, if one.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, settings: &Settings, status: &Status, events: Vec<egui::Event>) -> Option<Click> {
        if !self.menu.open {
            return None;
        }
        let (panel, menu) = (&mut self.panel, &self.menu);
        let mut click = None;
        self.quad.show(session, context, |fresh| {
            if fresh {
                panel.forget();
            }
            let (image, clicked) = panel.draw_pointed(device, context, menu, settings, status, events)?;
            click = clicked;
            Ok(image.cloned())
        });
        click
    }

    /// The panel's image and where it floats (tracking space), while it is shown.
    pub fn shown(&self) -> Option<(&ID3D11Texture2D, Rectangle)> {
        if !self.menu.open || self.quad.swapchain().is_none() {
            return None;
        }
        let at = self.shown_place();
        self.panel.image().map(|image| (image, Rectangle { orientation: at.orientation, position: at.centre, size: at.size }))
    }

    /// Where it floats in the row now ([`crate::debug_panels::strip`]).
    pub fn set_shown(&mut self, place: Place) {
        self.shown_at = Some(place);
    }

    fn shown_place(&self) -> Place {
        self.shown_at.unwrap_or_else(|| self.place())
    }

    /// Where the panel opened, for the row of panels beside it.
    pub fn place(&self) -> Place {
        let anchor = self.quad.anchor;
        Place { orientation: anchor.orientation.xyzw(), centre: anchor.position.xyz(), size: [WIDTH, WIDTH * PIXELS[1] as f32 / PIXELS[0] as f32] }
    }

    /// The menu's model, for the 2D window, which shows it with a pointer.
    pub fn model(&mut self) -> &mut Menu {
        &mut self.menu
    }

    /// The panel's layer, while open.
    pub fn layer<'a>(&'a self, space: &'a Space<'a>) -> Option<Layer<'a>> {
        let swapchain = self.quad.swapchain().filter(|_| self.menu.open)?;
        let at = self.shown_place();
        Some(Layer::Overlay { space, swapchain, pose: Posef::new(at.orientation, at.centre), size: Extent2Df { width: at.size[0], height: at.size[1] } })
    }
}

#[cfg(test)]
mod tests;
