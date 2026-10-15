//! The Monaka menu in the headset ([`monaka_overlay::settings`]): opened and closed by holding Start
//! for half a second (a short press still reaches the game, as a tap when it is let go; Start and
//! Back together are left to the capture chord). It offers the options the launcher named
//! (`--live`); a change is acted on by the viewer and printed as a [`SETTING_LINE`], which the
//! launcher saves. While open the menu takes the pad (the game gets it idle) and floats where it
//! opened, a metre ahead.

use crate::debug_menu::Buzz;
use crate::menu_quad::{self, MenuQuad};
use monaka_core::hold::HoldToggle;
use monaka_core::manifest::SETTING_LINE;
use monaka_core::math::{Quat, Vec3};
use monaka_core::pad::{Gamepad, button};
use monaka_openxr::{Extent2Df, Layer, Session, Space};
use monaka_overlay::menu::{Press, StickRepeat};
use monaka_overlay::settings::SettingsMenu;
use monaka_overlay::settings::panel::{PIXELS, SettingsPanel};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The panel's width (metres).
const WIDTH: f32 = 0.6;

/// What one read of the pad asked for.
#[derive(Default)]
pub struct Asked {
    /// An option the player changed: its id and value.
    pub changed: Option<(&'static str, String)>,
    pub buzzes: Vec<Buzz>,
}

pub struct MonakaMenu<'s> {
    menu: SettingsMenu,
    stick: StickRepeat,
    started: Instant,
    start: HoldToggle,
    /// Buttons held at the last read, for presses.
    held: u16,
    panel: SettingsPanel,
    quad: MenuQuad<'s>,
}

impl<'s> MonakaMenu<'s> {
    /// The menu of the options `live` names (`--live`), drawn in `format`.
    pub fn new(format: DXGI_FORMAT, live: &str) -> Self {
        Self {
            menu: SettingsMenu::parse(live),
            stick: StickRepeat::default(),
            started: Instant::now(),
            start: HoldToggle::new(button::START),
            held: 0,
            panel: SettingsPanel::new(format),
            quad: MenuQuad::new("Monaka menu", format, PIXELS),
        }
    }

    /// The pad as the controllers hold it, before the game reads it: Start held opens or closes the
    /// menu; while open the menu reads the pad and leaves the game an idle one. `head`: where the
    /// head is, to place the panel. `busy`: another menu has the pad (the debug menu).
    pub fn pad(&mut self, pad: &mut Gamepad, head: Option<(Quat, Vec3)>, now: Instant, busy: bool) -> Asked {
        let mut asked = Asked::default();
        if busy || self.menu.lines.is_empty() {
            return asked;
        }
        // With Back: the capture chord's, as they come.
        if self.start.toggled(pad, now) {
            self.menu.open = !self.menu.open;
            if self.menu.open {
                self.quad.open_at(head);
            }
            asked.buzzes.extend(menu_quad::TOGGLED);
        }
        if !self.menu.open {
            self.held = pad.buttons;
            return asked;
        }
        let pressed = pad.buttons & !self.held;
        self.held = pad.buttons;
        let seconds = now.duration_since(self.started).as_secs_f64();
        let mut presses = Vec::new();
        presses.extend(self.stick.step_pad(pad, seconds));
        presses.extend([(pressed & button::A != 0, Press::Switch), (pressed & button::B != 0, Press::Close)].into_iter().filter(|(on, _)| *on).map(|(_, p)| p));
        for press in presses {
            if let Some(line) = self.menu.press(press) {
                println!("{SETTING_LINE} {}={}", line.opt.id, line.value);
                asked.changed = Some((line.opt.id, line.value.clone()));
            }
            asked.buzzes.push(menu_quad::pressed(press == Press::Switch));
        }
        *pad = Gamepad::default();
        asked
    }

    /// Draws the panel when it shows something new and hands it to the headset.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext) {
        if !self.menu.open {
            return;
        }
        let (panel, menu) = (&mut self.panel, &self.menu);
        self.quad.show(session, context, |fresh| {
            if fresh {
                panel.forget();
            }
            panel.draw(device, context, menu).map(|image| image.cloned())
        });
    }

    /// The panel's layer, while open.
    pub fn layer<'a>(&'a self, space: &'a Space<'a>) -> Option<Layer<'a>> {
        let swapchain = self.quad.swapchain().filter(|_| self.menu.open)?;
        let height = WIDTH * PIXELS[1] as f32 / PIXELS[0] as f32;
        Some(Layer::Overlay { space, swapchain, pose: self.quad.anchor, size: Extent2Df { width: WIDTH, height } })
    }
}

#[cfg(test)]
mod tests;
