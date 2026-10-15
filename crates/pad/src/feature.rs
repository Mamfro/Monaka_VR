//! The gamepad feature as a ready-made component: the VR controllers as the game's XInput pad 0
//! (merged with a real pad), the right stick turning only while the head alone pitches the view
//! (`stick_pitch_speed` above 0 leaves the stick's pitch to the game). A game lists
//! `monaka_pad::feature::gamepad()` among its components; what its own pad reading needs beyond
//! XInput (a GameInput stand-in reported to the game, a "pad connected" flag the game keeps) is a
//! small component of its own that `needs` this one.
//!
//! The actions provider ([`actions`]): a game's actions pressed by what they mean
//! ([`crate::actions`]), listed by a game that implements [`Actions`].
//!
//! The VR button layout ([`remap`]): some of the controllers' buttons reaching the game as others
//! ([`crate::remap`]), listed by a game that implements [`Remaps`].

use crate::actions::Actions;
use crate::remap::Remaps;
use monaka_core::feature::Feature;
use monaka_contract::manifest::{CONTROLLER_PAD, CONTROLLER_PROXY};
use monaka_producer::game::{Component, Game, Lack, On, Start};
use std::sync::atomic::{AtomicBool, Ordering::*};

/// The gamepad component for game `G`, offering the pad and the controller proxy. A game with
/// options of its own lists these two among them.
pub const fn gamepad<G: Game>() -> Component<G> {
    Component {
        name: "gamepad",
        features: &[Feature::Gamepad],
        options: &[CONTROLLER_PAD, CONTROLLER_PROXY],
        needs_channel: true,
        on: on::<G>,
        install: install::<G>,
        arm,
        quiet,
        release,
        ..Component::NONE
    }
}

/// Whether the head alone pitches the view: the right stick's pitch is taken from the game.
static PITCH_FROM_HEAD: AtomicBool = AtomicBool::new(false);

/// Whether the head alone pitches the view while the pad runs (for a game's look, which then keeps
/// the player's own pitch level).
pub fn pitch_from_head() -> bool {
    PITCH_FROM_HEAD.load(Acquire)
}

fn on<G: Game>(run: &monaka_producer::game::Run<'_, G>) -> On {
    On::when(run.shared.controller_pad, "the controllers are not the game's pad")
}

fn install<G: Game>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    if !crate::install_optional(start.hooks, start.channel()?) {
        return Err(Lack::Missing("the game reads no XInput pad the controllers can stand in for".to_owned()));
    }
    PITCH_FROM_HEAD.store(start.shared.stick_speed[1] <= 0.0, Release);
    Ok(())
}

fn arm() {
    crate::take_right_stick_y(pitch_from_head());
    crate::announce();
}

/// The start for a game whose own look decides what the right stick's pitch does (it steers the
/// view toward targets, or lets some pitch through toward level): the stick left whole, by struct
/// update (`Component { arm: monaka_pad::feature::whole_stick, ..gamepad() }`).
pub fn whole_stick() {
    crate::announce();
}

fn quiet() {
    crate::stop();
    crate::take_right_stick_y(false);
}

/// The actions provider for game `G` ([`crate::actions`]): from its install, ready-mades press the
/// game's actions by its bindings ([`Actions`]); at the stop every action still held is let go. A
/// game lists it beside its gamepad.
pub const fn actions<G: Actions>() -> Component<G> {
    Component { name: "actions", install: install_actions::<G>, quiet: crate::actions::let_go_all, release: crate::actions::release, ..Component::NONE }
}

fn install_actions<G: Actions>(_: &mut Start<'_, G>) -> Result<(), Lack> {
    crate::actions::install::<G>();
    Ok(())
}

/// The VR button layout for game `G` ([`crate::remap`]): from its arm the controllers' buttons
/// reach the game as its [`Remaps`] say, at its stop as they are. A game lists it after its
/// gamepad, with the switch that turns it off.
pub const fn remap<G: Remaps>() -> Component<G> {
    Component { name: "vr buttons", arm: crate::remap::install::<G>, quiet: crate::remap::release, ..Component::NONE }
}

fn release() {
    crate::report();
    crate::release();
}
