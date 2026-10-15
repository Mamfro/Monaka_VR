//! A controller pointing at flat panels in the headset, the same for Monaka's home and the debug
//! menu: whichever controller's trigger was pulled last points (the pull also clicks), a cursor
//! goes where its ray meets the nearest panel, the trigger clicks there, the right stick scrolls.
//! What it does comes out as egui events, each for the panel it happened on.

use crate::debug_menu::Buzz;
use monaka_contract::gamepad::Gamepad;
use monaka_contract::protocol::{HandPose, LEFT_HAND, RIGHT_HAND};
use monaka_core::math::{Quat, Vec3};
use monaka_overlay::egui::{self, Pos2, Vec2};
use monaka_overlay::home::place;

/// A trigger counts as pulled past this, and let go below the other (0 to 255): apart, so a
/// trigger held at the edge does not click again and again.
const TRIGGER_PULLED: u8 = 160;
const TRIGGER_LET_GO: u8 = 96;
/// While the trigger is down the pointer stays where it was pulled until the hand moves it this
/// far (points, about 5 cm on a panel): pulling a trigger moves the hand, and egui takes a press
/// and release more than a few points apart for a drag, not a click.
const DRAG_START: f32 = 28.0;
/// The right stick's scroll at full tilt (points per frame), and the tilt below which it does not.
const SCROLL_SPEED: f32 = 14.0;
const SCROLL_DEADZONE: f32 = 0.2;

/// A panel the pointer can land on: where it floats (its face toward +Z), its size in the room
/// (metres) and its page's size (points).
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub orientation: Quat,
    pub centre: Vec3,
    pub size: [f32; 2],
    pub points: Vec2,
}

#[derive(Debug)]
pub struct Pointer {
    /// The hand that points, and which triggers are pulled.
    hand: usize,
    pulled: [bool; 2],
    /// Where it is: the panel and the point on it (points).
    at: Option<(usize, Pos2)>,
    /// The panel and point a pull started on, until it is let go, and whether the hand has moved
    /// far enough since to make it a drag.
    pressed_on: Option<(usize, Pos2)>,
    dragging: bool,
}

impl Default for Pointer {
    fn default() -> Self {
        Self { hand: RIGHT_HAND, pulled: [false; 2], at: None, pressed_on: None, dragging: false }
    }
}

impl Pointer {
    /// The hand that points.
    #[cfg(test)]
    pub fn hand(&self) -> usize {
        self.hand
    }

    /// Where it is now: the panel and the point on it.
    #[cfg(test)]
    pub fn at(&self) -> Option<(usize, Pos2)> {
        self.at
    }

    /// One read of the pad and the controllers' aim poses over `targets`: what happened, pushed to
    /// `events` (one list per target); scrolling with nothing pointed at goes to `fallback`. A
    /// buzz for a click.
    pub fn step(&mut self, pad: &Gamepad, hands: &[HandPose; 2], targets: &[Target], events: &mut [Vec<egui::Event>], fallback: usize) -> Vec<Buzz> {
        let mut buzzes = Vec::new();
        let triggers = [pad.left_trigger, pad.right_trigger];
        let mut clicked = false;
        let mut let_go = false;
        for hand in [LEFT_HAND, RIGHT_HAND] {
            let was = self.pulled[hand];
            self.pulled[hand] = if was { triggers[hand] > TRIGGER_LET_GO } else { triggers[hand] >= TRIGGER_PULLED };
            if self.pulled[hand] && !was && self.pressed_on.is_none() {
                // The hand that pulls points from now on, and its pull clicks where it points.
                self.hand = hand;
                clicked = true;
            }
            if was && !self.pulled[hand] && hand == self.hand {
                let_go = true;
            }
        }
        let mut hit = hit(&hands[self.hand], targets);
        // A pull held for a click keeps the pointer where it started, until the hand clearly moves
        // away (a drag) or off the panel.
        if let Some((target, start)) = self.pressed_on
            && !self.dragging
        {
            match hit {
                Some((now, at)) if now == target && at.distance(start) < DRAG_START => hit = Some((target, start)),
                _ => self.dragging = true,
            }
        }
        match (self.at, hit) {
            (Some((target, _)), Some((now, _))) if target != now => events[target].push(egui::Event::PointerGone),
            (Some((target, _)), None) => events[target].push(egui::Event::PointerGone),
            _ => {}
        }
        if let Some((target, at)) = hit
            && self.at != hit
        {
            events[target].push(egui::Event::PointerMoved(at));
        }
        self.at = hit;
        if clicked && let Some((target, at)) = hit {
            events[target].push(button(at, true));
            self.pressed_on = Some((target, at));
            self.dragging = false;
            buzzes.push((self.hand, 0.3, 0.03));
        }
        if let_go && let Some((target, start)) = self.pressed_on.take() {
            // A click lets go where it was pulled; a drag where the hand is, on the panel it began on.
            let at = if self.dragging { hit.filter(|(t, _)| *t == target).map_or(start, |(_, at)| at) } else { start };
            events[target].push(button(at, false));
            self.dragging = false;
        }
        let tilt = pad.thumb_ry as f32 / 32767.0;
        if tilt.abs() > SCROLL_DEADZONE {
            let target = hit.map_or(fallback, |(target, _)| target);
            if let Some(list) = events.get_mut(target) {
                list.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, tilt * SCROLL_SPEED),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                });
            }
        }
        buzzes
    }

    /// The pointer gone from the panels (they closed): a pull under way let go, the cursor gone.
    pub fn leave(&mut self, events: &mut [Vec<egui::Event>]) {
        if let Some((target, at)) = self.pressed_on.take()
            && let Some(list) = events.get_mut(target)
        {
            list.push(button(at, false));
        }
        self.dragging = false;
        if let Some((target, _)) = self.at.take()
            && let Some(list) = events.get_mut(target)
        {
            list.push(egui::Event::PointerGone);
        }
    }
}

/// The nearest of `targets` that `hand` points at, and where on it (points).
fn hit(hand: &HandPose, targets: &[Target]) -> Option<(usize, Pos2)> {
    if hand.valid == 0 {
        return None;
    }
    let direction = place::pointing(hand.orientation);
    targets
        .iter()
        .enumerate()
        .filter_map(|(index, target)| {
            let (along, [across, down]) = place::hit(target.orientation, target.centre, target.size, hand.position, direction)?;
            Some((along, index, Pos2::new(across * target.points.x, down * target.points.y)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, index, at)| (index, at))
}

fn button(at: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
}
