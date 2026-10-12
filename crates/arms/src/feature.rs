//! The finger rig as a ready-made component: finger tracking's options and its on/off, for a game
//! whose hand rig puts the fingers on with [`crate::finger_rig`]. A game lists
//! `monaka_arms::feature::fingers()` after its hand rig, saying what it works on top of (and its
//! own help line) by struct update: `Component { needs: &[&ARMS], ..fingers() }`. Whether a frame's
//! fingers follow tracking is then `game::on` of its place.

use monaka_core::feature::Feature;
use monaka_core::manifest::{BINARY_FINGERS, FINGER_TRACKING};
use monaka_producer::game::{Component, Game, Lack, On, Run, Start};

/// The finger rig component for game `G`: on with finger tracking.
pub const fn fingers<G: Game>() -> Component<G> {
    Component { name: "fingers", features: &[Feature::FingerRig], options: &[FINGER_TRACKING, BINARY_FINGERS], on: on::<G>, install: install::<G>, ..Component::NONE }
}

fn on<G: Game>(run: &Run<'_, G>) -> On {
    On::when(run.shared.fingers.tracking, "finger tracking is off")
}

fn install<G: Game>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    crate::fingers::set_binary(start.shared.fingers.binary);
    Ok(())
}
