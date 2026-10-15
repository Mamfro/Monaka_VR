//! A game's own temporal upscaler (DLSS, FSR) kept per eye, as a ready-made component
//! ([`per_eye`]). With alternate eyes the game's previous frame is always the other eye's, so the
//! upscaler's history would carry one eye into the other (shimmer, smearing, the eyes blending);
//! kept per eye, each eye's frames build only on its own.
//!
//! The component brings the feature (per-eye history), the option and its switch
//! ([`UPSCALE_PER_EYE`]), and its methods, one per upscaler, so the one option covers whichever the
//! game runs. The game arms each method its renderer offers where it hooks that upscaler's calls,
//! when this component is on (`Start::is_on`), with which eye each frame is:
//!
//! - DLSS through Streamline (`monaka_streamline::per_eye`): the game's Streamline imports hooked
//!   with `monaka_streamline::imports::PerEye` (its own viewport per eye, its constants describing
//!   the eye's own previous frame, the motion vectors rebased onto it);
//! - FSR through the FidelityFX loader (`monaka_framegen::upscale_per_eye`): the game's loader
//!   imports hooked with `monaka_framegen::ffx_imports::PerEye` (a twin of the game's upscale
//!   context for the right eye, the motion vectors chained onto the eye's own previous frame).
//!
//! Two more paths, each with its own entry:
//!
//! - FSR on D3D11 ([`fsr11`], its component `fsr11::per_eye`): an alternate-eye game on the present
//!   loop rendering each eye smaller, AMD's FSR bringing it to the eye size, from its frame labels
//!   and D3D11 frame inputs (`crate::providers::FrameInputs11`);
//! - a renderer's own DLSS calls, not Streamline's ([`own_dlss`]): the game hooks those calls and its
//!   scene's camera history and says where they are, which calls write the camera, and its constants
//!   packet's layout.
//!
//! The FSR method's stop, release and summary come with the component; the game adds its
//! `install` (and `needs`, `give_back` for the DLSS method) by struct update, and adjusts the
//! option's default where it differs (`UPSCALE_PER_EYE.default_switch(false)`).
//!
//! Each method runs after the frame inputs' provider in the game's calls, so frame generation, frame
//! warping and depth to the headset get the game's own inputs whichever method runs.

pub mod fsr11;
pub mod own_dlss;

use monaka_core::feature::Feature;
use monaka_contract::manifest::{Kind, Opt, Shown, opt};
use monaka_framegen::{ffx_imports, upscale_per_eye};
use monaka_producer::game::{Component, Game, Switch};

/// Each eye keeps its own upscaler history.
pub const UPSCALE_PER_EYE: Opt =
    opt("upscale_per_eye", "Upscaler per eye", "DLSS or FSR per eye: each eye keeps its own upscaler history, so the eyes do not blend.", Kind::Switch { default: true })
        .on(Shown::Rendering);

/// The per-eye upscaler component for game `G`: the feature, the option, the switch (on unless the
/// game's manifest says otherwise), the FSR method's stop, release and summary. The game adds its
/// `install` (and `needs`, `give_back`) by struct update.
pub const fn per_eye<G: Game>() -> Component<G> {
    Component {
        name: "upscaler per eye",
        features: &[Feature::PerEyeHistory],
        options: &[UPSCALE_PER_EYE],
        switch: Some(Switch::new("upscale_per_eye", true, "the upscaler runs as the game made it, one history for both eyes")),
        // Before the window size goes back: the game's upscales go to its own context again.
        quiet: upscale_per_eye::stop,
        release: ffx_imports::release,
        summary: Some(upscale_per_eye::summary),
        ..Component::NONE
    }
}

/// Each eye's frames go to its own upscaler history now, by either method (not merely armed: the
/// game's upscaler options passed on, its twin context made, nothing broken).
pub fn running() -> bool {
    monaka_streamline::per_eye::running() || upscale_per_eye::running()
}
