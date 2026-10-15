//! Per-eye frame generation as a ready-made component: AMD's FFX loader (the game's own copy)
//! loaded and frame generation on for the run ([`crate::enabled`]), or off with why. How the eyes
//! pair up stays in each game, so a game lists `framegen()` after its eyes and says, by struct
//! update, when it is wanted (`on`), what it frees (`release`) and what it reports (`every`):
//! `Component { on: smooth_chosen, release: framegen::close, ..framegen() }`.

use monaka_ffx::LOADER;
use monaka_producer::game::{Component, Game, Lack, Start};
use monaka_producer::log;

/// The frame-generation component for game `G`.
pub const fn framegen<G: Game>() -> Component<G> {
    Component { name: "frame generation", install: turn_on::<G>, ..Component::NONE }
}

/// The component's install: FFX loaded and frame generation on. A game whose own install does
/// more calls it first.
pub fn turn_on<G: Game>(_: &mut Start<'_, G>) -> Result<(), Lack> {
    let api = crate::load().ok_or_else(|| Lack::Missing(format!("{LOADER} is not loadable")))?;
    crate::enable(api);
    log!("FSR frame generation per eye: on");
    Ok(())
}
