//! A button held to open one of Monaka's own menus over the game (Start for Monaka's home, Back
//! for the debug menu): held for [`HOLD`] it acts; a shorter press still reaches the game, as a tap
//! of [`TAP`] when let go ([`LongPress`]). Start and Back held together belong to the capture chord
//! ([`crate::chord`]), so neither acts then ([`HoldToggle`]). The viewer and the harness both put
//! the pad through these, so a script's held button acts as a player's would.

use monaka_contract::{gamepad::Gamepad, gamepad::button};
use std::time::{Duration, Instant};

/// How long a button is held before it acts.
pub const HOLD: Duration = Duration::from_millis(500);
/// How long a short press let go is shown to the game.
pub const TAP: Duration = Duration::from_millis(80);
/// Held together, the capture chord's.
const CHORD: u16 = button::BACK | button::START;

/// A button that does something of Monaka's when held, and reaches the game as a tap otherwise.
#[derive(Clone, Copy, Debug)]
pub struct LongPress {
    bit: u16,
    /// Held since then, and whether it acted (or was taken by the capture chord) in this hold.
    since: Option<(Instant, bool)>,
    /// A short press let go, shown to the game until then.
    tap_until: Option<Instant>,
}

impl LongPress {
    pub const fn new(bit: u16) -> Self {
        Self { bit, since: None, tap_until: None }
    }

    /// Takes the button out of `pad` while it may be a long press; true once when it has been
    /// held long enough. A short press is put back as a tap when let go.
    pub fn step(&mut self, pad: &mut Gamepad, now: Instant) -> bool {
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
    pub fn yield_to_chord(&mut self, now: Instant) {
        self.since = Some((self.since.map_or(now, |(since, _)| since), true));
        self.tap_until = None;
    }
}

/// A menu opened and closed by holding a button ([`LongPress`]), never while Start and Back are
/// held together.
#[derive(Clone, Copy, Debug)]
pub struct HoldToggle {
    press: LongPress,
}

impl HoldToggle {
    pub const fn new(bit: u16) -> Self {
        Self { press: LongPress::new(bit) }
    }

    /// Whether this read of `pad` opens or closes the menu; the button is taken out of `pad` while
    /// it may be a hold.
    pub fn toggled(&mut self, pad: &mut Gamepad, now: Instant) -> bool {
        if pad.buttons & CHORD == CHORD {
            self.press.yield_to_chord(now);
            return false;
        }
        self.press.step(pad, now)
    }
}

#[cfg(test)]
mod tests;
