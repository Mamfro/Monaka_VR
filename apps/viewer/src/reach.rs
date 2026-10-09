//! Held d-pad menus by reaching over a shoulder (`--dpad-hold` with `--menu-reach`): the left
//! hand over the left shoulder opens the d-pad-left menu, the right hand over the right shoulder
//! the d-pad-right one (Dying Light: tools and weapons), and a flick of a stick picks from it.
//!
//! - **Opening.** A hand [`DEPTH`] behind the head's plane (`--menu-reach-depth`) and above the
//!   chest ([`in_zone`]) that comes to
//!   rest there ([`REST_SPEED`] for [`REST_FOR`]) opens its menu: the d-pad direction is held for the
//!   game and the controller buzzes. A melee backswing passes through the zone fast and never rests
//!   in it; a hand that swung within [`SWING_GRACE`] opens nothing. No button is involved.
//! - **Picking.** While a menu is open, the picking stick (the reaching hand's own, or the other
//!   hand's: [`Pick`]) neither moves nor turns: flicked from the centre past [`FLICK`], it presses
//!   the face button in that direction (up Y, right B, down A, left X) until it comes back past
//!   [`CENTRE`], as a player picking with that button would; each pick buzzes. A stick already
//!   pushed when the menu opened (running) picks nothing until it has been back at the centre.
//! - **Closing.** The hand leaving the zone ([`out_of_zone`], a little wider than the way in) lets
//!   go of the d-pad: the game closes the menu with the last pick made, or none. So does a hand the
//!   headset stops tracking for [`LOST_FOR`], and a menu open longer than [`OPEN_FOR`].
//!
//! Positions are the controllers' grip poses in the viewer's local space (metres, Y up, the head
//! facing -Z when level), against the head's position and its facing on the floor plane.

use monaka_core::math::{self, Quat, Vec3};
use monaka_core::pad::{Gamepad, button};
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

/// `--menu-reach`: on (picking with that stick) or off.
pub fn parse(word: &str) -> Result<Option<Pick>, String> {
    match word {
        "same" => Ok(Some(Pick::Same)),
        "other" => Ok(Some(Pick::Other)),
        "off" => Ok(None),
        other => Err(format!("--menu-reach: {other} is not same, other or off")),
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
    /// The d-pad direction each hand opens (0: none).
    menus: [u16; 2],
    /// How far behind the head a hand reaches to open (metres).
    depth: f32,
    hands: [Hand; 2],
}

impl Reach {
    /// `held`: the d-pad directions whose menus stay open only while held (`--dpad-hold`); the left
    /// hand opens d-pad left, the right hand d-pad right, where held. `depth`: how far behind the head
    /// a hand reaches to open its menu (metres; [`DEPTH`] unless the player sets their own).
    pub fn new(pick: Pick, held: u16, depth: f32) -> Self {
        Self { pick, menus: [held & button::DPAD_LEFT, held & button::DPAD_RIGHT], depth, hands: [Hand::default(); 2] }
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
        let Some((facing, eyes)) = head else { return buzz };
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
            pad.buttons |= held;
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
mod tests {
    use super::*;

    /// Level, at 1.7 m, facing -Z.
    const HEAD: (Quat, Vec3) = ([0.0, 0.0, 0.0, 1.0], [0.0, 1.7, 0.0]);
    const SHOULDER_L: Vec3 = [-0.2, 1.55, 0.25];
    const SHOULDER_R: Vec3 = [0.2, 1.55, 0.25];
    const FRONT_L: Vec3 = [-0.2, 1.3, -0.35];
    const FRONT_R: Vec3 = [0.2, 1.3, -0.35];

    fn ms(t: Instant, n: u64) -> Instant {
        t + Duration::from_millis(n)
    }

    /// One frame with the left hand at `left`, the right at `right` and the sticks as given.
    fn frame(reach: &mut Reach, at: Instant, left: Vec3, right: Vec3, lstick: (i16, i16), rstick: (i16, i16)) -> (Gamepad, Vec<Buzz>) {
        let mut pad = Gamepad { thumb_lx: lstick.0, thumb_ly: lstick.1, thumb_rx: rstick.0, thumb_ry: rstick.1, ..Default::default() };
        let buzz = reach.update(&mut pad, Some(HEAD), [Some(left), Some(right)], at);
        (pad, buzz)
    }

    const MAX: i16 = i16::MAX;

    #[test]
    fn the_zone_is_over_the_shoulder_and_turns_with_the_head() {
        let (q, eyes) = HEAD;
        assert!(in_zone(q, eyes, SHOULDER_L, DEPTH) && in_zone(q, eyes, SHOULDER_R, DEPTH));
        assert!(!in_zone(q, eyes, FRONT_L, DEPTH), "in front");
        assert!(!in_zone(q, eyes, [-0.2, 1.0, 0.25], DEPTH), "behind the back, too low");
        // Only just behind the head: not far enough by default, enough for a shallower reach.
        let shallow = [-0.2, 1.55, 0.08];
        assert!(!in_zone(q, eyes, shallow, DEPTH) && in_zone(q, eyes, shallow, 0.05));
        // Once open, a hand drifting a little forward keeps it open; well forward closes it.
        assert!(!out_of_zone(q, eyes, [-0.2, 1.55, 0.10], DEPTH) && out_of_zone(q, eyes, [-0.2, 1.55, 0.0], DEPTH));
        // Turned to face +X (yaw -90 degrees): behind is now -X.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let turned = [0.0, -s, 0.0, s];
        let right_back = [0.3, 1.55, 0.25];
        assert!(in_zone(q, eyes, right_back, DEPTH), "behind the unturned head");
        assert!(in_zone(turned, eyes, [-0.3, 1.55, 0.0], DEPTH) && !in_zone(turned, eyes, right_back, DEPTH), "in front of the turned one");
    }

    #[test]
    fn resting_over_the_shoulder_opens_and_a_flick_picks() {
        let t = Instant::now();
        let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
        // Arrives (still moving: nothing), comes to rest, and opens 150 ms later with a buzz on the left.
        frame(&mut reach, t, FRONT_L, FRONT_R, (0, 0), (0, 0));
        assert_eq!(frame(&mut reach, ms(t, 500), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, 0, "moving");
        assert_eq!(frame(&mut reach, ms(t, 600), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, 0, "resting");
        assert_eq!(frame(&mut reach, ms(t, 700), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, 0, "not yet");
        let (pad, buzz) = frame(&mut reach, ms(t, 760), SHOULDER_L, FRONT_R, (0, 0), (0, 0));
        assert_eq!((pad.buttons, buzz), (button::DPAD_LEFT, vec![(0, OPENED, BUZZ_FOR)]));
        // The left stick (same hand) flicked up: Y, the stick taken from the game; the right stick
        // still turns.
        let (pad, buzz) = frame(&mut reach, ms(t, 800), SHOULDER_L, FRONT_R, (0, MAX), (MAX, 0));
        assert_eq!((pad.buttons, pad.thumb_ly, pad.thumb_rx), (button::DPAD_LEFT | button::Y, 0, MAX));
        assert_eq!(buzz, vec![(0, PICKED, BUZZ_FOR)]);
        // Back to the centre: Y let go, the menu still open; then right: B.
        assert_eq!(frame(&mut reach, ms(t, 850), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, button::DPAD_LEFT);
        assert_eq!(frame(&mut reach, ms(t, 900), SHOULDER_L, FRONT_R, (MAX, 0), (0, 0)).0.buttons, button::DPAD_LEFT | button::B);
        // The hand comes forward: everything let go.
        assert_eq!(frame(&mut reach, ms(t, 950), FRONT_L, FRONT_R, (0, 0), (0, 0)).0.buttons, 0);
    }

    #[test]
    fn a_swing_through_the_zone_opens_nothing() {
        let t = Instant::now();
        let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
        // The right hand winds up fast over the shoulder and pauses there briefly before the swing.
        frame(&mut reach, t, FRONT_L, FRONT_R, (0, 0), (0, 0));
        frame(&mut reach, ms(t, 100), FRONT_L, SHOULDER_R, (0, 0), (0, 0));
        for n in [150, 200, 250, 300, 350, 400, 450] {
            assert_eq!(frame(&mut reach, ms(t, n), FRONT_L, SHOULDER_R, (0, 0), (0, 0)).0.buttons, 0, "{n} ms");
        }
        // Resting on after the grace: it opens (d-pad right, the weapons).
        frame(&mut reach, ms(t, 520), FRONT_L, SHOULDER_R, (0, 0), (0, 0));
        assert_eq!(frame(&mut reach, ms(t, 700), FRONT_L, SHOULDER_R, (0, 0), (0, 0)).0.buttons, button::DPAD_RIGHT);
    }

    #[test]
    fn a_stick_pushed_before_the_menu_opened_picks_nothing_until_centred() {
        let t = Instant::now();
        let mut reach = Reach::new(Pick::Other, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
        // Running (left stick forward) while the right hand reaches back: the other hand's stick,
        // the left, picks for the right hand's menu.
        frame(&mut reach, t, FRONT_L, SHOULDER_R, (0, MAX), (0, 0));
        frame(&mut reach, ms(t, 200), FRONT_L, SHOULDER_R, (0, MAX), (0, 0));
        let (pad, _) = frame(&mut reach, ms(t, 250), FRONT_L, SHOULDER_R, (0, MAX), (0, 0));
        assert_eq!((pad.buttons, pad.thumb_ly), (button::DPAD_RIGHT, 0), "open; no pick, no running");
        frame(&mut reach, ms(t, 300), FRONT_L, SHOULDER_R, (0, 0), (0, 0));
        assert_eq!(frame(&mut reach, ms(t, 350), FRONT_L, SHOULDER_R, (0, -MAX), (0, 0)).0.buttons, button::DPAD_RIGHT | button::A);
    }

    #[test]
    fn one_menu_at_a_time_and_none_for_a_direction_not_held() {
        let t = Instant::now();
        let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT, DEPTH);
        assert!(reach.active());
        // The right hand has no menu here (d-pad right is not held).
        frame(&mut reach, t, FRONT_L, SHOULDER_R, (0, 0), (0, 0));
        assert_eq!(frame(&mut reach, ms(t, 400), FRONT_L, SHOULDER_R, (0, 0), (0, 0)).0.buttons, 0);
        assert!(!Reach::new(Pick::Same, button::DPAD_UP, DEPTH).active());
        assert_eq!(parse("other"), Ok(Some(Pick::Other)));
        assert_eq!(parse("off"), Ok(None));
        assert!(parse("left").is_err());
    }

    #[test]
    fn a_lost_controller_closes_its_menu_after_a_while() {
        let t = Instant::now();
        let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
        frame(&mut reach, t, SHOULDER_L, FRONT_R, (0, 0), (0, 0));
        assert_eq!(frame(&mut reach, ms(t, 200), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, button::DPAD_LEFT);
        let lost = |reach: &mut Reach, n: u64| {
            let mut pad = Gamepad::default();
            reach.update(&mut pad, Some(HEAD), [None, Some(FRONT_R)], ms(t, n));
            pad.buttons
        };
        assert_eq!(lost(&mut reach, 700), button::DPAD_LEFT, "waits");
        assert_eq!(lost(&mut reach, 1300), 0, "then closes");
    }
}
