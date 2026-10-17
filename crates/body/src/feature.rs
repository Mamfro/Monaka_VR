//! The hand rig's ready-made components.
//!
//! The finger rig ([`fingers`]): finger tracking's options and its on/off, for a game whose hand rig
//! puts the fingers on with [`crate::finger_rig`]. A game lists `monaka_body::feature::fingers()`
//! after its hand rig, saying what it works on top of (and its own help line) by struct update:
//! `Component { needs: &[Need::On(&ARMS)], ..fingers() }`. Whether a frame's fingers follow tracking is then
//! `game::on` of its place.
//!
//! Room-scale following ([`room_scale`]): the character follows the player's real steps
//! ([`crate::roomscale`]) through the game's own step hook ([`Steps`]), listed the same way.

use crate::roomscale;
use monaka_core::feature::Feature;
use monaka_contract::manifest::{BINARY_FINGERS, FINGER_TRACKING, Manifest, ROOM_SCALE};
use monaka_core::options::{Options, Shared};
use monaka_producer::game::{Component, Game, Lack, Need, On, OrMissing, Run, Start};

/// The finger rig component for game `G`: on with finger tracking, reading the controllers' finger
/// block ([`crate::controllers`], opened here unless the hand rig has).
pub const fn fingers<G: Game>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::HandRig)],
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
/// player's character for the velocity to follow with and adds it to the game's own move, in one
/// of two ways. Measured at the next step ([`roomscale::begin_step`]): following asks why it waits
/// ([`Steps::waiting`]) and where the body is ([`Steps::position`]), and the hook says the game's
/// own velocity ([`roomscale::game_velocity`]). Measured inside the step: the hook asks
/// [`roomscale::follow`] with why it waits and says what the step moved beyond the game's own move
/// ([`roomscale::credit`]).
pub trait Steps: Game {
    /// Hooks the player's movement step.
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack>;
    /// Why following waits on the step of `body` (the step hook's handle on the stepping body), if
    /// the game knows a reason. A game that gives none follows whenever the player is not climbing.
    fn waiting(_body: usize) -> Option<roomscale::Wait> {
        None
    }
    /// Where `body` is as its step begins (world, metres; `None` when unreadable), for a step
    /// measured at the next one. A hook that measures its step itself leaves it.
    fn position(_body: usize) -> Option<[f32; 3]> {
        None
    }
    /// Puts back what the step hook changed in the game, the hooks still in.
    fn restore() {}
    /// The step hook's counts, for the stop's log (empty: nothing to say).
    fn summary() -> String {
        String::new()
    }
}

/// Room-scale following for game `G` (`room_scale`, with the hand rig and a tracked head): its
/// step hook ([`Steps`]) under [`roomscale`]'s following, which stops before the hook's
/// give-back.
pub const fn room_scale<G: Steps>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::HandRig)],
        name: "room scale",
        features: &[Feature::RoomScale],
        options: &[ROOM_SCALE],
        settings: read_room_scale,
        on: room_scale_on::<G>,
        install: install_room_scale::<G>,
        quiet: roomscale::stop,
        give_back: <G as Steps>::restore,
        summary: Some(<G as Steps>::summary),
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
