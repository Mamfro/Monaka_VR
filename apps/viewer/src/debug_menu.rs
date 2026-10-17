//! The debug menu in the headset ([`monaka_overlay::menu`]): opened and closed by holding Select
//! (Back) for half a second. A short press still reaches the game, as a tap when it is let go;
//! Select with Start is left to the capture chord. While open the menu takes the pad (the game
//! gets it idle). It is the first panel of the debug ring, the panels switched on beside it
//! ([`crate::debug_views`], [`crate::ring`], as Monaka's home): a controller points at them, LB / RB
//! turn the ring, B closes it. Its Tools tab saves a capture with the debug drawing (and the menu)
//! on it.

use crate::menu_quad;
use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_contract::{gamepad::Gamepad, gamepad::button};
use monaka_core::hold::HoldToggle;
use monaka_overlay::egui;
use monaka_overlay::menu::panel::Panel;
use monaka_overlay::menu::{Click, Menu, Settings, Status};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// A buzz: the hand, its strength (0..1) and length (seconds).
pub type Buzz = (usize, f32, f32);

/// What one read of the pad asked for: the buzzes, and the steps the ring turned.
#[derive(Default)]
pub struct Asked {
    pub buzzes: Vec<Buzz>,
    pub turn: i32,
}

pub struct HeadsetMenu {
    menu: Menu,
    select: HoldToggle,
    /// Buttons held at the last read, for presses.
    held: u16,
    panel: Panel,
}

impl HeadsetMenu {
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self { menu: Menu::default(), select: HoldToggle::new(button::BACK), held: 0, panel: Panel::new(format) }
    }

    pub fn open(&self) -> bool {
        self.menu.open
    }

    /// Opens the menu without its button (asked from Monaka's home, for a controller with no
    /// button for Back).
    pub fn open_now(&mut self, status: &Status) {
        if !self.menu.open {
            self.menu.open = true;
            self.menu.opened(status);
        }
    }

    /// The pad as the controllers hold it, before anything else reads it: Select held opens or
    /// closes the menu; while it is open B closes it and LB / RB turn the ring. The pointer
    /// ([`crate::pointer`]) does the rest, so the pad is left for it to read.
    pub fn pad(&mut self, pad: &mut Gamepad, now: Instant, status: &Status) -> Asked {
        let mut asked = Asked::default();
        // With Start: the capture chord's, as they come.
        if self.select.toggled(pad, now) {
            self.menu.open = !self.menu.open;
            if self.menu.open {
                self.menu.opened(status);
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
                asked.turn += step;
                asked.buzzes.push((if step < 0 { LEFT_HAND } else { RIGHT_HAND }, 0.25, 0.03));
            }
        }
        asked
    }

    /// Draws the menu's page when it shows something new, came to or left the `front`, the pointer
    /// did anything on it (`events`), or its image went somewhere new (`fresh`); the image when
    /// drawn, and the click the pointer made, if one.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, settings: &Settings, status: &Status, front: bool, events: Vec<egui::Event>, fresh: bool) -> Result<(Option<ID3D11Texture2D>, Option<Click>), String> {
        if fresh {
            self.panel.forget();
        }
        let (image, click) = self.panel.draw(device, context, &self.menu, settings, status, front, events).map_err(|e| e.to_string())?;
        Ok((image.cloned(), click))
    }

    /// The menu's latest image, while it is open.
    pub fn image(&self) -> Option<&ID3D11Texture2D> {
        self.panel.image().filter(|_| self.menu.open)
    }

    /// The menu's model, for the 2D window, which shows it with a pointer, and for its clicks.
    pub fn model(&mut self) -> &mut Menu {
        &mut self.menu
    }
}

#[cfg(test)]
mod tests;
