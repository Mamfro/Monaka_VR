//! The hand rig's ready-made components.
//!
//! The finger rig ([`fingers`]): finger tracking's options and its on/off, for a game whose hand rig
//! puts the fingers on with [`crate::finger_rig`]. A game lists `monaka_arms::feature::fingers()`
//! after its hand rig, saying what it works on top of (and its own help line) by struct update:
//! `Component { needs: &[&ARMS], ..fingers() }`. Whether a frame's fingers follow tracking is then
//! `game::on` of its place.
//!
//! Room-scale following ([`room_scale`]): the character follows the player's real steps
//! ([`crate::roomscale`]) through the game's own step hook ([`Steps`]), listed the same way.

use crate::roomscale;
use monaka_core::feature::Feature;
use monaka_core::manifest::{BINARY_FINGERS, FINGER_TRACKING, Manifest, ROOM_SCALE};
use monaka_core::options::{Options, Shared};
use monaka_producer::game::{Component, Game, Lack, On, OrMissing, Run, Start};

/// The finger rig component for game `G`: on with finger tracking, reading the controllers' finger
/// block ([`crate::controllers`], opened here unless the hand rig has).
pub const fn fingers<G: Game>() -> Component<G> {
    Component {
        name: "fingers",
        features: &[Feature::FingerRig],
        options: &[FINGER_TRACKING, BINARY_FINGERS],
        needs_channel: true,
        on: on::<G>,
        install: install::<G>,
        release: crate::controllers::close,
        ..Component::NONE
    }
}

fn on<G: Game>(run: &Run<'_, G>) -> On {
    On::when(run.shared.fingers.tracking, "finger tracking is off")
}

fn install<G: Game>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    crate::controllers::open(start.channel()?);
    crate::fingers::set_binary(start.shared.fingers.binary);
    Ok(())
}

/// A game's movement step as room-scale following needs it. The hook asks each step of the
/// player's character for the velocity to follow with ([`roomscale::follow`], with why the game
/// moves the body itself if it does), adds it to the game's own move, and says what the step moved
/// beyond the game's own move ([`roomscale::credit`]). When following waits and how a step is
/// measured are the game's.
pub trait Steps: Game {
    /// Hooks the player's movement step.
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack>;
    /// Puts back what the step hook changed in the game, the hooks still in.
    fn restore() {}
    /// Logs the step hook's counts, the hooks out.
    fn report() {}
}

/// Room-scale following for game `G` (`room_scale`, with the hand rig and a tracked head): its
/// step hook ([`Steps`]) under [`roomscale`]'s following, which stops before the hook's
/// give-back.
pub const fn room_scale<G: Steps>() -> Component<G> {
    Component {
        name: "room scale",
        features: &[Feature::RoomScale],
        options: &[ROOM_SCALE],
        settings: read_room_scale,
        on: room_scale_on::<G>,
        install: install_room_scale::<G>,
        quiet: roomscale::stop,
        give_back: <G as Steps>::restore,
        release: <G as Steps>::report,
        ..Component::NONE
    }
}

fn read_room_scale(options: &Options, manifest: &Manifest) {
    roomscale::Settings::from_options(options, &Shared::from_options(options, manifest));
}

fn room_scale_on<G: Steps>(run: &Run<'_, G>) -> On {
    On::when(roomscale::Settings::from_options(run.options, run.shared).is_some(), "room-scale movement is off")
}

fn install_room_scale<G: Steps>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let settings = roomscale::Settings::from_options(start.options, start.shared).or_missing("room-scale movement is off")?;
    <G as Steps>::install(start)?;
    roomscale::install(settings);
    Ok(())
}
