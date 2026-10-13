//! What the hands hold on to: whether the off hand rides a weapon the game holds two-handed (by
//! how close the off wrist is to the aim hand's weapon, debounced; [`two_handed`]), unless the game
//! declares it holds nothing so ([`declare_one_handed`]) or says what is held now keeps the hands
//! apart ([`hands_apart`]); and what the aim palm holds on to ([`Hold`]).

use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};

/// The game holds what is in the aim hand with both hands when the other wrist sits this close to
/// it (metres, its weapon holder, as animated): a rifle's front hand is about 0.25 m from the grip,
/// a free hand at rest 0.8 m away. The two thresholds and the hold
/// time keep a hand passing by, or an animation's moment, from flipping it.
pub(super) const TWO_HANDED_WITHIN: f32 = 0.40;
pub(super) const ONE_HANDED_BEYOND: f32 = 0.55;
pub(super) const HOLD_SWITCH_MS: u64 = 300;

/// Set by [`declare_one_handed`]: the game never holds anything with both hands.
pub(super) static ONE_HANDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Declares that the game holds nothing two-handed (Dishonored: a weapon or power per hand), so the
/// off hand always follows its own controller: with the rule on, a left wrist passing within 0.4 m
/// of the right hand's sword would hand the left arm the right controller's move.
pub fn declare_one_handed() {
    ONE_HANDED.store(true, Relaxed);
}

/// Set by [`hands_apart`]: what is held now keeps each hand on its own controller.
pub(super) static HANDS_APART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Says, each update, whether what the game holds now is held apart, each hand on its own
/// controller however close the game animates the hands (Dying Light 1's bow: the bow in the left
/// hand, the string in the right, the left wrist near the right hand as a rifle's would be).
pub fn hands_apart(apart: bool) {
    HANDS_APART.store(apart, Relaxed);
}

/// Whether the off hand rides a two-handed weapon (as of the latest update), and since when.
pub(super) static TWO_HANDED: Mutex<(bool, u64)> = Mutex::new((false, 0));

/// Whether the off hand rides a weapon held two-handed, as of the latest [`follow`]: its fingers
/// then grip as the game animates them.
pub fn off_hand_on_weapon() -> bool {
    TWO_HANDED.lock().is_ok_and(|s| s.0)
}

/// Whether the game holds the weapon with both hands now, from where the off wrist is against the
/// aim hand's holder, debounced.
pub(super) fn two_handed(distance: f32) -> bool {
    if ONE_HANDED.load(Relaxed) {
        return false;
    }
    let Ok(mut state) = TWO_HANDED.lock() else { return false };
    let now = monaka_channel::tick();
    if HANDS_APART.load(Relaxed) {
        if state.0 {
            *state = (false, now);
            note_two_handed(false, distance);
        }
        return false;
    }
    let wants = if state.0 { distance < ONE_HANDED_BEYOND } else { distance < TWO_HANDED_WITHIN };
    if wants == state.0 {
        state.1 = now;
    } else if now.saturating_sub(state.1) >= HOLD_SWITCH_MS {
        *state = (wants, now);
        note_two_handed(wants, distance);
    }
    state.0
}

pub(super) fn note_two_handed(both: bool, distance: f32) {
    static SWITCHES: AtomicU64 = AtomicU64::new(0);
    if SWITCHES.fetch_add(1, Relaxed) < 40 {
        log!("hand rig: {} (off wrist {distance:.2} m from the weapon)", if both { "two-handed: the off hand rides the weapon" } else { "one-handed: the off hand follows its own controller" });
    }
}
/// What the aim palm holds on to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Hold {
    Wrist = 1,
    Gun = 2,
    Melee = 3,
}

/// Logs switches between what the aim palm holds on to (the first few).
pub(super) fn note_hold(hold: Hold) {
    static STATE: AtomicU64 = AtomicU64::new(0);
    static SWITCHES: AtomicU64 = AtomicU64::new(0);
    let now = hold as u64;
    if STATE.swap(now, Relaxed) != now && SWITCHES.fetch_add(1, Relaxed) < 40 {
        log!(
            "hand rig: {}",
            match hold {
                Hold::Gun => "a gun, held by its grip in the palm",
                Hold::Melee => "a melee weapon locked to the controller by its holder",
                Hold::Wrist => "no gun, the wrist in the palm",
            }
        );
    }
}
