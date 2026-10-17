//! Throwing by hand, as a ready-made (`throw`): a throwable leaves the left hand the moment the
//! player lets go, the way the left controller moved through its fastest moment and as hard; the
//! melee weapon thrown flies the way the right hand swung it. The look and the game's own aim play
//! no part.
//!
//! A game implements the throw release (`crate::launch::Throws`: where the player lets go and where
//! the item leaves, and how the game sends it off, [`Way`]) and [`ThrowsByHand`]: how fast what it
//! throws leaves ([`Throwing`]: its gains), where its controllers are noted and where each hand
//! holds what it throws. The ready-made answers the release's questions
//! ([`crate::launch::throw_by`]): the throw the hands made at the let-go, then where the item starts
//! and how it flies. Without an answer the game's own throw stands.

use crate::hand_rig::{self, HandRig};
use crate::hand_trail::HandTrails;
use crate::launch::{self, HandThrow, Leaving, Thrower, Throws, Thrown, Way};
use monaka_core::feature::Feature;
use monaka_core::gesture::{ReleaseRules, ThrowSpeed};
use monaka_core::math::{Vec3, length};
use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Lack, Need, Start, Switch, lines};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// How fast what the hands throw leaves: the hand's own throw. What it throws leaves from the hand
/// at the hand's speed times a gain, one for an object given a velocity (`given`) and one for an
/// object launched on its own (`launched`), and at the hand's own velocity toward a point. A hand
/// with no throw to read leaves the throw to the game. The melee weapon flies at the fastest the
/// right hand moved over the last `weapon_within` seconds, from the hand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Throwing {
    pub given: f32,
    pub launched: f32,
    pub weapon_within: f64,
}

/// The hand that throws a throwable, and the one that holds the melee weapon.
const THROWING_HAND: usize = LEFT_HAND;
const WEAPON_HAND: usize = RIGHT_HAND;

/// A game's part in throwing by hand: its throw release ([`Throws`]), how fast what it throws
/// leaves; its hands are the hand rig's ([`HandRig::trail`], [`hand_rig::posed_hand`]).
pub trait ThrowsByHand: Throws + HandRig {
    const THROWING: Throwing;
}

/// Throwing by hand for game `G`, with motion controls (`throw_by_hand=0`: the game's throw, for a
/// test). A game says what it works on top of by struct update.
pub const fn throw<G: ThrowsByHand>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::HandRig)],
        name: "throw",
        features: &[Feature::Throw],
        switch: Some(Switch::new("throw_by_hand", true, "the game's throw")),
        install: install::<G>,
        release,
        summary: Some(summary::<G>),
        ..Component::NONE
    }
}

/// The release read off a trail kept by `controllers`: the fastest over half a second, the way
/// through it, each speed over the trail's own span.
fn release_rules(controllers: &HandTrails) -> ReleaseRules {
    ReleaseRules { span: controllers.rules().span, ..ReleaseRules::THROW }
}

impl Throwing {
    /// The throw the hands made as the player let go of `thrown`: the controllers' motion on
    /// `controllers`, each hand where `hand` says.
    pub fn let_go(&self, thrown: Thrown, controllers: &HandTrails, hand: impl Fn(usize) -> Option<Vec3>) -> Option<HandThrow> {
        match thrown {
            Thrown::Throwable => {
                let release = controllers.release(THROWING_HAND, &release_rules(controllers))?;
                hand(THROWING_HAND).map(|at| HandThrow { from: Some(at), velocity: release.velocity })
            }
            Thrown::Weapon => hand(WEAPON_HAND).zip(controllers.fastest(WEAPON_HAND, self.weapon_within)).map(|(at, velocity)| HandThrow { from: Some(at), velocity }),
        }
    }

    /// Where `leaving` starts: at the hand; none: where the game starts it.
    pub fn start(&self, leaving: &Leaving) -> Option<Vec3> {
        leaving.hand.from
    }

    /// How fast and which way `leaving` flies; none: as the game throws it.
    pub fn velocity(&self, leaving: &Leaving) -> Option<Vec3> {
        let hand = leaving.hand.velocity;
        match leaving.way {
            Way::Given => ThrowSpeed::gain(self.given).thrown(hand),
            Way::Launched => ThrowSpeed::gain(self.launched).thrown(hand),
            Way::AtPoint => Some(hand),
        }
    }
}

static LET_GOS: AtomicU64 = AtomicU64::new(0);
static THROWS: AtomicU64 = AtomicU64::new(0);
static INSTALLED: Mutex<bool> = Mutex::new(false);

fn let_go<G: ThrowsByHand>(thrown: Thrown) -> Option<HandThrow> {
    let controllers = G::trail()?;
    let hand = G::THROWING.let_go(thrown, controllers, hand_rig::posed_hand::<G>);
    let n = LET_GOS.fetch_add(1, Relaxed) + 1;
    if n <= 30 {
        let speed = hand.map_or(f32::NAN, |hand| length(hand.velocity));
        log!("throwing by hand: let go of a {thrown:?} {n}: {speed:.2} m/s{}", if hand.is_some() { "" } else { " (left to the game)" });
    }
    hand
}

fn start<G: ThrowsByHand>(leaving: &Leaving) -> Option<Vec3> {
    G::THROWING.start(leaving)
}

fn velocity<G: ThrowsByHand>(leaving: &Leaving) -> Option<Vec3> {
    let throw = G::THROWING.velocity(leaving)?;
    if THROWS.fetch_add(1, Relaxed) < 40 {
        let game = leaving.game.map_or(f32::NAN, length);
        log!("throwing by hand: a {:?} {:?}, the hand at {:.1} m/s, the game's {game:.1} m/s, thrown at {:.1} m/s", leaving.thrown, leaving.way, length(leaving.hand.velocity), length(throw));
    }
    Some(throw)
}

/// The answers game `G`'s throw release asks.
pub const fn thrower<G: ThrowsByHand>() -> Thrower {
    Thrower { let_go: let_go::<G>, start: start::<G>, velocity: velocity::<G>, throwing: <G as Throws>::throwing }
}

fn install<G: ThrowsByHand>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    <G as Throws>::install(start)?;
    launch::throw_by(Some(thrower::<G>()));
    *INSTALLED.locked() = true;
    log!("throwing by hand: a throwable leaves the left hand as it is let go, the melee weapon the way the right hand swung it");
    Ok(())
}

fn release() {
    launch::throw_by(None);
    *INSTALLED.locked() = false;
}

fn summary<G: ThrowsByHand>() -> String {
    lines(&[<G as Throws>::summary(), format!("throwing by hand: {} let go, {} throws", LET_GOS.load(Relaxed), THROWS.load(Relaxed))])
}

#[cfg(test)]
mod tests;
