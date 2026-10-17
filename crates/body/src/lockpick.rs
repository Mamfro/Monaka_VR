//! Lockpicking by hand, as a ready-made (`lockpick`): while the game's lockpicking minigame runs,
//! the left hand's twist turns the pick ([`Action::Pick`]) and the right hand's twist turns the
//! tool holding it ([`Action::Turn`]), through the actions provider (`monaka_pad::actions`), instead
//! of the sticks.
//!
//! A game implements [`Lockpicks`]: how its twist is read and how far it goes ([`Lockpicking`]),
//! which point of its hands it reads and when ([`crate::hands`]), whether a lock is being picked now, and its minigame's hook where it takes
//! the pick and the turn its own way. The pick and the turn follow the hands from the install on,
//! but give a value only while a lock is being picked, so outside the minigame the sticks stay the
//! player's. Each is the hand's twist since the lock began: the first read of the pick in a lock
//! (a fresh one) takes both hands' starts, the first read of the turn its own hand's again.

use crate::hand_rig::HandRig;
use crate::hands::{self, At, In, Point};
use monaka_core::feature::Feature;
use monaka_core::gesture::wrist_roll;
use monaka_core::math::{Quat, cross, dot, forward_axis, right_axis, wrap_degrees};
use monaka_contract::protocol::{HandPose, LEFT_HAND, RIGHT_HAND};
use monaka_core::sync::Locked;
use monaka_pad::actions::{self, Action, Actions, Reading, Source};
use monaka_producer::game::{Component, Lack, Need, Start, Switch, lines};
use monaka_producer::log;
use std::sync::Mutex;

/// How a hand's twist is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Twist {
    /// The hand's roll about where it points now, from its orientation as the lock began.
    FromStart,
    /// The hand's roll against the world's up, from its roll as the lock began (a hand pointing
    /// nearly straight up or down has none to read; one not readable as the lock began starts
    /// level).
    AgainstUp,
}

/// How a game's twists turn the pick and the tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lockpicking {
    pub twist: Twist,
    /// The pick's angle per radian of the left hand's twist.
    pub pick_gain: f32,
    /// The right hand's twist (radians) that turns the tool fully either way.
    pub full_turn: f32,
    /// Which point of each hand the twist is read from, and at which moment (in tracking space).
    pub hand: (Point, At),
}

/// A game's part in lockpicking by hand: its rules, whether a lock is being picked, and
/// the minigame's hook (where the game takes the pick and the turn its own way).
pub trait Lockpicks: Actions + HandRig {
    const LOCKPICKING: Lockpicking;

    /// Whether a lock is being picked now: the pick and the turn have a value only then.
    fn picking() -> bool;

    /// Hooks the minigame, where the game takes the pick and the turn its own way.
    fn install(_start: &mut Start<'_, Self>) -> Result<(), Lack> {
        Ok(())
    }

    /// The hooks' counts, for the stop's log (empty: nothing to say).
    fn summary() -> String {
        String::new()
    }
}

/// Lockpicking by hand for game `G`, with motion controls (`lockpick_by_hand=0`: the sticks, for a
/// test). A game says what it works on top of by struct update.
pub const fn lockpick<G: Lockpicks>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::HandRig)],
        name: "lockpick",
        features: &[Feature::Lockpick],
        switch: Some(Switch::new("lockpick_by_hand", true, "the game's lockpick")),
        install: install::<G>,
        release,
        summary: Some(summary::<G>),
        ..Component::NONE
    }
}

/// Where a hand's twist was as the lock began.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Begun {
    Orientation(Quat),
    /// Degrees.
    Roll(f32),
}

/// A hand's twist now, as its rules read it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Now {
    Orientation(Quat),
    Roll(f32),
}

/// The tracking space's up.
const UP: [f32; 3] = [0.0, 1.0, 0.0];

impl Twist {
    /// The twist of `hand` now; none where it has none to read.
    fn now(self, hand: &HandPose) -> Option<Now> {
        match self {
            Twist::FromStart => Some(Now::Orientation(hand.orientation)),
            Twist::AgainstUp => wrist_roll(hand.orientation, UP).map(|roll| Now::Roll(roll.to_degrees())),
        }
    }

    /// The start for `hand` as a lock begins; one not readable starts level where the twist is
    /// against the world's up, and has none otherwise.
    fn begin(self, hand: Option<&HandPose>) -> Option<Begun> {
        match (self, hand.and_then(|hand| self.now(hand))) {
            (_, Some(now)) => Some(begun(now)),
            (Twist::AgainstUp, None) => Some(Begun::Roll(0.0)),
            (Twist::FromStart, None) => None,
        }
    }
}

fn begun(now: Now) -> Begun {
    match now {
        Now::Orientation(q) => Begun::Orientation(q),
        Now::Roll(roll) => Begun::Roll(roll),
    }
}

/// How far `now` is twisted from `start` (radians, clockwise as the hand sees it).
fn twisted(start: Begun, now: Now) -> f32 {
    match (start, now) {
        (Begun::Orientation(start), Now::Orientation(now)) => roll_between(start, now),
        (Begun::Roll(start), Now::Roll(now)) => wrap_degrees(now - start).to_radians(),
        _ => 0.0,
    }
}

/// How far `now` is rolled from `reference` about where `now` points (radians, positive clockwise
/// as the hand sees it).
fn roll_between(reference: Quat, now: Quat) -> f32 {
    let forward = forward_axis(now);
    let flat = |v: [f32; 3]| {
        let d = dot(v, forward);
        [v[0] - forward[0] * d, v[1] - forward[1] * d, v[2] - forward[2] * d]
    };
    let (from, to) = (flat(right_axis(reference)), flat(right_axis(now)));
    -dot(cross(from, to), forward).atan2(dot(from, to))
}

/// The hand that sets `action`: the left the pick, the right the turn.
fn side(action: Action) -> usize {
    if action == Action::Pick { LEFT_HAND } else { RIGHT_HAND }
}

impl Lockpicking {
    /// The pick's angle (radians) or the tool's turn (-1 to 1) for a hand twisted `turn` (radians).
    pub fn answer(&self, action: Action, turn: f32) -> f32 {
        if action == Action::Pick { turn * self.pick_gain } else { (turn / self.full_turn).clamp(-1.0, 1.0) }
    }
}

/// Both hands' starts in the lock being picked, whether the turn's own fresh read has come since
/// the pick's, and the locks begun.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Picking {
    starts: [Option<Begun>; 2],
    turn_begun: bool,
    locks: u64,
}

impl Picking {
    pub const fn new() -> Self {
        Self { starts: [None; 2], turn_begun: false, locks: 0 }
    }

    /// The value of `action` read as `reading`, the hands at `hands` (by side): a fresh read of the
    /// pick takes both hands' starts (the right hand's only where the turn's own fresh read has not
    /// come since the pick's last), a fresh read of the turn its own hand's; a hand with no start
    /// takes its twist now as the start.
    pub fn value(&mut self, rules: &Lockpicking, action: Action, reading: Reading, hands: [Option<HandPose>; 2]) -> Option<f32> {
        let side = side(action);
        let now = rules.twist.now(hands[side].as_ref()?)?;
        if reading.fresh {
            if action == Action::Pick {
                if !std::mem::take(&mut self.turn_begun) {
                    self.starts[RIGHT_HAND] = rules.twist.begin(hands[RIGHT_HAND].as_ref());
                }
                self.locks += 1;
            } else {
                self.turn_begun = true;
            }
            self.starts[side] = Some(begun(now));
        }
        let start = *self.starts[side].get_or_insert(begun(now));
        Some(rules.answer(action, twisted(start, now)))
    }

    /// The locks begun (fresh reads of the pick).
    pub fn locks(&self) -> u64 {
        self.locks
    }
}

static PICKING: Mutex<Picking> = Mutex::new(Picking::new());
static INSTALLED: Mutex<bool> = Mutex::new(false);

/// Hand `side` of game `G` as its lockpicking reads it.
fn hand<G: Lockpicks>(side: usize) -> Option<HandPose> {
    let (point, at) = G::LOCKPICKING.hand;
    hands::pose::<G>(side, point, at, In::Tracking)
}

/// Whether hand `side`'s twist can be read now, while a lock is being picked.
fn ready<G: Lockpicks>(action: Action) -> bool {
    G::picking() && hand::<G>(side(action)).is_some_and(|hand| G::LOCKPICKING.twist.now(&hand).is_some())
}

fn value<G: Lockpicks>(action: Action, reading: Reading) -> Option<f32> {
    if !G::picking() {
        return None;
    }
    let hands = [hand::<G>(LEFT_HAND), hand::<G>(RIGHT_HAND)];
    PICKING.locked().value(&G::LOCKPICKING, action, reading, hands)
}

/// The pick and the turn, from game `G`'s hands.
pub const fn source<G: Lockpicks>() -> Source {
    Source { ready: ready::<G>, value: value::<G> }
}

fn install<G: Lockpicks>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    G::install(start)?;
    *PICKING.locked() = Picking::new();
    let followed = actions::follow(Action::Pick, Some(source::<G>())) & actions::follow(Action::Turn, Some(source::<G>()));
    *INSTALLED.locked() = true;
    log!("lockpicking by hand: the left hand's twist turns the pick, the right hand's the tool holding it{}", if followed { "" } else { " (not taken by the game now)" });
    Ok(())
}

fn release() {
    actions::follow(Action::Pick, None);
    actions::follow(Action::Turn, None);
    *INSTALLED.locked() = false;
}

fn summary<G: Lockpicks>() -> String {
    lines(&[G::summary(), format!("lockpicking by hand: {} locks", PICKING.locked().locks())])
}

#[cfg(test)]
mod tests;
