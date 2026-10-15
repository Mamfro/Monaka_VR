//! A gamepad made from VR controllers: the viewer turns its OpenXR actions into one Xbox-layout
//! pad ([`Gamepad`], XInput's `XINPUT_GAMEPAD`), a producer hands it to the game's own gamepad
//! input, merged with a real pad when one is plugged in ([`merge`]). A producer that turns the
//! player itself with the right stick does it through [`StickTurn`].

use crate::options::Options;
use monaka_contract::gamepad::Gamepad;
use monaka_contract::manifest::Manifest;

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

    /// The turn the `turn` option asks for ([`monaka_contract::manifest::TURN`]; the game's default when
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
mod tests;
