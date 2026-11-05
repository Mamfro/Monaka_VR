//! Monaka's home in the headset ([`monaka_overlay::home`]): the Games, Game Settings, Monaka and
//! Log panels, opened and closed by holding Start for half a second (a short press still reaches
//! the game). They stand on a ring round where the player last recentred, facing the way they
//! faced then ([`centre`]; [`crate::ring`], as the debug menu's), the Game Settings panel in front; LB and RB turn the ring, the panels going round to their
//! new places. A controller points at them, B closes. While open the home takes the pad (the game gets it idle). A setting changed
//! here is printed as a [`SETTING_LINE`] for the launcher to save, and acted on at once when the
//! viewer owns it.

use crate::debug_menu::Buzz;
use crate::menu_quad;
use crate::ring::RingView;
use monaka_contract::gamepad::{Gamepad, button};
use monaka_core::home::{HOME_CLOSED_LINE, PLAY_LINE, SELECT_LINE, SETTING_LINE};
use monaka_contract::protocol::HandPose;
use monaka_core::hold::HoldToggle;
use monaka_core::math::{Quat, Vec3};
use monaka_openxr::{Layer, Session, Space};
use monaka_overlay::eye::EyeView;
use monaka_overlay::frost::FrostPass;
use monaka_overlay::home::pages::Pages;
use monaka_overlay::home::place::Ring;
use monaka_overlay::home::{Home, PAGES};
use monaka_overlay::quad::Rectangle;
use std::path::Path;
use std::time::Instant;

/// Where the ring stands, the head at `head`: over the room's origin, where the player last
/// recentred, facing its forward; at the head's height, so the panels are at eye level sitting or
/// standing.
fn centre(head: Option<(Quat, Vec3)>) -> (Quat, Vec3) {
    (monaka_core::math::IDENTITY, [0.0, head.map_or(0.0, |(_, position)| position[1]), 0.0])
}
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// What one read of the pad asked for.
#[derive(Default)]
pub struct Asked {
    /// The settings the player changed since the last read: each id and value.
    pub changed: Vec<(&'static str, String)>,
    pub buzzes: Vec<Buzz>,
    /// The player asked to stop VR.
    pub stop: bool,
    /// The player asked for a capture of what the headset shows.
    pub capture: bool,
    /// The player asked for the debug menu (the home has closed for it).
    pub debug_menu: bool,
    /// The player asked for the settings waiting to apply now (the game's part of Monaka starts
    /// again with them).
    pub apply: bool,
    /// The player asked to leave VR (the Monaka window's own headset view closes).
    pub leave: bool,
}

/// The gain for volume setting `percent` (0 to 100): 100 the recording as it is, each percent
/// 0.4 dB quieter, 0 silent.
pub fn volume_gain(percent: f32) -> f32 {
    if percent <= 0.0 { 0.0 } else { 10f32.powf(-0.4 * (100.0 - percent.min(100.0)) / 20.0) }
}

pub struct HomeView<'s> {
    home: Home,
    pages: Pages,
    /// The panels round the player, and the pointer on them.
    ring: RingView<'s>,
    open: bool,
    start: HoldToggle,
    /// Buttons held at the last read, for presses.
    held: u16,
    /// Opens as soon as the head is known (the window's view at its start, and when a game ends).
    open_soon: bool,
    /// The ring goes round to where the head faces at the next read (the player recentred).
    recenter: bool,
    /// The game blurred behind the panels while they are frosted, made at the first need; `Err`
    /// once it failed (the panels are solid for the rest of the run).
    frost: Option<Result<FrostPass, ()>>,
}

impl<'s> HomeView<'s> {
    /// The home of the run in `run_folder` (none: a viewer started by hand), drawn in `format`;
    /// with the debug menu's tools when `debug` (the debug menu is on this run).
    pub fn new(format: DXGI_FORMAT, run_folder: Option<&Path>, debug: bool) -> Self {
        let mut home = Home::load(run_folder);
        home.debug = debug;
        let ring = RingView::new("Monaka's home", format, PAGES.len(), home.front);
        Self {
            home,
            pages: Pages::new(format),
            ring,
            open: false,
            start: HoldToggle::new(button::START),
            held: 0,
            open_soon: false,
            recenter: false,
            frost: None,
        }
    }

    /// The panels frosted (the game blurred behind them) or solid, from now on.
    pub fn set_frosted(&mut self, frosted: bool) {
        self.home.set_frosted(frosted && !matches!(self.frost, Some(Err(()))));
    }

    /// Blurs eye `image` (made in `format`, a copy of `source`, seen as `eye`) behind the panels,
    /// while they are open and frosted.
    pub fn frost_eye(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, format: DXGI_FORMAT, source: &ID3D11Texture2D, eye: &EyeView) {
        if !self.open || !self.home.frosted() {
            return;
        }
        let made = self.frost.get_or_insert_with(|| FrostPass::new(device).map_err(|e| println!("Frosted panels: {e}; solid this run")));
        let Ok(frost) = made else {
            self.home.set_frosted(false);
            return;
        };
        let panels: Vec<Rectangle> = (0..PAGES.len()).filter_map(|page| self.ring.rectangle(page)).collect();
        if let Err(e) = frost.draw(device, context, image, format, source, eye, &panels) {
            println!("Frosted panels: {e}; solid this run");
            self.frost = Some(Err(()));
            self.home.set_frosted(false);
        }
    }

    /// What surrounds the home while no game shows (`background`, Monaka's setting): none for the
    /// plain dark.
    pub fn background(&self) -> Option<crate::scene::Background> {
        use monaka_contract::manifest::{BACKGROUND, MONAKA};
        match self.home.values.text(&MONAKA, BACKGROUND.id).as_str() {
            "home" => Some(crate::scene::Background::Home),
            "drift" => Some(crate::scene::Background::Drift),
            _ => None,
        }
    }

    /// The background shows round a game's flat screen too (`background_flat`).
    pub fn background_flat(&self) -> bool {
        use monaka_contract::manifest::{BACKGROUND_FLAT, MONAKA};
        self.home.values.switch(&MONAKA, BACKGROUND_FLAT.id)
    }

    /// The hour the scene stays at (`scene_time`), or none while it follows the clock.
    pub fn scene_hour(&self) -> Option<u8> {
        use monaka_contract::manifest::{MONAKA, SCENE_TIME};
        self.home.values.text(&MONAKA, SCENE_TIME.id).parse::<u8>().ok().filter(|h| *h < 24)
    }

    /// How loud the scene's sound is (`scene_volume`), as a gain on the recording (0 to 1): the
    /// setting's steps are even to the ear, 40 dB from 1 to 100.
    pub fn scene_volume(&self) -> f32 {
        use monaka_contract::manifest::{MONAKA, SCENE_VOLUME};
        volume_gain(self.home.values.number(&MONAKA, SCENE_VOLUME.id) as f32)
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The Monaka window's own headset view: games are picked and played here, and it opens at
    /// once (nothing else shows yet).
    pub fn for_window(&mut self) {
        self.home.window = true;
        self.home.show_run(None);
        self.open_soon = true;
    }

    /// Opens as soon as the head is known (the game let go: the home is all there is to see).
    pub fn open_soon(&mut self) {
        self.open_soon = !self.open;
    }

    /// The player recentred: the ring goes round to where the head now faces.
    pub fn recenter(&mut self) {
        self.recenter = true;
    }

    /// Where the ring stands (none until it first opened): its middle panel is straight ahead of
    /// it.
    pub fn centre(&self) -> Option<Ring> {
        self.ring.centre()
    }

    /// The room's origin moved to the ring's centre ([`RingView::at_origin`]).
    pub fn at_origin(&mut self) {
        self.ring.at_origin();
    }

    /// Takes what the window changed in the files in `folder`.
    pub fn sync(&mut self, folder: &Path) {
        self.home.sync(folder);
    }

    /// The game linked in: the logs of its run (`run_folder`), and the debug menu's tools on its
    /// Log panel when `debug`; or none.
    pub fn show_run(&mut self, run_folder: Option<&Path>, debug: bool) {
        self.home.show_run(run_folder);
        self.home.debug = debug;
    }

    /// The game is back with the settings applied (`problem`: or it is not, and why).
    pub fn applied(&mut self, problem: Option<String>) {
        self.home.applied(problem);
    }

    /// The pad as the controllers hold it, before the game reads it, and the controllers' aim
    /// poses: Start held opens or closes the home; while open it reads the pad and the pointer and
    /// leaves the game an idle pad. `head`: where the head is, to place the ring. `busy`: another
    /// menu has the pad (the debug menu).
    pub fn pad(&mut self, pad: &mut Gamepad, head: Option<(Quat, Vec3)>, hands: &[HandPose; 2], now: Instant, busy: bool) -> Asked {
        let mut asked = Asked { changed: self.home.take_changes(), stop: self.home.take_stop(), capture: self.home.take_capture(), apply: self.home.take_apply(), ..Asked::default() };
        for (id, value) in &asked.changed {
            println!("{SETTING_LINE} {id}={value}");
        }
        if self.home.take_debug_menu() {
            asked.debug_menu = true;
            self.set_open(false, head);
        }
        // What only the window can do: show another game, play one.
        if let Some(id) = self.home.take_select() {
            println!("{SELECT_LINE} {id}");
        }
        if self.home.take_play() {
            println!("{PLAY_LINE}");
        }
        asked.leave = self.home.take_leave();
        if std::mem::take(&mut self.recenter) {
            self.ring.place_at(centre(head));
        }
        if self.open_soon && !self.open && head.is_some() {
            self.open_soon = false;
            self.set_open(true, head);
        }
        if busy {
            return asked;
        }
        if self.start.toggled(pad, now) {
            self.set_open(!self.open, head);
            asked.buzzes.extend(menu_quad::TOGGLED);
        }
        if !self.open {
            self.held = pad.buttons;
            return asked;
        }
        let pressed = pad.buttons & !self.held;
        self.held = pad.buttons;
        if pressed & button::B != 0 {
            self.set_open(false, head);
            asked.buzzes.extend(menu_quad::TOGGLED);
            *pad = Gamepad::default();
            return asked;
        }
        for (shoulder, steps) in [(button::LEFT_SHOULDER, -1), (button::RIGHT_SHOULDER, 1)] {
            if pressed & shoulder != 0 {
                self.home.turn(steps);
                self.ring.turn(steps, now);
                asked.buzzes.push(menu_quad::pressed(false));
            }
        }
        self.ring.step(now);
        asked.buzzes.extend(self.ring.point(pad, hands, |_| true));
        *pad = Gamepad::default();
        asked
    }

    fn set_open(&mut self, open: bool, head: Option<(Quat, Vec3)>) {
        // Closed: the player goes back to the game, which the launcher brings to the front.
        if self.open && !open {
            println!("{HOME_CLOSED_LINE}");
        }
        self.open = open;
        if open {
            // Where the player recentred, facing the way they did: not wherever the head turned
            // since, so the panels are always found in the same place.
            self.ring.place_at(centre(head));
            self.ring.show_in_front(self.home.front);
            self.home.all_stale();
        } else {
            self.ring.leave();
        }
    }

    /// Draws the panels that show something new and hands them to the headset.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, now: Instant) {
        if !self.open {
            return;
        }
        self.home.refresh(now);
        for page in 0..PAGES.len() {
            let events = self.ring.take_events(page);
            let stale = self.home.take_stale(page);
            let (pages, home) = (&mut self.pages, &mut self.home);
            self.ring.show(page, session, context, |fresh| {
                if !(fresh || stale || !events.is_empty()) {
                    return Ok(None);
                }
                pages.draw(page, device, context, home, events).map(|image| image.cloned())
            });
        }
    }

    /// The panels' layers, while open.
    pub fn layers<'a>(&'a self, space: &'a Space<'a>) -> Vec<Layer<'a>> {
        if !self.open {
            return Vec::new();
        }
        (0..PAGES.len()).filter_map(|page| self.ring.layer(page, space)).collect()
    }
}

#[cfg(test)]
mod tests;
