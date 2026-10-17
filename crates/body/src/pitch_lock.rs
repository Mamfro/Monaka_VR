//! The pitch lock, as a ready-made component ([`pitch_lock`], `lock_pitch`, on by default): while
//! head aim does not steer the character, a stick's vertical look input passes only when it turns
//! the game camera back toward level, so the stick turns and the head alone pitches the view. The
//! mouse keeps its vertical look where the game tells it from a stick ([`PitchInput::stick_in_use`]);
//! where it cannot, the mouse is held too. It holds for every look method: a game says once where
//! its vertical look input arrives and what its camera's pitch is ([`PitchInput`]), and its hook
//! there asks [`pass`] for each input while head aim does not steer (while it steers, the look
//! method holds that input neutral itself).

use monaka_core::feature::Feature;
use monaka_core::aim::Levelling;
use monaka_contract::manifest::{Kind, Opt, line};
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Game, Lack, Need, Start, Switch};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[cfg(test)]
mod tests;

/// The pitch lock's option: on by default (the stick turns only; the head alone pitches).
pub const LOCK_PITCH: Opt = line(
    "lock_pitch",
    "Block stick pitch",
    "With mouse or stick aim: no looking up and down with the stick, except back toward level. In a game that cannot tell the mouse from the stick, the mouse too.",
    Kind::Switch { default: true },
);

/// A game's vertical look, for the pitch lock: where its look input arrives and its camera's pitch.
pub trait PitchInput: Game {
    /// Hooks where the game's vertical look input arrives (a second install does nothing): its hook
    /// asks [`pass`] for each vertical look input while head aim does not steer.
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack>;
    /// The game camera's own pitch now (radians, positive up), when known.
    fn pitch() -> Option<f32>;
    /// Whether the lock holds now: while the eyes are drawn.
    fn holds() -> bool;
    /// Whether the vertical look input now comes from a stick, not the mouse. A game that cannot
    /// tell keeps the default, and the mouse is held with the stick.
    fn stick_in_use() -> bool {
        true
    }
}

/// The pitch lock component for game `G` (`lock_pitch=0`: vertical look as the game has it). A game
/// says what it works on top of (its eyes) by struct update.
pub const fn pitch_lock<G: PitchInput>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::Stereo)],
        name: "pitch lock",
        options: &[LOCK_PITCH],
        switch: Some(Switch::new("lock_pitch", true, "vertical look as the game has it")),
        install: install::<G>,
        release,
        ..Component::NONE
    }
}

/// The game's answers the lock reads, from its install to its release.
#[derive(Clone, Copy)]
struct Rule {
    pitch: fn() -> Option<f32>,
    holds: fn() -> bool,
    stick_in_use: fn() -> bool,
}

static RULE: Mutex<Option<Rule>> = Mutex::new(None);
static LEVELLING: AtomicU64 = AtomicU64::new(0);
static BLOCKED: AtomicU64 = AtomicU64::new(0);

/// Vertical look passes back toward level from beyond 1.5 degrees (radians). The game writes its own
/// pitch, so nothing is stepped.
const LEVEL: Levelling = Levelling { dead_band: 0.026, step: 0.0 };

fn install<G: PitchInput>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    G::install(start)?;
    *RULE.locked() = Some(Rule { pitch: G::pitch, holds: G::holds, stick_in_use: G::stick_in_use });
    log!("pitch lock: without head aim, vertical look passes only back toward level (lock_pitch=0 lets it through)");
    Ok(())
}

fn release() {
    if RULE.locked().take().is_some() {
        log!("pitch lock: {} vertical look inputs held, {} passed toward level", blocked(), levelling());
    }
}

/// One vertical look input while head aim does not steer: `value` as the game made it, `neutral`
/// its value without input, `up` whether it turns the camera up. The value passed on: `value` back
/// toward level (or while the lock is off, does not hold, or the mouse is in use), else `neutral`.
pub fn pass(value: f32, neutral: f32, up: bool) -> f32 {
    let Some(rule) = *RULE.locked() else { return value };
    let holds = (rule.holds)() && (rule.stick_in_use)();
    let passed = passed(value, neutral, up, holds, (rule.pitch)());
    if holds && neutral != value {
        if passed == value { &LEVELLING } else { &BLOCKED }.fetch_add(1, Relaxed);
    }
    passed
}

/// The lock's rule: with the lock holding (`holds`) and the camera at `pitch`, `value` (its
/// `neutral` without input, turning up with `up`) passes back toward level and is held at `neutral`
/// otherwise.
fn passed(value: f32, neutral: f32, up: bool, holds: bool, pitch: Option<f32>) -> f32 {
    if !holds || pitch.is_some_and(|pitch| LEVEL.toward_level(pitch, up)) { value } else { neutral }
}

/// Vertical look inputs the lock held at their neutral value so far.
pub fn blocked() -> u64 {
    BLOCKED.load(Relaxed)
}

/// Vertical look inputs the lock let through back toward level so far.
pub fn levelling() -> u64 {
    LEVELLING.load(Relaxed)
}
