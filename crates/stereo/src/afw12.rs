//! AFW (alternate frame warping) on D3D12: alternate-eye rendering where every present is a full
//! pair of one moment. The real eye's frame (its colour without the HUD, and its depth) is warped
//! into the other eye's view at the same head pose (`monaka_warp::reproject12`), and what the real
//! eye could not see is filled from that other eye's own newest real frame, which saw it from the
//! right place, so the gaps get rendered pixels instead of stretched ones ([`crate::afw11`] is
//! the same on D3D11). Nothing waits for a later frame: no latency is added.
//!
//! A game gives, at each present, the real eye's colour (the display size), its depth (the render
//! size, plane 0 a 32-bit float), and how it was rendered and how the other eye would be
//! ([`Rendered`]: the camera, the projection and its depth convention, reversed or infinite
//! included), and gets the pair ([`Afw12::warp`]): both eyes' images without the HUD (lay it over
//! each, then publish them as one pair, `crate::pairs12`), and both eyes' depth
//! ([`Afw12::depth`]), the warped eye's in its own view.
//!
//! The weak spots are the warp's own: the edges neither eye's frames saw (continued from a
//! neighbour), transparencies and particles (not in the depth: they land at what is behind them),
//! and a HUD left in the frame (it would land at the depth behind it; keep it out and lay it over).
//!
//! ## The ready-mades: AFW, and AFW with frame generation
//!
//! [`frame_warping`] (AFW) and [`frame_generation`] (AFW with frame generation: in-between pairs
//! added) run it on the present loop ([`crate::feature::DRIVER`]), or [`frame_warping_pairs`] on
//! the same-frame loop fed one eye per frame (`crate::same_frame`: each frame handed over at its
//! present, [`pair_present`], the pairs going out through the loop's pair sink), for a game with
//! the providers it reads ([`crate::providers`]: its frame labels, its frame inputs, its HUD
//! layer):
//!
//! - Inputs, on the render thread: each frame's depth with the camera it was rendered from and,
//!   for frame generation, its motion vectors (asked of `monaka_framegen::inputs` at install).
//! - At the present request, before the HUD goes over the frame ([`warp`], through
//!   `providers::before_hud`): the back buffer (without the HUD, which the HUD layer keeps out) is
//!   warped into the other eye. Frame generation then generates, per eye, the frame halfway between
//!   this pair and the one before. Once the HUD is laid over the back buffer, it goes over the
//!   other images too ([`lay_hud`], through `providers::lay_hud_beside`), each eye with its own
//!   place and disparity.
//! - At the present loop's step ([`before_present`]): AFW alone publishes the back buffer and the
//!   warped eye as one pair. Frame generation publishes the generated pair at once and the real
//!   pair half a frame later (`monaka_framegen::pacing`), so the headset gets twice the pair rate,
//!   evenly spaced, for about one frame of latency. Menus go flat. Each pair carries both eyes'
//!   depth for the headset while it goes there ([`crate::depth_layer`]).
//!
//! Frame generation's weak spot: frame generation's inputs for the warped eye are approximations.
//! Its depth is the warped depth, but its motion vectors are the real eye's (in the real eye's
//! pixel grid), carried onto each eye's previous image by the eyes' parallax ([`parallax`]: the
//! game's vectors point at the other eye's frame). Near edges, where the eyes' images differ most,
//! and in the filled gaps, which have no honest motion, the in-between frames may shimmer.
//!
//! Its parts: the warp itself ([`Afw12`], here), the ready-made components (`ready`), the work at
//! the present request (`request`) and at the present loop's step (`publish`).

use crate::driver::{Driver, Pending};
use crate::eye_site::EyeSite;
use crate::feature::{DRIVER, Presents};
use crate::hudover12;
use crate::pairs12::{EyeDepth12, PairDepth, PairPublisher12, Step};
use crate::providers::{FrameInputs, HudLayer};
use crate::same_frame::PresentsPairs;
use monaka_channel::ChannelName;
use monaka_channel::pose::HeadSource;
use monaka_channel::d3d12::{self, Recorder, barrier};
use monaka_core::camera::{Frustum, Mat34, Mat44, offset_right, plausible_camera};
use monaka_core::depth::{DepthMapping, depth_params, invert, multiply};
use monaka_core::math::Vec3;
use monaka_core::protocol::{DepthParams, HeadPose};
use monaka_core::sync::Locked;
use monaka_framegen::ffx::{Camera, Size2};
use monaka_framegen::inputs::{self, Motion, Rendering};
use monaka_framegen::motion::{self, MotionFix};
use monaka_framegen::pacing::{Interval, Pacing};
use monaka_framegen::{EyeStream, SLOTS, fitting_stream, skip_eye as skip};
use monaka_producer::game::{Component, Every, Lack, Start};
use monaka_producer::{log, log_first};
use monaka_warp::View;
use monaka_warp::reproject12::{Reprojector12, Source, Targets, uav_format, writable};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32_TYPELESS};
use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, IDXGISwapChain3};
use windows::core::Interface;

mod publish;
mod ready;
mod request;
#[cfg(test)]
mod tests;

pub use publish::{close, pair_present, report};
pub use ready::{enabled, frame_generation, frame_warping, frame_warping_pairs, on_same_frame};
pub use request::{lay_hud, parallax, warp};
use publish::*;
use ready::*;
use request::*;

/// How a frame was (or would be) rendered: its canonical camera-to-world and projection, and how
/// its depth values map to nearness (`near / distance`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rendered {
    pub camera: Mat34,
    pub projection: Mat44,
    pub mapping: DepthMapping,
}

impl Rendered {
    /// A camera and canonical projection with its `near` distance (the depth convention is read
    /// from the projection: standard, reversed, infinite). `None` for a camera that is not rigid or
    /// a projection that is not a perspective one.
    pub fn new(camera: Mat34, projection: Mat44, near: f32) -> Option<Self> {
        let mapping = DepthMapping::from_projection(&projection, near)?;
        plausible_camera(&camera).then_some(Self { camera, projection, mapping })
    }

    /// A camera given as its position and axes (view space looks down -z: `forward` is -z) with a
    /// view-to-clip matrix in row-vector form (Streamline's, a DirectX game's), as `new` takes them.
    pub fn from_row_vectors(position: Vec3, right: Vec3, up: Vec3, forward: Vec3, view_to_clip: &Mat44, near: f32) -> Option<Self> {
        let back = forward.map(|v| -v);
        let camera = [right[0], up[0], back[0], position[0], right[1], up[1], back[1], position[1], right[2], up[2], back[2], position[2]];
        let projection = std::array::from_fn(|i| view_to_clip[(i % 4) * 4 + i / 4]);
        Self::new(camera, projection, near)
    }

    /// How a frame was rendered, from its frame inputs (the game's camera and its view to clip, from
    /// whichever of its upscaler's calls told them).
    pub fn from_rendering(rendering: &Rendering) -> Option<Self> {
        let camera = &rendering.camera;
        Self::from_row_vectors(camera.position, camera.right, camera.up, camera.forward, &rendering.view_to_clip, camera.near)
    }

    /// How its depth maps to distance in metres, for the headset (`units_per_metre`: the game's
    /// world units in a metre).
    pub fn depth_params(&self, units_per_metre: f32) -> Option<DepthParams> {
        depth_params(&self.projection, units_per_metre)
    }

    /// The other eye at the same moment, for this one being eye `eye` (0 left): the camera moved
    /// `separation` along its right axis toward it (the eyes are parallel), and its projection with
    /// the other eye's `frustum` when it has one of its own (its depth terms kept).
    pub fn other_eye(&self, eye: usize, separation: f32, frustum: Option<Frustum>) -> Self {
        let camera = offset_right(&self.camera, if eye == 0 { separation } else { -separation });
        let mut projection = self.projection;
        if let Some(frustum) = frustum {
            frustum.apply_to_projection(&mut projection);
        }
        Self { camera, projection, mapping: self.mapping }
    }
}

/// One present's real eye, as the game rendered it.
pub struct RealEye<'a> {
    /// 0 left, 1 right.
    pub eye: usize,
    /// The frame without its HUD (the display size), in `color_state`.
    pub color: &'a ID3D12Resource,
    pub color_state: D3D12_RESOURCE_STATES,
    /// Its depth (the render size; plane 0 is copied), in `depth_state`.
    pub depth: &'a ID3D12Resource,
    pub depth_state: D3D12_RESOURCE_STATES,
    pub rendered: Rendered,
    /// The other eye at the same moment ([`Rendered::other_eye`]).
    pub other: Rendered,
}

/// Whether the pairs' images of frames in `format` have that format themselves (a pass made for
/// the frames, such as a HUD lay-over, can go over them too); otherwise they are its UNORM or float
/// member.
pub fn keeps_format(format: DXGI_FORMAT) -> bool {
    uav_format(format) == Some(format)
}

/// `this` (eye `eye`'s, 0 left) and `other` (the other eye's) as (left, right).
pub fn in_eye_order<T>(eye: usize, this: T, other: T) -> [T; 2] {
    if eye == 0 { [this, other] } else { [other, this] }
}

/// One present's pair, both eyes of one moment.
#[derive(Clone)]
pub struct Pair {
    pub real_eye: usize,
    /// Both eyes' images (left, right) without the HUD, in COMMON, the frame's size: the real
    /// eye's copy and the warped eye.
    pub images: [ID3D12Resource; 2],
    /// Both eyes' depth (left, right), R32 (typeless, read as float), the render size, in COMMON,
    /// each in its eye's own view and its projection's convention.
    pub depths: [ID3D12Resource; 2],
    /// How each eye is seen (left, right): the real eye's, and the view it was warped into.
    pub views: [Rendered; 2],
    /// The warped eye's gaps came from its own last real frame (not only from neighbours).
    pub filled: bool,
}

/// An eye's newest real frame: copies of its colour and depth, how it was rendered, and the
/// present it came at.
struct Kept {
    color: Option<ID3D12Resource>,
    depth: Option<ID3D12Resource>,
    rendered: Option<Rendered>,
    at: u64,
}

/// A frame of the other eye older than this many presents does not fill gaps (it is from before a
/// pause or a cut).
const FILL_AGE: u64 = 4;

/// Counts for the summary.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub pairs: u64,
    /// Pairs whose gaps were filled from the warped eye's own last real frame.
    pub filled: u64,
    /// Presents with no command list free (the GPU behind).
    pub busy: u64,
    pub failures: u64,
}

pub struct Afw12 {
    device: ID3D12Device,
    recorder: Recorder,
    reprojector: Reprojector12,
    kept: [Kept; 2],
    /// The warped eye's images and depths, a ring of `slots`.
    warped: Vec<(Option<ID3D12Resource>, Option<ID3D12Resource>)>,
    slot: usize,
    presents: u64,
    last: Option<Pair>,
    /// The frame and depth sizes and the frame format the textures were made for.
    shape: Option<(u32, u32, u32, u32, DXGI_FORMAT)>,
    counts: Counts,
}

// SAFETY: D3D12 objects are free-threaded; the warp is used under its owner's lock.
unsafe impl Send for Afw12 {}

impl Afw12 {
    /// The warp on `device`, keeping `slots` pairs' warped eyes in a ring (at least one): one when
    /// each pair is published before the next present, more when something reads a pair later (frame
    /// generation reads the previous pair's images at the next present).
    pub fn new(device: &ID3D12Device, slots: usize) -> Result<Self, String> {
        let kept = || Kept { color: None, depth: None, rendered: None, at: 0 };
        Ok(Self {
            device: device.clone(),
            recorder: Recorder::new(device, 4).map_err(|e| format!("command lists: {e}"))?,
            reprojector: Reprojector12::new(device).map_err(|e| e.to_string())?,
            kept: [kept(), kept()],
            warped: vec![(None, None); slots.max(1)],
            slot: 0,
            presents: 0,
            last: None,
            shape: None,
            counts: Counts::default(),
        })
    }

    /// Keeps `real` as its eye's newest real frame and warps it into the other eye, the gaps filled
    /// from that eye's own newest real frame, on `queue` (after what the game submitted there).
    /// `Ok(None)` when no command list is free (it never waits).
    pub fn warp(&mut self, queue: &ID3D12CommandQueue, real: &RealEye) -> Result<Option<Pair>, String> {
        let result = self.warp_inner(queue, real);
        match &result {
            Ok(Some(_)) => self.counts.pairs += 1,
            Ok(None) => self.counts.busy += 1,
            Err(_) => self.counts.failures += 1,
        }
        result
    }

    fn warp_inner(&mut self, queue: &ID3D12CommandQueue, real: &RealEye) -> Result<Option<Pair>, String> {
        let (eye, other) = (real.eye.min(1), 1 - real.eye.min(1));
        // SAFETY: reads resource descriptors.
        let (frame, depth) = unsafe { (real.color.GetDesc(), real.depth.GetDesc()) };
        let format = uav_format(frame.Format).ok_or_else(|| format!("frame format {} not handled", frame.Format.0))?;
        let shape = (frame.Width as u32, frame.Height, depth.Width as u32, depth.Height, format);
        if self.shape != Some(shape) {
            if !writable(&self.device, format) {
                return Err(format!("frame format {} cannot be written by a shader here", format.0));
            }
            // New sizes: the old textures go once the GPU is done with them.
            self.recorder.idle(1000);
            self.kept.iter_mut().for_each(|k| (k.color, k.depth, k.rendered) = (None, None, None));
            self.warped.iter_mut().for_each(|w| *w = (None, None));
            self.last = None;
            self.shape = Some(shape);
        }
        let Some(list) = self.recorder.begin().map_err(|e| e.to_string())? else { return Ok(None) };
        self.presents += 1;
        let (width, height, depth_width, depth_height, _) = shape;
        let none = D3D12_RESOURCE_FLAG_NONE;
        let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        let device = &self.device;
        let kept = &mut self.kept[eye];
        let color = d3d12::fitting(&mut kept.color, device, width as u64, height, format, none).ok_or("no frame copy")?;
        let kept_depth = d3d12::fitting(&mut kept.depth, device, depth_width as u64, depth_height, DXGI_FORMAT_R32_TYPELESS, none).ok_or("no depth copy")?;
        (kept.rendered, kept.at) = (Some(real.rendered), self.presents);
        d3d12::copy_whole(&list, &color, real.color, real.color_state);
        d3d12::copy_plane0(&list, &kept_depth, real.depth, real.depth_state);
        let slot = &mut self.warped[self.slot];
        let warped = d3d12::fitting(&mut slot.0, device, width as u64, height, format, uav).ok_or("no warped image")?;
        let warped_depth = d3d12::fitting(&mut slot.1, device, depth_width as u64, depth_height, DXGI_FORMAT_R32_TYPELESS, uav).ok_or("no warped depth")?;
        self.slot = (self.slot + 1) % self.warped.len();
        let view = View { projection: real.other.projection, camera: real.other.camera, mapping: real.other.mapping, width, height };
        let primary = Source { color: &color, depth: &kept_depth, projection: real.rendered.projection, camera: real.rendered.camera };
        let fill = match &self.kept[other] {
            Kept { color: Some(color), depth: Some(depth), rendered: Some(rendered), at } if self.presents - at <= FILL_AGE => {
                Some(Source { color, depth, projection: rendered.projection, camera: rendered.camera })
            }
            _ => None,
        };
        let filled = fill.is_some();
        self.reprojector.record(&list, &view, &primary, fill.as_ref(), &Targets { color: &warped, depth: Some(&warped_depth) }).map_err(|e| e.to_string())?;
        self.recorder.submit(queue).map_err(|e| e.to_string())?;
        if filled {
            self.counts.filled += 1;
        }
        let pair = Pair {
            real_eye: eye,
            images: in_eye_order(eye, color, warped),
            depths: in_eye_order(eye, kept_depth, warped_depth),
            views: in_eye_order(eye, real.rendered, real.other),
            filled,
        };
        self.last = Some(pair.clone());
        Ok(Some(pair))
    }

    /// The newest pair's depth of eye `eye` (R32 typeless, the render size, in COMMON), in that
    /// eye's own view: the real eye's copy, or the warped eye's. Valid until the next warp
    /// (the warped eye's for as long as its ring slot is not reused).
    pub fn depth(&self, eye: usize) -> Option<(&ID3D12Resource, &Rendered)> {
        let pair = self.last.as_ref()?;
        let eye = eye.min(1);
        Some((&pair.depths[eye], &pair.views[eye]))
    }

    /// The newest pair.
    pub fn last(&self) -> Option<&Pair> {
        self.last.as_ref()
    }

    /// Forgets both eyes' frames (a menu, a cut): the next warp fills its gaps from neighbours only.
    pub fn forget(&mut self) {
        self.kept.iter_mut().for_each(|k| k.rendered = None);
        self.last = None;
    }

    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// Waits (at most `timeout_ms`) for the GPU before the textures may be released.
    pub fn idle(&self, timeout_ms: u32) -> bool {
        self.recorder.idle(timeout_ms)
    }
}
