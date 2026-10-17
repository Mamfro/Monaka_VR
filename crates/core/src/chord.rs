//! Monaka's own button chord: Start and Back held together for [`HOLD`] asks for a capture of what
//! the headset shows (the viewer's). Neither reaches the game while they may be the chord: a
//! lone press is held back for [`GRACE`] in case the other follows (Start alone would open the
//! game's pause menu), then goes to the game as usual; one let go within it still reaches the game,
//! as a tap of [`TAP`].

use monaka_contract::{gamepad::Gamepad, gamepad::button};
use std::time::{Duration, Instant};

/// How long both are held before the chord counts.
pub const HOLD: Duration = Duration::from_millis(500);
/// How long a lone press waits for the other before it goes to the game.
pub const GRACE: Duration = Duration::from_millis(120);
/// How long a press let go within [`GRACE`] is shown to the game.
pub const TAP: Duration = Duration::from_millis(80);

const KEYS: u16 = button::START | button::BACK;

#[derive(Default)]
pub struct Chord {
    /// A lone press held back: its button and since when.
    waiting: Option<(u16, Instant)>,
    /// Both held since then; the buttons stay with us until both are let go.
    both: Option<Instant>,
    fired: bool,
    /// A press let go within the grace, shown to the game until then.
    tap: Option<(u16, Instant)>,
}

impl Chord {
    /// Takes Start and Back out of `pad` while they may be the chord. True once per hold, when
    /// both have been held for [`HOLD`].
    pub fn step(&mut self, pad: &mut Gamepad, now: Instant) -> bool {
        let keys = pad.buttons & KEYS;
        if keys == KEYS {
            self.waiting = None;
            self.tap = None;
            let since = *self.both.get_or_insert(now);
            pad.buttons &= !KEYS;
            if !self.fired && now.duration_since(since) >= HOLD {
                self.fired = true;
                return true;
            }
            return false;
        }
        if self.both.is_some() {
            // The chord being let go: what is still held stays with us until both are up.
            pad.buttons &= !KEYS;
            if keys == 0 {
                (self.both, self.fired) = (None, false);
            }
            return false;
        }
        match self.waiting {
            Some((key, since)) if keys == 0 => {
                // Let go before it went to the game: a tap.
                self.waiting = None;
                if now.duration_since(since) < GRACE {
                    self.tap = Some((key, now));
                }
            }
            Some((key, since)) if keys == key => {
                if now.duration_since(since) < GRACE {
                    pad.buttons &= !KEYS;
                }
            }
            _ if keys != 0 => {
                self.waiting = Some((keys, now));
                self.tap = None;
                pad.buttons &= !KEYS;
            }
            _ => {}
        }
        if let Some((key, since)) = self.tap {
            if now.duration_since(since) < TAP {
                pad.buttons |= key;
            } else {
                self.tap = None;
            }
        }
        false
    }

    /// While one of Monaka's menus has the pad and leaves the game an idle one: the buttons as the
    /// controllers hold them, so the chord still asks for a capture over a menu.
    pub fn step_held(&mut self, buttons: u16, now: Instant) -> bool {
        self.step(&mut Gamepad { buttons, ..Gamepad::default() }, now)
    }
}

#[cfg(test)]
mod tests;
