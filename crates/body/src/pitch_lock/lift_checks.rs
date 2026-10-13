//! The pitch lock against Dying Light 2's own, which it replaced: the value each vertical look input
//! passed on, and what was counted.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32};

/// What Dying Light 2's lock did with one vertical look input while head aim did not steer: with
/// stereo running and the camera's pitch known, input turning it back toward level from beyond 1.5
/// degrees passed (counted as levelling when it was input), other input got its neutral value
/// (counted as held when it was input). The values, and (levelling, held) counted.
fn before(value: f32, neutral: f32, up: bool, running: bool, pitch: Option<f32>) -> (f32, (u64, u64)) {
    if !running {
        return (value, (0, 0));
    }
    let toward = pitch.is_some_and(|pitch| if up { pitch < -0.026 } else { pitch > 0.026 });
    if toward {
        return (value, (u64::from(neutral != value), 0));
    }
    (neutral, (0, u64::from(neutral != value)))
}

static HOLDS: AtomicBool = AtomicBool::new(false);
/// The camera's pitch (f32 bits; NaN: not known).
static PITCH: AtomicU32 = AtomicU32::new(0);

#[test]
fn each_input_passes_as_dying_light_2s_lock_passed_it() {
    *RULE.locked() = Some(Rule { pitch: || Some(f32::from_bits(PITCH.load(Relaxed))).filter(|p| !p.is_nan()), holds: || HOLDS.load(Relaxed), stick_in_use: || true });
    for running in [false, true] {
        for pitch in [None, Some(-0.5), Some(-0.026), Some(-0.02), Some(0.0), Some(0.02), Some(0.027), Some(0.9)] {
            for up in [false, true] {
                for (value, neutral) in [(0.0, 0.0), (0.7, 0.0), (1.0, 1.0), (0.2, 1.0), (-1.0, 0.0)] {
                    HOLDS.store(running, Relaxed);
                    PITCH.store(pitch.unwrap_or(f32::NAN).to_bits(), Relaxed);
                    let counted = (levelling(), blocked());
                    let now = pass(value, neutral, up);
                    let (old, (levelled, held)) = before(value, neutral, up, running, pitch);
                    assert_eq!(now.to_bits(), old.to_bits(), "{value} {neutral} up {up} running {running} {pitch:?}");
                    assert_eq!((levelling() - counted.0, blocked() - counted.1), (levelled, held), "{value} {neutral} up {up} running {running} {pitch:?}");
                }
            }
        }
    }
    *RULE.locked() = None;
    assert_eq!(pass(0.7, 0.0, true), 0.7, "without the lock, input as the game made it");
}
