//! The Monaka menu in the headset ([`monaka_overlay::settings`]): opened and closed by holding Start
//! for half a second (a short press still reaches the game, as a tap when it is let go; Start and
//! Back together are left to the capture chord). It offers the options the launcher named
//! (`--live`); a change is acted on by the viewer and printed as a [`SETTING_LINE`], which the
//! launcher saves. While open the menu takes the pad (the game gets it idle) and floats where it
//! opened, a metre ahead.

use crate::debug_menu::{Buzz, LongPress, pose};
use monaka_channel::d3d;
use monaka_core::manifest::SETTING_LINE;
use monaka_core::math::{self, Quat, Vec3};
use monaka_core::pad::{Gamepad, button};
use monaka_core::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_openxr::{Extent2Df, Layer, Posef, Session, Space, Swapchain};
use monaka_overlay::menu::{Press, StickRepeat};
use monaka_overlay::settings::SettingsMenu;
use monaka_overlay::settings::panel::{PIXELS, SettingsPanel};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Where the panel floats when it opens: this far ahead of the head, this far below the eyes, this
/// wide (metres).
const DISTANCE: f32 = 1.0;
const BELOW_EYES: f32 = 0.1;
const WIDTH: f32 = 0.6;
const CHORD: u16 = button::BACK | button::START;

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
    start: LongPress,
    /// Buttons held at the last read, for presses.
    held: u16,
    panel: SettingsPanel,
    format: DXGI_FORMAT,
    swapchain: Option<Swapchain<'s>>,
    failed: bool,
    anchor: Posef,
}

impl<'s> MonakaMenu<'s> {
    /// The menu of the options `live` names (`--live`), drawn in `format`.
    pub fn new(format: DXGI_FORMAT, live: &str) -> Self {
        Self {
            menu: SettingsMenu::parse(live),
            stick: StickRepeat::default(),
            started: Instant::now(),
            start: LongPress::new(button::START),
            held: 0,
            panel: SettingsPanel::new(format),
            format,
            swapchain: None,
            failed: false,
            anchor: pose([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, -DISTANCE]),
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
        if pad.buttons & CHORD == CHORD {
            // Both: the capture chord's, as they come.
            self.start.yield_to_chord(now);
        } else if self.start.step(pad, now) {
            self.menu.open = !self.menu.open;
            if self.menu.open
                && let Some((orientation, position)) = head
            {
                let (facing, ahead) = math::ahead_of(orientation, position, DISTANCE);
                self.anchor = pose(facing, [ahead[0], ahead[1] - BELOW_EYES, ahead[2]]);
            }
            asked.buzzes.extend([LEFT_HAND, RIGHT_HAND].map(|hand| (hand, 0.5, 0.08)));
        }
        if !self.menu.open {
            self.held = pad.buttons;
            return asked;
        }
        let pressed = pad.buttons & !self.held;
        self.held = pad.buttons;
        let seconds = now.duration_since(self.started).as_secs_f64();
        let mut presses = Vec::new();
        presses.extend(self.stick.step(f32::from(pad.thumb_lx) / 32767.0, f32::from(pad.thumb_ly) / 32767.0, seconds));
        presses.extend([(pressed & button::A != 0, Press::Switch), (pressed & button::B != 0, Press::Close)].into_iter().filter(|(on, _)| *on).map(|(_, p)| p));
        for press in presses {
            if let Some(line) = self.menu.press(press) {
                println!("{SETTING_LINE} {}={}", line.opt.id, line.value);
                asked.changed = Some((line.opt.id, line.value.clone()));
            }
            asked.buzzes.push((RIGHT_HAND, if press == Press::Switch { 0.4 } else { 0.15 }, 0.03));
        }
        *pad = Gamepad::default();
        asked
    }

    /// Draws the panel when it shows something new and hands it to the headset.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext) {
        if !self.menu.open || self.failed {
            return;
        }
        if self.swapchain.is_none() {
            match session.create_swapchain(self.format.0 as i64, PIXELS[0], PIXELS[1]) {
                Ok(swapchain) => {
                    self.swapchain = Some(swapchain);
                    self.panel.forget();
                }
                Err(e) => return self.fail(format!("no swapchain for it ({e})")),
            }
        }
        let image = match self.panel.draw(device, context, &self.menu) {
            Ok(Some(image)) => image.clone(),
            Ok(None) => return,
            Err(e) => return self.fail(e),
        };
        if let Some(swapchain) = &self.swapchain
            && let Err(e) = swapchain.fill(|target| {
                d3d::copy_texture(context, target, &image);
                d3d::flush(context);
            })
        {
            self.fail(format!("its image could not be shown ({e})"));
        }
    }

    /// The panel's layer, while open.
    pub fn layer<'a>(&'a self, space: &'a Space<'a>) -> Option<Layer<'a>> {
        let swapchain = self.swapchain.as_ref().filter(|_| self.menu.open && !self.failed)?;
        let height = WIDTH * PIXELS[1] as f32 / PIXELS[0] as f32;
        Some(Layer::Overlay { space, swapchain, pose: self.anchor, size: Extent2Df { width: WIDTH, height } })
    }

    fn fail(&mut self, why: String) {
        println!("Monaka menu: {why}; the menu is off this run");
        self.failed = true;
    }
}
