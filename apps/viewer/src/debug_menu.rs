//! The debug menu in the headset ([`monaka_overlay::menu`]): opened and closed by holding Select
//! (Back) for half a second. A short press still reaches the game, as a tap when it is let go;
//! Select with Start is left to the capture chord. While open the menu takes the pad (the game
//! gets it idle) and floats where it opened, a metre ahead, in a row with the panels switched on
//! ([`crate::debug_panels`]): LB / RB pick the one the pad drives, the stick or the D-pad and A
//! drive it (the menu's own presses go to the menu), the right stick scrolls a panel's log, LT / RT
//! change the menu's tab and B closes it. Its Tools tab saves a capture with the debug drawing
//! (and the menu) on it.

use crate::debug_panels::Place;
use crate::menu_quad::{self, MenuQuad};
use monaka_core::math::{Quat, Vec3};
use monaka_core::hold::HoldToggle;
use monaka_core::pad::{Gamepad, button};
use monaka_core::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_openxr::{Extent2Df, Layer, Posef, Session, Space};
use monaka_overlay::menu::panel::{PIXELS, Panel};
use monaka_overlay::quad::Rectangle;
use monaka_overlay::menu::{Command, Menu, Press, Settings, Status, StickRepeat};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The panel's width (metres).
const WIDTH: f32 = 0.7;
/// A trigger past this (of 255) is pressed.
const TRIGGER: u8 = 160;

/// A buzz: the hand, its strength (0..1) and length (seconds).
pub type Buzz = (usize, f32, f32);

/// What one read of the pad asked for: of the menu, and of the panel the pad drives when it is
/// not the menu (its presses, the steps through the row, the lines scrolled up).
#[derive(Default)]
pub struct Asked {
    pub command: Option<Command>,
    pub buzzes: Vec<Buzz>,
    pub panel_presses: Vec<Press>,
    pub focus_step: i32,
    pub scroll: i32,
}

/// Lines a log scrolls each push of the right stick.
const SCROLL_LINES: i32 = 3;

pub struct HeadsetMenu<'s> {
    menu: Menu,
    stick: StickRepeat,
    /// The right stick, scrolling a panel's log.
    scroll_stick: StickRepeat,
    /// Where it floats in the row now (none: where it opened).
    shown_at: Option<Place>,
    started: Instant,
    select: HoldToggle,
    /// Buttons and triggers held at the last read, for presses.
    held: u16,
    triggers: [bool; 2],
    panel: Panel,
    quad: MenuQuad<'s>,
}

impl<'s> HeadsetMenu<'s> {
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self {
            menu: Menu::default(),
            stick: StickRepeat::default(),
            scroll_stick: StickRepeat::default(),
            shown_at: None,
            started: Instant::now(),
            select: HoldToggle::new(button::BACK),
            held: 0,
            triggers: [false; 2],
            panel: Panel::new(format),
            quad: MenuQuad::new("Debug menu", format, PIXELS),
        }
    }

    pub fn open(&self) -> bool {
        self.menu.open
    }

    /// The pad as the controllers hold it, before anything else reads it: Select held opens or
    /// closes the menu; while the menu is open it reads the pad and leaves the game an idle one.
    /// `head`: where the head is, to place the panel; `drives_menu`: the pad drives the menu (else a
    /// panel beside it: the presses are handed back).
    pub fn pad(&mut self, pad: &mut Gamepad, head: Option<(Quat, Vec3)>, now: Instant, settings: &mut Settings, status: &Status, drives_menu: bool) -> Asked {
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
        let triggers = [pad.left_trigger >= TRIGGER, pad.right_trigger >= TRIGGER];
        let pulled = [triggers[0] && !self.triggers[0], triggers[1] && !self.triggers[1]];
        self.triggers = triggers;
        let seconds = now.duration_since(self.started).as_secs_f64();
        let mut presses = Vec::new();
        presses.extend(self.stick.step_pad(pad, seconds));
        presses.extend([(pressed & button::A != 0, Press::Switch), (pressed & button::B != 0, Press::Close), (pulled[0], Press::PreviousTab), (pulled[1], Press::NextTab)].into_iter().filter(|(on, _)| *on).map(|(_, p)| p));
        for press in presses {
            // Closing and the tabs are always the menu's.
            if drives_menu || matches!(press, Press::Close | Press::PreviousTab | Press::NextTab) {
                asked.command = asked.command.or(self.menu.press(press, settings, status));
            } else {
                asked.panel_presses.push(press);
            }
            asked.buzzes.push(menu_quad::pressed(press == Press::Switch));
        }
        for (bit, step) in [(button::LEFT_SHOULDER, -1), (button::RIGHT_SHOULDER, 1)] {
            if pressed & bit != 0 {
                asked.focus_step += step;
                asked.buzzes.push((if step < 0 { LEFT_HAND } else { RIGHT_HAND }, 0.25, 0.03));
            }
        }
        match self.scroll_stick.step(0.0, f32::from(pad.thumb_ry) / 32767.0, seconds) {
            Some(Press::Up) => asked.scroll += SCROLL_LINES,
            Some(Press::Down) => asked.scroll -= SCROLL_LINES,
            _ => {}
        }
        *pad = Gamepad::default();
        asked
    }

    /// Draws the panel when it shows something new (`outlined`: the pad drives it, beside other
    /// panels) and hands it to the headset.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, settings: &Settings, status: &Status, outlined: bool) {
        if !self.menu.open {
            return;
        }
        let (panel, menu) = (&mut self.panel, &self.menu);
        self.quad.show(session, context, |fresh| {
            if fresh {
                panel.forget();
            }
            panel.draw(device, context, menu, settings, status, outlined).map(|image| image.cloned())
        });
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
