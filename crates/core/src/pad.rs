//! A gamepad made from VR controllers: the viewer turns its OpenXR actions into one Xbox-layout
//! pad ([`Gamepad`], XInput's `XINPUT_GAMEPAD`), a producer hands it to the game's own gamepad
//! input, merged with a real pad when one is plugged in ([`merge`]).

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

/// Stick deflection a producer's own turning ignores around the centre ([`turn_response`]).
pub const TURN_DEADZONE: f32 = 0.15;

/// A stick axis (-1..=1) a producer turns the player with: past [`TURN_DEADZONE`], rescaled to
/// -1..=1 and eased in for fine control.
pub fn turn_response(v: f32) -> f32 {
    let past = ((v.abs() - TURN_DEADZONE) / (1.0 - TURN_DEADZONE)).clamp(0.0, 1.0);
    v.signum() * past.powf(1.5)
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

    #[test]
    fn turning_has_a_deadzone_and_eases_in() {
        assert_eq!(turn_response(0.1), 0.0);
        assert_eq!(turn_response(-0.15), 0.0);
        assert!((turn_response(1.0) - 1.0).abs() < 1e-6 && (turn_response(-1.0) + 1.0).abs() < 1e-6);
        let half = turn_response(0.575);
        assert!(half > 0.3 && half < 0.4, "eased: {half}");
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
