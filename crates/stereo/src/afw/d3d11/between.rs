//! AFW with frame generation on D3D11 ([`super::Afw11::frame_generation`]): in-between pairs added,
//! as `crate::afw::d3d12` adds them on D3D12. Each eye's images, real and warped in turn, are one
//! stream of frames with their depth (the real eye's own, the warped eye's from the warp) and the
//! cameras they were seen with; at each warped pair FSR frame generation (`monaka_fsr11::FrameGen11`,
//! on a D3D12 device of its own) makes each eye's frame halfway between the last pair and this one.
//! The generated pair goes out at once with the head pose halfway between the two pairs', and the
//! real pair waits half a frame ([`Waiting`]), so the headset gets twice the pair rate, evenly
//! spaced, for about one frame of latency. The waiting pair goes out at its time (the caller asks
//! [`super::Afw11::due`]) or before anything the next present publishes.
//!
//! The motion vectors are the camera's alone (from each eye's depth and its two cameras): what moves
//! in the world (the hands, enemies) is left to frame generation's own optical flow. The HUD, drawn
//! into the frame, goes through the interpolation.

use crate::afw::{Previous, Presents, Rendered, Shot, halfway};
use crate::upscaler::fsr11::fov_vertical;
use monaka_contract::protocol::HeadPose;
use monaka_framegen::pacing::Interval;
use monaka_fsr11::{Error, FrameGen11, GenFrame, GenSettings};
use monaka_producer::log;
use monaka_warp::EyeView;
use std::path::Path;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

/// Frame generation's settings for an AFW run.
#[derive(Clone, Copy, Debug)]
pub struct FrameGeneration {
    /// Where AMD's DLLs are (the loader and the frame generation provider).
    pub dlls: &'static Path,
    /// 1 or -1: whether a jittered depth's shift is told as the content shift or its opposite
    /// (the upscaler's `jitter_sign`; no matter without jitter).
    pub jitter_sign: f32,
    /// The depth convention (`monaka_ffx::create` flags: inverted, infinite).
    pub depth_flags: u32,
    /// The game's world units in a metre (a camera jump of [`CUT_METRES`] between pairs is a cut).
    pub units_per_metre: f32,
}

impl FrameGeneration {
    /// [`FrameGeneration::depth_flags`] for a standard depth (0 near, 1 far) with an infinite far
    /// plane.
    pub const STANDARD_INFINITE: u32 = monaka_ffx::create::DEPTH_INFINITE;
    /// For a reversed depth (1 near, 0 far) with an infinite far plane.
    pub const REVERSED_INFINITE: u32 = monaka_ffx::create::DEPTH_INVERTED | monaka_ffx::create::DEPTH_INFINITE;
}

/// A camera that moved this far (metres) between two pairs went through a cut: the histories go.
const CUT_METRES: f32 = 2.0;
/// The frame time given with a stream's first frame (ms).
const FIRST_FRAME_MS: f32 = 16.7;

/// One warped pair as frame generation takes it: its shot, the present it goes out at, and per eye
/// (left, right) its image (the display size, RGBA8), its depth and the depth's jitter (its pixels).
pub(super) struct PairIn<'a> {
    pub shot: &'a Shot,
    pub present: u64,
    pub images: [&'a ID3D11Texture2D; 2],
    pub depths: [&'a ID3D11Texture2D; 2],
    pub jitters: [[f32; 2]; 2],
}

/// What a pair made: each eye's generated frame, the pose halfway between the last pair's and this
/// one's (as drawn), and this pair's images as frame generation kept them (to go out later).
pub(super) struct Made {
    pub generated: [ID3D11Texture2D; 2],
    pub halfway: HeadPose,
    pub kept: [ID3D11Texture2D; 2],
}

/// The real pair waiting for its time.
pub(super) struct Waiting {
    pub at: Instant,
    pub images: [ID3D11Texture2D; 2],
    /// The pose as it goes out (reported).
    pub pose: HeadPose,
    /// Each eye's depth for the headset, when it goes there.
    pub depth: Option<super::PairDepth>,
}

pub(super) struct Between {
    settings: FrameGeneration,
    generator: Option<FrameGen11>,
    /// Frame generation could not be made: the pairs go out as AFW alone makes them.
    broken: bool,
    /// How each eye was seen in the last pair given (each stream's previous image).
    before: Option<[Rendered; 2]>,
    last: Option<Instant>,
    pub interval: Interval,
    pub waiting: Option<Waiting>,
    /// Eyes not generated (a history starting, the GPU behind, a failure).
    pub skipped: u64,
    failures: u64,
}

impl Between {
    pub fn new(settings: FrameGeneration) -> Self {
        Self { settings, generator: None, broken: false, before: None, last: None, interval: Interval::new(Duration::from_millis(16)), waiting: None, skipped: 0, failures: 0 }
    }

    /// Frame generation may still run (it was not found unavailable).
    pub fn on(&self) -> bool {
        !self.broken
    }

    /// Frame generation is off for the run (`why`, logged): the pairs go out as AFW alone makes them.
    pub fn give_up(&mut self, why: impl std::fmt::Display) {
        if !self.broken {
            log!("frame generation unavailable: {why}; AFW alone from now on");
        }
        self.broken = true;
        self.generator = None;
    }

    /// The generator for images of `display` size with depths of `render` size, made (again) when
    /// they change; none if it cannot be made (then frame generation is off for the run).
    fn generator(&mut self, device: &ID3D11Device, display: (u32, u32), render: (u32, u32)) -> Option<&mut FrameGen11> {
        if self.generator.as_ref().is_none_or(|g| g.display() != display || g.render() != render) {
            // The old one waits for the GPU in its drop.
            self.generator = None;
            let settings = GenSettings { render, display, jitter_sign: self.settings.jitter_sign, depth_flags: self.settings.depth_flags };
            match monaka_fsr11::load_with_framegen(self.settings.dlls).and_then(|api| FrameGen11::new(device, api, settings)) {
                Ok(made) => {
                    log!("FSR frame generation {}: {}x{} images with {}x{} depth, in-between pairs added", made.version(), display.0, display.1, render.0, render.1);
                    self.generator = Some(made);
                    self.before = None;
                }
                Err(e) => {
                    self.give_up(e);
                    return None;
                }
            }
        }
        self.generator.as_mut()
    }

    /// Gives `pair` to frame generation: each eye's frame halfway between the last pair and this
    /// one when the histories run on (the last pair was the present before's, no cut), else none
    /// (this pair starts them). `presents` keeps frame generation's last pair.
    pub fn pair(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, presents: &mut Presents, pair: &PairIn<'_>) -> Option<Made> {
        let shot = pair.shot;
        let views = shot.views();
        let (previous, continuous) = presents.generation(shot.eye, pair.present, false);
        let before = self.before.filter(|_| continuous && previous.is_some());
        let cut = before.is_some_and(|before| (0..2).any(|e| moved(&before[e], &views[e]) > CUT_METRES * self.settings.units_per_metre));
        let reset = before.is_none() || cut;
        let display = size(pair.images[0]);
        let render = size(pair.depths[shot.eye]);
        let now = Instant::now();
        let frame_ms = self.last.map_or(FIRST_FRAME_MS, |t| (now - t).as_secs_f32() * 1000.0);
        let generator = self.generator(device, display, render)?;
        let eyes: [GenFrame; 2] = std::array::from_fn(|e| {
            let view = EyeView { camera: views[e].camera, projection: views[e].projection };
            GenFrame {
                color: pair.images[e],
                depth: pair.depths[e],
                jitter: pair.jitters[e],
                view,
                previous: before.map_or(view, |b| EyeView { camera: b[e].camera, projection: b[e].projection }),
                // The near plane in the camera's units: nearness = a * depth + b, a = near / P[11].
                near: views[e].mapping.a * views[e].projection[11],
                fov_vertical: fov_vertical(&views[e].projection),
            }
        });
        match generator.generate(context, &eyes, frame_ms, reset) {
            Ok(usable) => {
                let made = Made { generated: generator.generated(), halfway: HeadPose::default(), kept: generator.kept() };
                self.last = Some(now);
                self.before = Some(views);
                presents.generated(Previous { pose: shot.pose, eye: shot.eye, present: pair.present });
                match previous.filter(|_| usable) {
                    Some(previous) => Some(Made { halfway: halfway(&previous.pose, &shot.pose), ..made }),
                    None => {
                        self.skipped += 2;
                        None
                    }
                }
            }
            Err(Error::Busy) => {
                self.skipped += 2;
                None
            }
            Err(e) => {
                if self.failures < 5 {
                    log!("frame generation: a pair not generated: {e}");
                }
                self.failures += 1;
                self.skipped += 2;
                None
            }
        }
    }
}

/// A texture's width and height.
fn size(texture: &ID3D11Texture2D) -> (u32, u32) {
    let desc = monaka_channel::d3d::texture_desc(texture);
    (desc.Width, desc.Height)
}

/// How far the camera moved from `a` to `b` (world units).
fn moved(a: &Rendered, b: &Rendered) -> f32 {
    let (a, b) = (&a.camera, &b.camera);
    [3, 7, 11].iter().map(|&i| (b[i] - a[i]).powi(2)).sum::<f32>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_core::depth::DepthMapping;

    fn at(x: f32) -> Rendered {
        Rendered { camera: [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], projection: [0.0; 16], mapping: DepthMapping { a: 1.0, b: 0.0 } }
    }

    #[test]
    fn a_camera_jump_is_measured_in_world_units() {
        assert_eq!(moved(&at(0.0), &at(3.0)), 3.0);
        // 50 units a metre: a 2 m cut is 100 units.
        let settings = FrameGeneration { dlls: Path::new("."), jitter_sign: 1.0, depth_flags: 0, units_per_metre: 50.0 };
        assert!(moved(&at(0.0), &at(99.0)) < CUT_METRES * settings.units_per_metre);
        assert!(moved(&at(0.0), &at(101.0)) > CUT_METRES * settings.units_per_metre);
    }
}
