//! Crouching by ducking, as a ready-made (`duck`): the head lowered from standing crouches the
//! character, and standing up stands it up again ([`monaka_core::gesture::Duck`]). The crouch is
//! held through the actions provider ([`Action::Crouch`], `monaka_pad::actions`), however the game
//! takes it: held down, tapped as a toggle, or its own way.
//!
//! A game lists [`duck`] and hands the head's height to [`note_head`] each time it places a view
//! (its eye site). The head is followed all the time; the crouch is asked only while the component
//! is on, and let go at its stop. The standing height is kept from run to run, as the playspace is.

use crate::hand_trail;
use monaka_core::feature::Feature;
use monaka_core::gesture::{Duck, DuckRules};
use monaka_core::sync::Locked;
use monaka_pad::actions::{self, Action, Actions};
use monaka_producer::game::Component;
use std::sync::Mutex;

/// The head 0.35 m below standing crouches; back within 0.22 m stands. While standing, the standing
/// height follows a lower head down at 0.02 m/s: sitting down or the tracking drifting does not
/// leave the player crouched.
pub const RULES: DuckRules = DuckRules::CROUCH;

/// Crouching by ducking for game `G`: the crouch held through its actions ([`Actions`]) while the
/// head is down. A game says what it works on top of by struct update.
pub const fn duck<G: Actions>() -> Component<G> {
    Component { name: "duck", features: &[Feature::Duck], arm, quiet, ..Component::NONE }
}

/// The head against its standing height, whether the crouch may be asked, and the crouch as last
/// asked of the game.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ducking {
    head: Duck,
    on: bool,
    asked: bool,
}

impl Ducking {
    pub const fn new() -> Self {
        Self { head: Duck::new(), on: false, asked: false }
    }

    /// The head at `height` (metres, up) at `time` (seconds): while asking is on, the crouch the
    /// head wants is asked (`ask`, true when the game took it) whenever it differs from the one
    /// last asked. A height not finite is skipped.
    pub fn feed(&mut self, time: f64, height: f32, rules: &DuckRules, ask: impl FnOnce(bool) -> bool) {
        if !height.is_finite() {
            return;
        }
        let crouched = self.head.feed(time, height, rules);
        if self.on && crouched != self.asked && ask(crouched) {
            self.asked = crouched;
        }
    }

    /// Asking starts (the component armed): a head already down asks for the crouch at once.
    pub fn start(&mut self, ask: impl FnOnce(bool) -> bool) {
        self.on = true;
        let crouched = self.head.crouched();
        if crouched != self.asked && ask(crouched) {
            self.asked = crouched;
        }
    }

    /// Asking stops, a crouch still asked let go (`ask`).
    pub fn stop(&mut self, ask: impl FnOnce(bool) -> bool) {
        self.on = false;
        if self.asked {
            ask(false);
            self.asked = false;
        }
    }

    /// The crouch as last asked.
    pub fn asked(&self) -> bool {
        self.asked
    }
}

static DUCKING: Mutex<Ducking> = Mutex::new(Ducking::new());

/// The crouch held or let go through the actions provider.
fn ask(on: bool) -> bool {
    actions::hold(Action::Crouch, on)
}

/// The head's height in tracking space (metres, y up), each time a view is placed: the crouch held
/// or let go as it changes.
pub fn note_head(height: f32) {
    DUCKING.locked().feed(hand_trail::now(), height, &RULES, ask);
}

fn arm() {
    DUCKING.locked().start(ask);
}

fn quiet() {
    DUCKING.locked().stop(ask);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crouch_is_asked_as_the_head_crosses_the_band_and_only_then() {
        let mut ducking = Ducking::new();
        ducking.start(|_| unreachable!("standing: nothing to ask"));
        let mut duck = Duck::new();
        let heights = [1.7, 1.6, 1.4, 1.3, 1.3, 1.45, 1.5, 1.6, 1.2, 1.25, f32::NAN, 1.7];
        let mut asks = Vec::new();
        for (i, &height) in heights.iter().enumerate() {
            let time = 0.05 * i as f64;
            let crouched = if height.is_finite() { duck.feed(time, height, &RULES) } else { duck.crouched() };
            ducking.feed(time, height, &RULES, |on| {
                asks.push((i, on));
                true
            });
            assert_eq!(ducking.asked(), crouched, "{i}");
        }
        assert_eq!(asks, [(3, true), (6, false), (8, true), (11, false)]);
    }

    #[test]
    fn an_ask_not_taken_is_asked_again_and_none_while_off() {
        let mut ducking = Ducking::new();
        let mut asks = 0;
        ducking.feed(0.0, 1.7, &RULES, |_| unreachable!());
        ducking.feed(0.05, 1.2, &RULES, |_| unreachable!("off: nothing asked"));
        ducking.start(|on| {
            asks += 1;
            !on
        });
        ducking.feed(0.15, 1.2, &RULES, |on| {
            asks += 1;
            on
        });
        assert_eq!((asks, ducking.asked()), (2, true), "not taken: asked again");
        ducking.feed(0.2, 1.2, &RULES, |_| unreachable!("already asked"));
        let mut let_go = None;
        ducking.stop(|on| {
            let_go = Some(on);
            true
        });
        assert_eq!((let_go, ducking.asked()), (Some(false), false), "let go at the stop");
    }
}
