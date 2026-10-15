//! Monaka's home in the headset ([`monaka_overlay::home`]): the Games, Settings and Log panels,
//! opened and closed by holding Start for half a second (a short press still reaches the game).
//! They stand round where the head was when it opened, the Settings panel in front; LB and RB turn
//! the ring, the panels going round to their new places. A controller points at them
//! ([`crate::pointer`]), B closes. While open the home takes the pad (the game gets it idle). A setting changed
//! here is printed as a [`SETTING_LINE`] for the launcher to save, and acted on at once when the
//! viewer owns it.

use crate::debug_menu::Buzz;
use crate::menu_quad::{self, MenuQuad};
use crate::pointer::{Pointer, Target};
use monaka_contract::gamepad::{Gamepad, button};
use monaka_contract::manifest::SETTING_LINE;
use monaka_core::home::{PLAY_LINE, SELECT_LINE};
use monaka_contract::protocol::HandPose;
use monaka_core::hold::HoldToggle;
use monaka_core::math::{Quat, Vec3};
use monaka_openxr::{Extent2Df, Layer, Posef, Session, Space};
use monaka_overlay::egui;
use monaka_overlay::home::pages::Pages;
use monaka_overlay::home::place::{self, Ring, TURN_SECONDS, Turning, slot_angle};
use monaka_overlay::home::{Home, PAGES};
use std::path::Path;
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
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

/// The ring turning: how, and since when.
struct Turn {
    turning: Turning,
    started: Instant,
}

pub struct HomeView<'s> {
    home: Home,
    pages: Pages,
    quads: [MenuQuad<'s>; 3],
    open: bool,
    start: HoldToggle,
    /// Buttons held at the last read, for presses.
    held: u16,
    /// Where the ring was opened, each panel's angle round it now, and a turn under way.
    ring: Option<Ring>,
    angles: [f32; 3],
    turn: Option<Turn>,
    pointer: Pointer,
    /// What the pointer did on each panel since it was last drawn.
    events: [Vec<egui::Event>; 3],
    /// Opens as soon as the head is known (the window's view at its start, and when a game ends).
    open_soon: bool,
    /// The ring goes round to where the head faces at the next read (the player recentred).
    recenter: bool,
}

impl<'s> HomeView<'s> {
    /// The home of the run in `run_folder` (none: a viewer started by hand), drawn in `format`;
    /// with the debug menu's tools when `debug` (the debug menu is on this run).
    pub fn new(format: DXGI_FORMAT, run_folder: Option<&Path>, debug: bool) -> Self {
        let mut home = Home::load(run_folder);
        home.debug = debug;
        let angles = std::array::from_fn(|page| slot_angle(page, home.front, PAGES.len()));
        Self {
            home,
            pages: Pages::new(format),
            quads: std::array::from_fn(|page| MenuQuad::new(PAGES[page].title(), format, place::PIXELS)),
            open: false,
            start: HoldToggle::new(button::START),
            held: 0,
            ring: None,
            angles,
            turn: None,
            pointer: Pointer::default(),
            events: Default::default(),
            open_soon: false,
            recenter: false,
        }
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
        if let (true, Some((orientation, position))) = (self.recenter, head) {
            self.recenter = false;
            self.ring = Some(Ring::at(orientation, position));
            self.placed();
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
                self.turn = Some(Turn { turning: Turning::new(self.angles, self.home.front, steps), started: now });
                asked.buzzes.push(menu_quad::pressed(false));
            }
        }
        if self.home.take_recenter() {
            self.ring = head.map(|(orientation, position)| Ring::at(orientation, position)).or(self.ring);
        }
        self.step_turn(now);
        let points = self.pages.points();
        let size = [place::WIDTH, place::height()];
        let targets = self.quads.each_ref().map(|quad| Target { orientation: quad.anchor.orientation.xyzw(), centre: quad.anchor.position.xyz(), size, points });
        let targets = if self.ring.is_some() { &targets[..] } else { &[] };
        asked.buzzes.extend(self.pointer.step(pad, hands, targets, &mut self.events, self.home.front));
        *pad = Gamepad::default();
        asked
    }

    fn set_open(&mut self, open: bool, head: Option<(Quat, Vec3)>) {
        self.open = open;
        if open {
            if let Some((orientation, position)) = head {
                self.ring = Some(Ring::at(orientation, position));
            }
            self.turn = None;
            self.angles = std::array::from_fn(|page| slot_angle(page, self.home.front, PAGES.len()));
            self.placed();
            self.home.all_stale();
        } else {
            self.pointer.leave(&mut self.events);
        }
    }

    /// The panels' angles along a turn under way, settled in their places once it is done.
    fn step_turn(&mut self, now: Instant) {
        if let Some(turn) = &self.turn {
            let progress = now.duration_since(turn.started).as_secs_f32() / TURN_SECONDS;
            if progress >= 1.0 {
                self.angles = std::array::from_fn(|page| slot_angle(page, self.home.front, PAGES.len()));
                self.turn = None;
            } else {
                self.angles = turn.turning.at(progress);
            }
        }
        self.placed();
    }

    /// The quads where their angles round the ring put them.
    fn placed(&mut self) {
        let Some(ring) = self.ring else { return };
        for (quad, angle) in self.quads.iter_mut().zip(self.angles) {
            let (orientation, centre) = ring.at_angle(angle);
            quad.anchor = Posef::new(orientation, centre);
        }
    }

    /// Draws the panels that show something new and hands them to the headset.
    pub fn show(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, now: Instant) {
        if !self.open {
            return;
        }
        self.home.refresh(now);
        for page in 0..PAGES.len() {
            let events = std::mem::take(&mut self.events[page]);
            let stale = self.home.take_stale(page);
            let (pages, home) = (&mut self.pages, &mut self.home);
            self.quads[page].show(session, context, |fresh| {
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
        let size = Extent2Df { width: place::WIDTH, height: place::height() };
        self.quads.iter().filter_map(|quad| Some(Layer::Overlay { space, swapchain: quad.swapchain()?, pose: quad.anchor, size })).collect()
    }
}

#[cfg(test)]
mod tests;
