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
use monaka_contract::manifest::{HUD_DISTANCE, HUD_SIZE, Opt};
use crate::providers::FrameLabels;
use monaka_contract::protocol::HeadPose;
use monaka_core::hud::{Placement, Rect};
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

/// Where the HUD goes in eye `eye` of a frame drawn from `pose`, `distance` metres away, whatever
/// the graphics API: in the eye's frustum (the game camera's own where no eye camera drew it),
/// toward the nose by half the eyes' separation seen at that distance (`G`'s frame labels). `None`
/// while the labels have no frustum or separation.
pub fn place_eye<G: FrameLabels>(pose: &HeadPose, eye: usize, distance: f32) -> Option<Placement> {
    Some(Placement::new(G::frustum(pose, eye).or_else(G::own_frustum)?, eye, G::separation(pose)?, distance))
}

/// Where the HUD goes over the frame presenting now (`size` its pixels), at `scale` and `distance`,
/// as (x, y, width, height) in pixels: in its eye's place (or eye `eye`'s, for a feature's image of
/// the frame), or centred with no disparity in a frame no eye camera drew. `None` without a frustum
/// to place it in.
pub fn place_presenting<G: FrameLabels>(size: (u32, u32), eye: Option<usize>, scale: f32, distance: f32) -> Option<[f32; 4]> {
    let placement = match G::presenting() {
        Some(label) => place_eye::<G>(&label.pose, eye.unwrap_or(label.eye), distance)?,
        None => Placement::new(G::own_frustum()?, 0, 0.0, distance),
    };
    let image = Rect::sized(size.0 as f32, size.1 as f32);
    Some(placement.rect(image, image.aspect(), scale).array())
}

#[cfg(test)]
mod tests;
