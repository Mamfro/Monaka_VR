//! The HUD as a layer in front of the eyes, as the base every method's HUD component builds on: it
//! provides the HUD layer ([`Feature::HudLayer`]), offers the HUD's size and distance
//! (`hud_scale`, `hud_distance`) and is on while the HUD is placed in front of the eyes (`hud=0`:
//! as the game draws it). A method's HUD lists `Component { install, .., ..hud_layer() }`; a game
//! with a size of its own offers `options: &hud_layer::options(0.55)`.
//!
//! The lay-over of a D3D12 game's HUD layer at its present request is [`present12`], on the game's
//! `crate::providers::HudLayer12`.

pub mod d3d11;
pub mod d3d12;
pub mod present12;

use monaka_core::feature::Feature;
use monaka_core::manifest::{HUD_DISTANCE, HUD_SIZE, Opt};
use monaka_producer::game::{Component, Game, On, Run};

/// The HUD's size (`size` the game's default, 1 its size on a monitor) and distance options.
pub const fn options(size: f64) -> [Opt; 2] {
    [HUD_SIZE.default_number(size), HUD_DISTANCE]
}

const OPTIONS: [Opt; 2] = [HUD_SIZE, HUD_DISTANCE];

/// The HUD layer's base component for game `G`, offering the size at [`HUD_SIZE`]'s default.
pub const fn hud_layer<G: Game>() -> Component<G> {
    Component { name: "hud", features: &[Feature::HudLayer], options: &OPTIONS, on: shown::<G>, ..Component::NONE }
}

fn shown<G: Game>(run: &Run<'_, G>) -> On {
    On::when(run.shared.hud.shown, "the HUD stays as the game draws it")
}

#[cfg(test)]
mod tests;
