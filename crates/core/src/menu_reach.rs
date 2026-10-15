//! Held d-pad menus by reaching over a shoulder (`dpad_menus` with `menu_reach`): the left
//! hand over the left shoulder opens the d-pad-left menu, the right hand over the right shoulder
//! the d-pad-right one (Dying Light: tools and weapons), and a flick of a stick picks from it.
//!
//! - **Opening.** A hand [`DEPTH`] behind the head's plane (`menu_reach_depth`) and above the
//!   chest ([`in_zone`]) that comes to
//!   rest there ([`REST_SPEED`] for [`REST_FOR`]) opens its menu: the d-pad direction is held for the
//!   game and the controller buzzes. A melee backswing passes through the zone fast and never rests
//!   in it; a hand that swung within [`SWING_GRACE`] opens nothing. No button is involved.
//! - **Picking.** While a menu is open, the picking stick (the reaching hand's own, or the other
//!   hand's: [`Pick`]) neither moves nor turns: flicked from the centre past [`FLICK`], it presses
//!   the face button in that direction (up Y, right B, down A, left X) until it comes back past
//!   [`CENTRE`], as a player picking with that button would; each pick buzzes. A game that picks
//!   with its right stick instead (`menu_pick=stick`, [`PickWith`]) is handed the flick as its
//!   right stick, and nothing else there while the menu is open; its left stick reads centred
//!   then (its wheels follow either stick, and a player's thumb on the d-pad is off the stick). A
//!   stick already pushed when the menu opened (running) picks nothing until it has been back at
//!   the centre.
//! - **Closing.** The hand leaving the zone ([`out_of_zone`], a little wider than the way in) lets
//!   go of the d-pad: the game closes the menu with the last pick made, or none. So does a hand the
//!   headset stops tracking for [`LOST_FOR`], and a menu open longer than [`OPEN_FOR`].
//!
//! Positions are the controllers' grip poses in the viewer's local space (metres, Y up, the head
//! facing -Z when level), against the head's position and the body's facing on the floor plane:
//! the head's, followed over [`BODY_FOLLOW`] ([`Reach::body_facing`]). Turned to look over a
//! shoulder, the head would put the other hand, hanging at the body's side, behind it.

use crate::math::{self, Quat, Vec3};
use monaka_contract::{gamepad::Gamepad, gamepad::button};
use std::time::{Duration, Instant};

/// How far behind the head's plane a hand reaches to open its menu (metres), unless the player
/// sets their own.
pub const DEPTH: f32 = 0.15;
/// An open menu closes once its hand comes this much nearer than that (metres): picking does not
/// close it by drifting forward a little.
const CLOSE_MARGIN: f32 = 0.10;
/// No lower below the eyes than this (metres): over the shoulder, not behind the back, where the
/// headset's cameras lose the controller.
const BELOW_IN: f32 = 0.35;
const BELOW_OUT: f32 = 0.45;
/// The body turns to where the head faces over about this long: a glance over a shoulder hardly
/// turns it, a turn of the whole body is followed within it.
const BODY_FOLLOW: Duration = Duration::from_millis(1000);
/// A hand slower than this (metres per second) rests.
const REST_SPEED: f32 = 0.5;
/// Resting in the zone this long opens the menu.
const REST_FOR: Duration = Duration::from_millis(150);
/// A hand faster than this swings.
const SWING_SPEED: f32 = 1.5;
/// A hand that swung this recently opens nothing.
const SWING_GRACE: Duration = Duration::from_millis(400);
/// A stick flicked past this (of its range) picks ...
const FLICK: f32 = 0.6;
/// ... and is back at the centre within this.
const CENTRE: f32 = 0.3;
/// A menu closes after this long, and when its hand goes untracked this long.
const OPEN_FOR: Duration = Duration::from_secs(8);
const LOST_FOR: Duration = Duration::from_secs(1);

/// Which stick picks from an open menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pick {
    /// The reaching hand's own: the other hand stays free to aim and turn.
    #[default]
    Same,
    /// The other hand's.
    Other,
}

/// How a pick reaches the game.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PickWith {
    /// The face button in the flick's direction (Dying Light 1).
    #[default]
    Buttons,
    /// The flick itself, on the game's right stick (The Beast's wheels).
    Stick,
}

/// `menu_pick` (for the viewer): how a pick reaches the game.
pub fn parse_with(word: &str) -> Result<PickWith, String> {
    match word {
        "buttons" => Ok(PickWith::Buttons),
        "stick" => Ok(PickWith::Stick),
        other => Err(format!("menu_pick: {other} is not buttons or stick")),
    }
}

/// The `menu_reach` option: on (picking with that stick) or off.
pub fn parse(word: &str) -> Result<Option<Pick>, String> {
    match word {
        "same" => Ok(Some(Pick::Same)),
        "other" => Ok(Some(Pick::Other)),
        "off" => Ok(None),
        other => Err(format!("menu_reach: {other} is not same, other or off")),
    }
}

/// A buzz for a controller: hand (0 left, 1 right), amplitude 0 to 1, seconds.
pub type Buzz = (usize, f32, f32);
const OPENED: f32 = 0.35;
const PICKED: f32 = 0.6;
const BUZZ_FOR: f32 = 0.04;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum State {
    #[default]
    Away,
    /// In the zone, resting since then.
    Resting(Instant),
    Open {
        since: Instant,
        /// The picking stick has been at the centre since the menu opened.
        centred: bool,
        /// The face button the stick holds down (0: none).
        held: u16,
    },
}

#[derive(Clone, Copy, Debug, Default)]
struct Hand {
    state: State,
    last: Option<(Vec3, Instant)>,
    swung: Option<Instant>,
    seen: Option<Instant>,
}

/// The menus by reach, both hands.
pub struct Reach {
    pick: Pick,
    with: PickWith,
    /// The d-pad direction each hand opens (0: none).
    menus: [u16; 2],
    /// How far behind the head a hand reaches to open (metres).
    depth: f32,
    hands: [Hand; 2],
    /// The body's facing (yaw about Y, radians; 0 faces -Z) and when it was last followed.
    body: Option<(f32, Instant)>,
}

impl Reach {
    /// `held`: the d-pad directions whose menus stay open only while held (`dpad_menus`); the left
    /// hand opens d-pad left, the right hand d-pad right, where held. `depth`: how far behind the head
    /// a hand reaches to open its menu (metres; [`DEPTH`] unless the player sets their own).
    pub fn new(pick: Pick, held: u16, depth: f32) -> Self {
        Self { pick, with: PickWith::Buttons, menus: [held & button::DPAD_LEFT, held & button::DPAD_RIGHT], depth, hands: [Hand::default(); 2], body: None }
    }

    /// Picks reach the game as `with` says.
    pub fn picking_with(self, with: PickWith) -> Self {
        Self { with, ..self }
    }

    /// The body's facing this frame: the head's yaw, followed over [`BODY_FOLLOW`].
    fn body_facing(&mut self, head: Quat, now: Instant) -> Quat {
        let f = math::forward_axis(head);
        let head_yaw = if f[0].abs() + f[2].abs() > 1e-4 { (-f[0]).atan2(-f[2]) } else { self.body.map_or(0.0, |(yaw, _)| yaw) };
        let yaw = match self.body {
            Some((yaw, then)) => {
                let share = (now.saturating_duration_since(then).as_secs_f32() / BODY_FOLLOW.as_secs_f32()).min(1.0);
                let turn = (head_yaw - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                yaw + turn * share
            }
            None => head_yaw,
        };
        self.body = Some((yaw, now));
        [0.0, (yaw * 0.5).sin(), 0.0, (yaw * 0.5).cos()]
    }

    /// Whether any hand opens a menu.
    pub fn active(&self) -> bool {
        self.menus != [0, 0]
    }

    /// One frame: `head` (orientation, position) and the hands' positions (`None`: untracked), at
    /// `now`. Holds the open menu's d-pad and the picked face button on `pad`, takes the picking
    /// stick from it, and returns the buzzes to play.
    pub fn update(&mut self, pad: &mut Gamepad, head: Option<(Quat, Vec3)>, hands: [Option<Vec3>; 2], now: Instant) -> Vec<Buzz> {
        let mut buzz = Vec::new();
        let Some((head_facing, eyes)) = head else { return buzz };
        let facing = self.body_facing(head_facing, now);
        let open_hand = (0..2).find(|&h| matches!(self.hands[h].state, State::Open { .. }));
        let depth = self.depth;
        for (h, at) in hands.into_iter().enumerate() {
            if self.menus[h] == 0 {
                continue;
            }
            let hand = &mut self.hands[h];
            let Some(at) = at else {
                // Untracked: an open menu waits a little for the controller to come back.
                if hand.seen.is_some_and(|seen| now.duration_since(seen) >= LOST_FOR) {
                    hand.state = State::Away;
                    hand.last = None;
                }
                continue;
            };
            hand.seen = Some(now);
            let speed = match hand.last {
                Some((was, then)) if now > then => distance(at, was) / now.duration_since(then).as_secs_f32(),
                _ => 0.0,
            };
            hand.last = Some((at, now));
            if speed > SWING_SPEED {
                hand.swung = Some(now);
            }
            let swinging = hand.swung.is_some_and(|swung| now.duration_since(swung) < SWING_GRACE);
            hand.state = match hand.state {
                State::Away | State::Resting(_) if open_hand.is_some_and(|other| other != h) => State::Away,
                State::Away if in_zone(facing, eyes, at, depth) && speed < REST_SPEED && !swinging => State::Resting(now),
                State::Away => State::Away,
                State::Resting(_) if !in_zone(facing, eyes, at, depth) || speed >= REST_SPEED || swinging => State::Away,
                State::Resting(since) if now.duration_since(since) >= REST_FOR => {
                    buzz.push((h, OPENED, BUZZ_FOR));
                    State::Open { since: now, centred: false, held: 0 }
                }
                resting @ State::Resting(_) => resting,
                State::Open { since, .. } if out_of_zone(facing, eyes, at, depth) || now.duration_since(since) >= OPEN_FOR => State::Away,
                open @ State::Open { .. } => open,
            };
        }
        // The open menu: its d-pad held, picked with a flick.
        for h in 0..2 {
            let State::Open { since, centred, held } = self.hands[h].state else { continue };
            pad.buttons |= self.menus[h];
            let stick = if (h == 0) == (self.pick == Pick::Same) { Stick::Left } else { Stick::Right };
            let (x, y) = stick.take(pad);
            let push = x.abs().max(y.abs());
            let centred = centred || push < CENTRE;
            let held = if push < CENTRE {
                0
            } else if centred && push >= FLICK {
                let face = match (x.abs() > y.abs(), x > 0.0, y > 0.0) {
                    (true, true, _) => button::B,
                    (true, false, _) => button::X,
                    (false, _, true) => button::Y,
                    (false, _, false) => button::A,
                };
                if face != held {
                    buzz.push((if stick == Stick::Left { 0 } else { 1 }, PICKED, BUZZ_FOR));
                }
                face
            } else {
                held
            };
            match self.with {
                PickWith::Buttons => pad.buttons |= held,
                PickWith::Stick => {
                    // The game's right stick carries the flick (from the centre) and nothing else;
                    // its left stick, which its wheels also follow, nothing.
                    Stick::Left.take(pad);
                    Stick::Right.take(pad);
                    if held != 0 {
                        let axis = |v: f32| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        (pad.thumb_rx, pad.thumb_ry) = (axis(x), axis(y));
                    }
                }
            }
            self.hands[h].state = State::Open { since, centred, held };
        }
        buzz
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stick {
    Left,
    Right,
}

impl Stick {
    /// The stick's position (-1 to 1 each way), and the stick centred for the game.
    fn take(self, pad: &mut Gamepad) -> (f32, f32) {
        let (x, y) = match self {
            Stick::Left => (&mut pad.thumb_lx, &mut pad.thumb_ly),
            Stick::Right => (&mut pad.thumb_rx, &mut pad.thumb_ry),
        };
        let read = (*x as f32 / i16::MAX as f32, *y as f32 / i16::MAX as f32);
        (*x, *y) = (0, 0);
        read
    }
}

fn distance(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// How far `at` sits behind the head (metres, along its facing on the floor) and below the eyes.
fn behind_and_below(facing: Quat, eyes: Vec3, at: Vec3) -> (f32, f32) {
    let f = math::forward_axis(facing);
    let length = (f[0] * f[0] + f[2] * f[2]).sqrt().max(1e-4);
    let (fx, fz) = (f[0] / length, f[2] / length);
    let (dx, dz) = (at[0] - eyes[0], at[2] - eyes[2]);
    (-(dx * fx + dz * fz), eyes[1] - at[1])
}

/// Over the shoulder: `depth` behind the head and no lower than the chest.
fn in_zone(facing: Quat, eyes: Vec3, at: Vec3, depth: f32) -> bool {
    let (behind, below) = behind_and_below(facing, eyes, at);
    behind > depth && below < BELOW_IN
}

/// Out of the zone again, with a margin so a menu does not flicker at its edge.
fn out_of_zone(facing: Quat, eyes: Vec3, at: Vec3, depth: f32) -> bool {
    let (behind, below) = behind_and_below(facing, eyes, at);
    behind < depth - CLOSE_MARGIN || below > BELOW_OUT
}

#[cfg(test)]
mod tests;
