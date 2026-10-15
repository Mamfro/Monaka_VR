//! Frame generation for VR producers: AMD FSR frame generation through the FFX API from a game's
//! own loader (`monaka_ffx`) per image stream ([`stream`]), as a ready-made component
//! ([`feature`]), the HUD composer that keeps a HUD out of it ([`hud12`]), motion vectors onto each
//! eye's own previous frame ([`motion`]), each eye's inputs from the calls a game makes for its
//! upscaler ([`inputs`]: depth, motion vectors, how the frame was rendered; from Streamline's calls
//! for DLSS or FSR's dispatch), the pacing of generated and real pairs ([`pacing`]), and the game's
//! own FSR upscaling through its loader imports ([`ffx_imports`]) kept per eye ([`upscale_per_eye`]).

pub mod feature;
pub mod ffx_imports;
pub mod hud12;
pub mod inputs;
pub mod motion;
pub mod pacing;
pub mod stream;
pub mod upscale_per_eye;

pub use stream::{EyeStream, FrameInputs, SLOTS, enable, enabled, eyes_skipped, fitting_stream, free_when_idle, skip_eye};

use monaka_contract::protocol::HeadPose;
use monaka_ffx as ffx;
use monaka_producer::log;

/// Loads AMD's FFX loader shipped with the game (the game folder is searched) and its API.
pub fn load() -> Option<ffx::Api> {
    ffx::Api::load(None, &[])
        .map_err(|e| {
            if let ffx::LoadError::Os(..) = e {
                log!("{e}");
            }
        })
        .ok()
}

/// The camera a frame was rendered with, from the DLSS constants the game gave Streamline. The
/// motion-vector scale normalises the vectors to the screen; [`ffx::Camera::for_render`] applies
/// the render size.
pub fn camera(constants: &monaka_streamline::Constants) -> ffx::Camera {
    use monaka_streamline::constants as at;
    let [jx, jy] = constants.vec2(at::JITTER);
    let [mx, my] = constants.vec2(at::MVEC_SCALE);
    ffx::Camera {
        render_size: Default::default(),
        jitter: ffx::Float2 { x: jx, y: jy },
        motion_vector_scale: ffx::Float2 { x: mx, y: my },
        near: constants.f32(at::NEAR),
        far: constants.f32(at::FAR),
        fov_vertical: constants.f32(at::FOV),
        position: constants.vec3(at::POSITION),
        up: constants.vec3(at::UP),
        right: constants.vec3(at::RIGHT),
        forward: constants.vec3(at::FORWARD),
    }
}

/// The head pose `t` of the way from `a` to `b` (orientation normalised-lerped, position and IPD
/// lerped): the pose a frame generated between frames rendered from `a` and `b` stands for.
pub fn blend(a: &HeadPose, b: &HeadPose, t: f32) -> HeadPose {
    let mut r = *b;
    let dot: f32 = (0..4).map(|i| a.orientation[i] * b.orientation[i]).sum();
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    for i in 0..4 {
        r.orientation[i] = a.orientation[i] * (1.0 - t) + b.orientation[i] * sign * t;
    }
    let length = r.orientation.iter().map(|q| q * q).sum::<f32>().sqrt();
    if length > 0.0 {
        r.orientation.iter_mut().for_each(|q| *q /= length);
    }
    for i in 0..3 {
        r.position[i] = a.position[i] * (1.0 - t) + b.position[i] * t;
    }
    r.ipd = a.ipd * (1.0 - t) + b.ipd * t;
    r
}

#[cfg(test)]
mod tests;
