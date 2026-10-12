//! A gamepad made from VR controllers: the viewer turns its OpenXR actions into one Xbox-layout
//! pad ([`Gamepad`], XInput's `XINPUT_GAMEPAD`), a producer hands it to the game's own gamepad
//! input, merged with a real pad when one is plugged in ([`merge`]). A producer that turns the
//! player itself with the right stick does it through [`StickTurn`].

use crate::manifest::Manifest;
use crate::options::Options;

/// XInput's `XINPUT_GAMEPAD`: buttons, triggers 0..255, sticks -32768..32767 (up positive).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gamepad {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

/// XInput button bits.
pub mod button {
    pub const DPAD_UP: u16 = 0x0001;
    pub const DPAD_DOWN: u16 = 0x0002;
    pub const DPAD_LEFT: u16 = 0x0004;
    pub const DPAD_RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LEFT_THUMB: u16 = 0x0040;
    pub const RIGHT_THUMB: u16 = 0x0080;
    pub const LEFT_SHOULDER: u16 = 0x0100;
    pub const RIGHT_SHOULDER: u16 = 0x0200;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// Below this a VR stick reads as centred, so a resting controller's drift never moves the game.
pub const STICK_DEADZONE: f32 = 0.12;

/// Stick deflection a producer's own smooth turning ignores around the centre
/// ([`StickTurn::smooth`]).
pub const TURN_DEADZONE: f32 = 0.15;

/// How a producer's own turning takes a stick axis (-1..=1, right positive). The turn it gives is
/// in degrees, right positive as the stick is; each producer applies it to its own heading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StickTurn {
    /// Turning while the stick is pushed: past `dead_zone` the push is rescaled to 0..=1 and raised
    /// to `ease` (1 straight; more eases in for fine control), `speed` degrees a second at full tilt.
    Smooth { speed: f32, dead_zone: f32, ease: f32 },
    /// A turn of `degrees` once the stick is pushed past `at`; the next once it has come back
    /// within `release` ([`TurnLatch`]).
    Snap { degrees: f32, at: f32, release: f32 },
}

/// A snap turn's latch: set by a snap, cleared once the stick comes back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnLatch {
    snapped: bool,
}

impl TurnLatch {
    pub const fn new() -> Self {
        Self { snapped: false }
    }
}

impl StickTurn {
    /// A smooth turn at `speed` degrees a second, past [`TURN_DEADZONE`] and eased in.
    pub const fn smooth(speed: f32) -> Self {
        Self::Smooth { speed, dead_zone: TURN_DEADZONE, ease: 1.5 }
    }

    /// A 30 degree snap past seven tenths of the stick's travel, again once it is back within three.
    pub const SNAP: Self = Self::Snap { degrees: 30.0, at: 0.7, release: 0.3 };

    /// The turn the `turn` option asks for ([`crate::manifest::TURN`]; the game's default when
    /// unset): `smooth` or `snap`.
    pub fn from_options(options: &Options, manifest: &Manifest, smooth: StickTurn, snap: StickTurn) -> StickTurn {
        let snapping = options.choice("turn", &[("smooth", false), ("snap", true)], manifest.choice_default("turn") == Some("snap"));
        if snapping { snap } else { smooth }
    }

    /// A smooth turn's share of its full speed for the stick at `x` (-1..=1, signed as the
    /// stick): 0 within the dead zone. A snap's is 0.
    pub fn share(&self, x: f32) -> f32 {
        match *self {
            Self::Smooth { dead_zone, ease, .. } => {
                let past = ((x.abs() - dead_zone) / (1.0 - dead_zone)).clamp(0.0, 1.0);
                x.signum() * past.powf(ease)
            }
            Self::Snap { .. } => 0.0,
        }
    }

    /// Whether the stick at `x` is pushed far enough to turn.
    pub fn pushed(&self, x: f32) -> bool {
        match *self {
            Self::Smooth { .. } => self.share(x) != 0.0,
            Self::Snap { at, .. } => x.abs() > at,
        }
    }

    /// How far the stick at `x` turns over `seconds` (degrees, right positive): a smooth turn by
    /// its share of the speed, a snap once per push (`latch`).
    pub fn turn(&self, x: f32, seconds: f32, latch: &mut TurnLatch) -> f32 {
        match *self {
            Self::Smooth { speed, .. } => self.share(x) * speed * seconds,
            Self::Snap { degrees, at, release } => {
                if !latch.snapped && x.abs() > at {
                    latch.snapped = true;
                    degrees * x.signum()
                } else {
                    if x.abs() < release {
                        latch.snapped = false;
                    }
                    0.0
                }
            }
        }
    }
}

/// A stick axis (-1..1) as XInput's, with [`STICK_DEADZONE`] taken out and the rest rescaled.
pub fn stick(value: f32) -> i16 {
    if !value.is_finite() {
        return 0;
    }
    let magnitude = ((value.abs() - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).clamp(0.0, 1.0);
    (magnitude.copysign(value) * 32767.0).round() as i16
}

/// A trigger (0..1) as XInput's.
pub fn trigger(value: f32) -> u8 {
    if !value.is_finite() {
        return 0;
    }
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A real pad and the VR one as one: buttons either, the stronger trigger, sticks added (each
/// alone reaches full range; together they saturate).
pub fn merge(real: Gamepad, vr: Gamepad) -> Gamepad {
    let add = |a: i16, b: i16| a.saturating_add(b);
    Gamepad {
        buttons: real.buttons | vr.buttons,
        left_trigger: real.left_trigger.max(vr.left_trigger),
        right_trigger: real.right_trigger.max(vr.right_trigger),
        thumb_lx: add(real.thumb_lx, vr.thumb_lx),
        thumb_ly: add(real.thumb_ly, vr.thumb_ly),
        thumb_rx: add(real.thumb_rx, vr.thumb_rx),
        thumb_ry: add(real.thumb_ry, vr.thumb_ry),
    }
}

/// A real stick never holds a value exactly, so a game sees it moving every read; some (Dying
/// Light through SDL) only move the character while stick events keep coming. The axes `held`
/// holds off centre get their lowest bit flipped while `odd` (alternating every few frames, by
/// time, never per read: a reader may read many times a frame) in `pad`: one part in
/// 32767, never felt. Axes `held` leaves centred (a real pad's own resting drift) are untouched.
pub fn dither(pad: Gamepad, held: Gamepad, odd: bool) -> Gamepad {
    let jiggle = |v: i16, by: i16| if by != 0 && odd { v ^ 1 } else { v };
    Gamepad {
        thumb_lx: jiggle(pad.thumb_lx, held.thumb_lx),
        thumb_ly: jiggle(pad.thumb_ly, held.thumb_ly),
        thumb_rx: jiggle(pad.thumb_rx, held.thumb_rx),
        thumb_ry: jiggle(pad.thumb_ry, held.thumb_ry),
        ..pad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The producers' stick response as it was written before [`StickTurn`].
    fn old_turn_response(v: f32) -> f32 {
        let past = ((v.abs() - TURN_DEADZONE) / (1.0 - TURN_DEADZONE)).clamp(0.0, 1.0);
        v.signum() * past.powf(1.5)
    }

    #[test]
    fn turning_has_a_deadzone_and_eases_in() {
        let turn = StickTurn::smooth(150.0);
        assert_eq!(turn.share(0.1), 0.0);
        assert_eq!(turn.share(-0.15), 0.0);
        assert!((turn.share(1.0) - 1.0).abs() < 1e-6 && (turn.share(-1.0) + 1.0).abs() < 1e-6);
        let half = turn.share(0.575);
        assert!(half > 0.3 && half < 0.4, "eased: {half}");
    }

    #[test]
    fn the_smooth_turn_is_the_old_stick_response_times_speed() {
        let mut latch = TurnLatch::new();
        for speed in [150.0f32, 90.0, 720.0] {
            let turn = StickTurn::smooth(speed);
            for i in -40..=40 {
                let x = i as f32 / 40.0;
                assert_eq!(turn.share(x), old_turn_response(x), "{x}");
                assert_eq!(turn.pushed(x), old_turn_response(x) != 0.0, "{x}");
                for step in [0.0f32, 0.011, 0.1] {
                    // Head aim through the mouse: degrees, `x * speed * step` in that order.
                    assert_eq!(turn.turn(x, step, &mut latch), old_turn_response(x) * speed * step);
                    // DL1's facing: radians, the speed turned to radians first.
                    let old = old_turn_response(x) * speed.to_radians() * step;
                    assert!((turn.turn(x, step, &mut latch).to_radians() - old).abs() <= 1e-6 * old.abs().max(1e-3), "{x} {step}");
                }
            }
        }
    }

    #[test]
    fn a_snap_turns_once_per_push_and_a_straight_smooth_turn_is_linear() {
        // As eng_unr's first person turns: a straight smooth turn past 0.2, and a 30 degree snap.
        let straight = StickTurn::Smooth { speed: 120.0, dead_zone: 0.2, ease: 1.0 };
        let mut latch = TurnLatch::new();
        assert_eq!(straight.turn(0.15, 0.1, &mut latch), 0.0);
        assert!((straight.turn(0.6, 0.1, &mut latch) - 0.5 * 120.0 * 0.1).abs() < 1e-5);
        assert!((straight.turn(-1.0, 0.1, &mut latch) + 12.0).abs() < 1e-5);
        let snap = StickTurn::SNAP;
        let pushes = [0.0, 0.8, 0.9, 0.5, 0.8, 0.2, -0.75, -0.8, 0.0, 0.71];
        let turned: Vec<f32> = pushes.iter().map(|&x| snap.turn(x, 0.011, &mut latch)).collect();
        assert_eq!(turned, [0.0, 30.0, 0.0, 0.0, 0.0, 0.0, -30.0, 0.0, 0.0, 30.0], "back within 0.3 before the next");
        assert!(snap.pushed(0.75) && !snap.pushed(0.6) && snap.share(1.0) == 0.0);
    }

    #[test]
    fn the_turn_option_picks_smooth_or_snap() {
        let manifest = Manifest { options: &[crate::manifest::TURN], ..Manifest::GAME };
        let pick = |text: &str| StickTurn::from_options(&Options::parse(text), &manifest, StickTurn::smooth(150.0), StickTurn::SNAP);
        assert_eq!(pick(""), StickTurn::smooth(150.0));
        assert_eq!(pick("turn=snap"), StickTurn::SNAP);
        assert_eq!(pick("turn=Smooth"), StickTurn::smooth(150.0));
    }

    #[test]
    fn layout_is_xinputs() {
        assert_eq!(size_of::<Gamepad>(), 12);
        assert_eq!(std::mem::offset_of!(Gamepad, thumb_lx), 4);
    }

    #[test]
    fn axes_convert_with_a_deadzone() {
        assert_eq!(stick(0.1), 0, "drift stays centred");
        assert_eq!(stick(1.0), 32767);
        assert_eq!(stick(-1.0), -32767);
        assert!(stick(0.56) > 16000 && stick(0.56) < 16800, "half way past the deadzone is about half");
        assert_eq!(stick(f32::NAN), 0);
        assert_eq!((trigger(0.0), trigger(1.0), trigger(2.0), trigger(-1.0)), (0, 255, 255, 0));
    }

    #[test]
    fn dither_moves_only_held_sticks() {
        let pad = Gamepad { buttons: button::A, thumb_ly: 32767, thumb_rx: -20000, ..Gamepad::default() };
        assert_eq!(dither(pad, pad, false), pad);
        let odd = dither(pad, pad, true);
        assert_eq!((odd.thumb_ly, odd.thumb_rx, odd.thumb_lx, odd.buttons), (32766, -19999, 0, button::A));
        let drift = Gamepad { thumb_lx: 817, ..pad };
        assert_eq!(dither(drift, pad, true).thumb_lx, 817, "a real pad's drift is not jiggled");
    }

    #[test]
    fn pads_merge() {
        let real = Gamepad { buttons: button::A, left_trigger: 10, thumb_lx: 30000, ..Gamepad::default() };
        let vr = Gamepad { buttons: button::B, left_trigger: 200, thumb_lx: 10000, thumb_ry: -500, ..Gamepad::default() };
        let both = merge(real, vr);
        assert_eq!(both.buttons, button::A | button::B);
        assert_eq!((both.left_trigger, both.thumb_lx, both.thumb_ry), (200, i16::MAX, -500));
        assert_eq!(merge(Gamepad::default(), vr), vr);
    }
}
