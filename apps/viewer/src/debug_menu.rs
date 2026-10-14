//! The debug menu in the headset ([`monaka_overlay::menu`]): opened and closed by holding Select
//! (Back) for half a second. A short press still reaches the game, as a tap when it is let go;
//! Select with Start is left to the capture chord. While open the menu takes the pad (the game
//! gets it idle) and floats where it opened, a metre ahead. Its Tools tab saves a capture with the
//! debug drawing (and the menu) on it.

use crate::debug_panels::Place;
use monaka_channel::d3d;
use monaka_core::math::{self, Quat, Vec3};
use monaka_core::pad::{Gamepad, button};
use monaka_core::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_openxr::{Extent2Df, Layer, Posef, Quaternionf, Session, Space, Swapchain, Vector3f};
use monaka_overlay::menu::panel::{PIXELS, Panel};
use monaka_overlay::quad::Rectangle;
use monaka_overlay::menu::{Command, Menu, Press, Settings, Status, StickRepeat};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// How long a button is held before it acts.
const HOLD: Duration = Duration::from_millis(500);
/// How long a short press let go is shown to the game.
const TAP: Duration = Duration::from_millis(80);
/// Where the panel floats when it opens: this far ahead of the head, this far below the eyes, this
/// wide (metres).
const DISTANCE: f32 = 1.0;
const BELOW_EYES: f32 = 0.1;
const WIDTH: f32 = 0.7;
const MENU_BUTTONS: u16 = button::BACK | button::START;
/// A trigger past this (of 255) is pressed.
const TRIGGER: u8 = 160;

/// A buzz: the hand, its strength (0..1) and length (seconds).
pub type Buzz = (usize, f32, f32);

/// What one read of the pad asked for.
#[derive(Default)]
pub struct Asked {
    pub command: Option<Command>,
    pub buzzes: Vec<Buzz>,
}

/// A button that does something of Monaka's when held, and reaches the game as a tap otherwise.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LongPress {
    bit: u16,
    /// Held since then, and whether it acted (or was taken by the capture chord) in this hold.
    since: Option<(Instant, bool)>,
    /// A short press let go, shown to the game until then.
    tap_until: Option<Instant>,
}

impl LongPress {
    pub(crate) const fn new(bit: u16) -> Self {
        Self { bit, since: None, tap_until: None }
    }

    /// Takes the button out of `pad` while it may be a long press; true once when it has been
    /// held long enough. A short press is put back as a tap when let go.
    pub(crate) fn step(&mut self, pad: &mut Gamepad, now: Instant) -> bool {
        let mut fired = false;
        if pad.buttons & self.bit != 0 {
            let (since, acted) = *self.since.get_or_insert((now, false));
            if !acted && now.duration_since(since) >= HOLD {
                self.since = Some((since, true));
                fired = true;
            }
            pad.buttons &= !self.bit;
        } else if let Some((_, acted)) = self.since.take()
            && !acted
        {
            self.tap_until = Some(now + TAP);
        }
        if let Some(until) = self.tap_until {
            if now < until {
                pad.buttons |= self.bit;
            } else {
                self.tap_until = None;
            }
        }
        fired
    }

    /// The button is part of another chord this hold: it does nothing of its own until let go.
    pub(crate) fn yield_to_chord(&mut self, now: Instant) {
        self.since = Some((self.since.map_or(now, |(since, _)| since), true));
        self.tap_until = None;
    }
}

pub struct HeadsetMenu<'s> {
    menu: Menu,
    stick: StickRepeat,
    started: Instant,
    select: LongPress,
    /// Buttons and triggers held at the last read, for presses.
    held: u16,
    triggers: [bool; 2],
    panel: Panel,
    format: DXGI_FORMAT,
    swapchain: Option<Swapchain<'s>>,
    failed: bool,
    anchor: Posef,
}

impl<'s> HeadsetMenu<'s> {
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self {
            menu: Menu::default(),
            stick: StickRepeat::default(),
            started: Instant::now(),
            select: LongPress::new(button::BACK),
            held: 0,
            triggers: [false; 2],
            panel: Panel::new(format),
            format,
            swapchain: None,
            failed: false,
            anchor: pose([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, -DISTANCE]),
        }
    }

    pub fn open(&self) -> bool {
        self.menu.open
    }

    /// The pad as the controllers hold it, before anything else reads it: Select held opens or
    /// closes the menu; while the menu is open it reads the pad and leaves the game an idle one.
    /// `head`: where the head is, to place the panel.
    pub fn pad(&mut self, pad: &mut Gamepad, head: Option<(Quat, Vec3)>, now: Instant, settings: &mut Settings, status: &Status) -> Asked {
        let mut asked = Asked::default();
        if pad.buttons & MENU_BUTTONS == MENU_BUTTONS {
            // With Start: the capture chord's, as they come.
            self.select.yield_to_chord(now);
        } else {
            if self.select.step(pad, now) {
                self.menu.open = !self.menu.open;
                if self.menu.open {
                    self.menu.opened(status);
                }
                if self.menu.open
                    && let Some((orientation, position)) = head
                {
                    let (facing, ahead) = math::ahead_of(orientation, position, DISTANCE);
                    self.anchor = pose(facing, [ahead[0], ahead[1] - BELOW_EYES, ahead[2]]);
                }
                asked.buzzes.extend([LEFT_HAND, RIGHT_HAND].map(|hand| (hand, 0.5, 0.08)));
            }
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
        presses.extend(self.stick.step(f32::from(pad.thumb_lx) / 32767.0, f32::from(pad.thumb_ly) / 32767.0, seconds));
        presses.extend([(pressed & button::A != 0, Press::Switch), (pressed & button::B != 0, Press::Close), (pulled[0], Press::PreviousTab), (pulled[1], Press::NextTab)].into_iter().filter(|(on, _)| *on).map(|(_, p)| p));
        for press in presses {
            asked.command = asked.command.or(self.menu.press(press, settings, status));
            asked.buzzes.push((RIGHT_HAND, if press == Press::Switch { 0.4 } else { 0.15 }, 0.03));
        }
        *pad = Gamepad::default();
        asked
    }

    /// Draws the panel when it shows something new and hands it to the headset.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, settings: &Settings, status: &Status) {
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
        let image = match self.panel.draw(device, context, &self.menu, settings, status) {
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

    /// The panel's image and where it floats (tracking space), while it is shown.
    pub fn shown(&self) -> Option<(&ID3D11Texture2D, Rectangle)> {
        if !self.menu.open || self.failed || self.swapchain.is_none() {
            return None;
        }
        let q = self.anchor.orientation;
        let p = self.anchor.position;
        let size = [WIDTH, WIDTH * PIXELS[1] as f32 / PIXELS[0] as f32];
        self.panel.image().map(|image| (image, Rectangle { orientation: [q.x, q.y, q.z, q.w], position: [p.x, p.y, p.z], size }))
    }

    /// Where the panel floats (where it last opened), for the panels placed beside it.
    pub fn place(&self) -> Place {
        let q = self.anchor.orientation;
        let p = self.anchor.position;
        Place { orientation: [q.x, q.y, q.z, q.w], centre: [p.x, p.y, p.z], size: [WIDTH, WIDTH * PIXELS[1] as f32 / PIXELS[0] as f32] }
    }

    /// The menu's model, for the 2D window, which shows it with a pointer.
    pub fn model(&mut self) -> &mut Menu {
        &mut self.menu
    }

    /// The panel's layer, while open.
    pub fn layer<'a>(&'a self, space: &'a Space<'a>) -> Option<Layer<'a>> {
        let swapchain = self.swapchain.as_ref().filter(|_| self.menu.open && !self.failed)?;
        let height = WIDTH * PIXELS[1] as f32 / PIXELS[0] as f32;
        Some(Layer::Overlay { space, swapchain, pose: self.anchor, size: Extent2Df { width: WIDTH, height } })
    }

    fn fail(&mut self, why: String) {
        println!("Debug menu: {why}; the menu is off this run");
        self.failed = true;
    }
}

pub(crate) fn pose(orientation: Quat, position: Vec3) -> Posef {
    let [x, y, z, w] = orientation;
    let [px, py, pz] = position;
    Posef { orientation: Quaternionf { x, y, z, w }, position: Vector3f { x: px, y: py, z: pz } }
}

#[cfg(test)]
mod tests;
