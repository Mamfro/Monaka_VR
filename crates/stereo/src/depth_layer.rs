//! Each eye's depth to the headset, as a ready-made component ([`depth_layer`]): the pairs carry
//! each eye's depth (`crate::pairs12::PairPublisher12::publish_with_depth`), and the viewer gives it
//! to the headset's runtime as a depth layer, so its own reprojection of a late frame corrects the
//! head moving as well as turning.
//!
//! The component brings the feature, the option ([`HEADSET_DEPTH`], on by default, off for an A/B
//! check against colour only) and its switch, and asks the game's frame inputs
//! ([`crate::providers::FrameInputs`]) for each eye's depth and camera. The present loop takes the
//! depth of each frame it publishes itself ([`frame_depth`]); a game publishing its own pairs takes
//! both eyes' ([`pair_depth`]); frame warping and per-eye frame generation send their own. Menus
//! and other flat frames go without.

use crate::afw::Rendered;
use crate::alternate12::FrameDepth;
use crate::pairs12::{EyeDepth12, PairDepth};
use crate::providers::FrameInputs;
use monaka_core::feature::Feature;
pub use monaka_core::manifest::HEADSET_DEPTH;
use monaka_core::protocol::DepthParams;
use monaka_framegen::inputs::{self, EyeInputs, Motion};
use monaka_producer::game::{Component, Lack, Start, Switch};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::*};
use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_COMMON;

static ON: AtomicBool = AtomicBool::new(false);
/// The game's world units in a metre (its bits).
static UNITS_PER_METRE: AtomicU32 = AtomicU32::new(0);

/// The depth component for game `G`: the feature, the option, the switch. The game adds what it
/// works on top of (`needs`) by struct update.
pub const fn depth_layer<G: FrameInputs>() -> Component<G> {
    Component {
        name: "depth to headset",
        features: &[Feature::DepthLayer],
        options: &[HEADSET_DEPTH],
        switch: Some(Switch::new("headset_depth", true, "the headset gets colour only")),
        install: install::<G>,
        release,
        ..Component::NONE
    }
}

fn install<G: FrameInputs>(_: &mut Start<'_, G>) -> Result<(), Lack> {
    UNITS_PER_METRE.store(G::UNITS_PER_METRE.to_bits(), Release);
    inputs::want(Motion::None, None);
    ON.store(true, Release);
    Ok(())
}

fn release() {
    ON.store(false, Release);
}

/// Whether the pairs carry each eye's depth this run.
pub fn on() -> bool {
    ON.load(Acquire)
}

fn units_per_metre() -> f32 {
    f32::from_bits(UNITS_PER_METRE.load(Acquire))
}

/// How the depth of a frame rendered with `inputs`' camera maps to distance in metres.
pub fn params(inputs: &EyeInputs) -> Option<DepthParams> {
    Rendered::from_rendering(inputs.rendering.as_ref()?)?.depth_params(units_per_metre())
}

/// Eye `eye`'s depth as it goes with its image: its copy (in COMMON, written again at that eye's
/// next frame), the part that lines up with the image, and how it maps to distance.
fn eye_depth(inputs: &EyeInputs) -> Option<(EyeDepth12, DepthParams)> {
    Some((EyeDepth12 { texture: inputs.depth.clone()?, state: D3D12_RESOURCE_STATE_COMMON, rect: inputs.rect }, params(inputs)?))
}

/// The present loop, publishing eye `eye`'s frame itself: that frame's depth (taken), when it came
/// since the eye's last present and depth goes to the headset.
pub fn frame_depth(eye: usize) -> Option<FrameDepth> {
    if !on() {
        return None;
    }
    let (eye, params) = eye_depth(&inputs::take(eye.min(1)).filter(|i| i.fresh.depth)?)?;
    Some(FrameDepth { eye, params })
}

/// A game publishing its own pairs: both eyes' depth (taken), when both came since their last pair
/// and depth goes to the headset; the depth mapping is eye `eye`'s.
pub fn pair_depth(eye: usize) -> Option<PairDepth> {
    if !on() {
        return None;
    }
    let taken = [inputs::take(0), inputs::take(1)];
    let [Some(left), Some(right)] = taken.map(|i| i.filter(|i| i.fresh.depth).and_then(|i| eye_depth(&i))) else { return None };
    let params = if eye.min(1) == 0 { left.1 } else { right.1 };
    Some(PairDepth { eyes: [left.0, right.0], params })
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_framegen::inputs::Rendering;
    use monaka_streamline::{Constants, constants as at};

    #[test]
    fn the_games_projection_gives_the_near_plane_in_metres() {
        // DLSS's view to clip (row vectors): reverse-Z infinite, near 0.05, the camera looking down -z.
        let mut bytes = vec![0u8; at::BYTES];
        let mut put = |offset: usize, values: &[f32]| {
            for (i, v) in values.iter().enumerate() {
                bytes[offset + 4 * i..offset + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
            }
        };
        put(at::CAMERA_VIEW_TO_CLIP, &[1.036, 0.0, 0.0, 0.0, 0.0, 1.84, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.05, 0.0]);
        put(at::UP, &[0.0, 1.0, 0.0]);
        put(at::RIGHT, &[1.0, 0.0, 0.0]);
        put(at::FORWARD, &[0.0, 0.0, -1.0]);
        put(at::NEAR, &[0.05]);
        UNITS_PER_METRE.store(1.0f32.to_bits(), Release);
        let inputs = EyeInputs { rendering: Constants::read(bytes.as_ptr()).as_ref().map(Rendering::from_constants), ..EyeInputs::default() };
        assert_eq!(params(&inputs), Some(DepthParams::new(0.05, f32::INFINITY, true)));
        assert_eq!(params(&EyeInputs::default()), None, "no camera");
    }
}
