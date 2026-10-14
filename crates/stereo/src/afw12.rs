//! AFW (alternate frame warping) on D3D12: the GPU work of [`crate::afw`] (which eye is real, the
//! gaps' rules, what goes out at each present: the shared core) over the fence channel. The real
//! eye's frame (its colour without the HUD, and its depth) is warped into the other eye's view at
//! the same head pose (`monaka_warp::reproject12`), and what the real eye could not see is filled
//! from that other eye's own newest real frame ([`crate::afw11`] is the same on D3D11).
//!
//! A game gives, at each present, the real eye's colour (the display size), its depth (the render
//! size, plane 0 a 32-bit float), and how it was rendered and how the other eye would be
//! ([`crate::afw::Shot`]: the camera, the projection and its depth convention, reversed or infinite
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
//! - Which eye renders: the dominant eye (Monaka's own setting) every present, the other eye every
//!   [`REFRESH`]th, so the dominant eye never alternates between a real frame and a warped one and
//!   the other eye's warp artefacts stand still instead of flickering ([`ready::dominant_eye`],
//!   the present loop's eyes chosen through `Driver::choose_eyes`); the other eye's own real frame,
//!   kept that long, fills its gaps and, where it shows the same surface as the warp, goes out in
//!   its place ([`Rules::fill_first`]). The eyes taking turns, as before, by `afw_alternate=1`.
//! - At the present loop's step ([`before_present`]): AFW alone publishes the back buffer and the
//!   warped eye as one pair. Frame generation publishes the generated pair at once and the real
//!   pair half a frame later (`monaka_framegen::pacing`), so the headset gets twice the pair rate,
//!   evenly spaced, for about one frame of latency. Menus go flat. Each pair carries both eyes'
//!   depth for the headset while it goes there ([`crate::depth_layer`]).
//!
//! Frame generation's motion vectors: each eye's stream has a previous image of its own (the real
//! eye's is the frame warped into it the present before, the warped eye's its own real frame), so
//! the game's vectors are rebased onto it per eye from the cameras (`request`): the camera's part by
//! how each eye was and is seen, the objects' part as the game measured it, against whatever frame
//! the game measured it ([`monaka_framegen::inputs::PreviousFrame`]). The warped eye's are carried
//! into its view from the real eye's pixels (`monaka_framegen::motion::MotionFix::warper`); in the
//! filled gaps, which have no honest motion, only the camera's part remains, and the in-between
//! frames may shimmer there.
//!
//! Its parts: the warp itself ([`Afw12`], here), the ready-made components (`ready`), the work at
//! the present request (`request`) and at the present loop's step (`publish`).

use crate::afw::{self, Counters, Counts, History, Out, Rendered, Rules, Shot, in_eye_order};
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
use monaka_core::camera::{Frustum, Mat44};
use monaka_core::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_framegen::ffx::{Camera, Size2};
use monaka_framegen::inputs::{self, Motion, Rendering};
use monaka_framegen::motion::{self, MotionFix};
use monaka_framegen::pacing::{Interval, Pacing};
use monaka_framegen::{EyeStream, SLOTS, fitting_stream, skip_eye as skip};
use monaka_producer::game::{Component, Every, Lack, Start};
use monaka_producer::{log, log_first};
use monaka_warp::reproject12::{Reprojector12, Source, Targets, uav_format, writable};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::*};
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
pub use ready::{REFRESH, dominant_eye, enabled, frame_generation, frame_warping, frame_warping_pairs, on_same_frame};
pub use request::{lay_hud, to_clip, warp};
use publish::*;
use ready::*;
use request::*;

/// The Beast's rules: every one ([`Rules::WHOLE`]).
pub const RULES: Rules = Rules::WHOLE;

/// One present's real eye as the game rendered it ([`Shot`] says which eye and how).
pub struct RealEye<'a> {
    /// The frame without its HUD (the display size), in `color_state`.
    pub color: &'a ID3D12Resource,
    pub color_state: D3D12_RESOURCE_STATES,
    /// Its depth (the render size, or larger with the render area at `depth_rect`; plane 0 is
    /// copied), in `depth_state`, rendered with the sub-pixel `jitter` (in its pixels).
    pub depth: &'a ID3D12Resource,
    pub depth_state: D3D12_RESOURCE_STATES,
    pub depth_rect: Option<[u32; 4]>,
    pub jitter: [f32; 2],
}

/// Whether the pairs' images of frames in `format` have that format themselves (a pass made for
/// the frames, such as a HUD lay-over, can go over them too); otherwise they are its UNORM or float
/// member.
pub fn keeps_format(format: DXGI_FORMAT) -> bool {
    uav_format(format) == Some(format)
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
    /// The part of each depth that lines up with its image (`None`: all of it).
    pub depth_rect: Option<[u32; 4]>,
    /// How each eye's newest real frame before this one was seen (left, right): the real eye's own
    /// frame before, and the other eye's newest, the frame the game had just before this one.
    pub previous: [Option<Rendered>; 2],
    /// The warped eye's gaps came from its own last real frame (not only from neighbours).
    pub filled: bool,
}

/// An eye's newest real frame: copies of its colour and depth, the depth's area and jitter (how it
/// was rendered and when it came are the core's [`History`]).
#[derive(Default)]
struct Kept {
    color: Option<ID3D12Resource>,
    depth: Option<ID3D12Resource>,
    rect: Option<[u32; 4]>,
    jitter: [f32; 2],
}

pub struct Afw12 {
    device: ID3D12Device,
    recorder: Recorder,
    reprojector: Reprojector12,
    kept: [Kept; 2],
    history: History,
    /// The warped eye's images and depths, a ring of `slots`.
    warped: Vec<(Option<ID3D12Resource>, Option<ID3D12Resource>)>,
    slot: usize,
    last: Option<Pair>,
    /// The frame and depth sizes and the frame format the textures were made for.
    shape: Option<(u32, u32, u32, u32, DXGI_FORMAT)>,
    counts: Counts,
}

// SAFETY: D3D12 objects are free-threaded; the warp is used under its owner's lock.
unsafe impl Send for Afw12 {}

impl Afw12 {
    /// The warp on `device` by `rules`, keeping `slots` pairs' warped eyes in a ring (at least one):
    /// one when each pair is published before the next present, more when something reads a pair
    /// later (frame generation reads the previous pair's images at the next present).
    pub fn new(device: &ID3D12Device, slots: usize, rules: Rules) -> Result<Self, String> {
        let mut reprojector = Reprojector12::new(device).map_err(|e| e.to_string())?;
        reprojector.fill_first = rules.fill_first;
        Ok(Self {
            device: device.clone(),
            recorder: Recorder::new(device, 4).map_err(|e| format!("command lists: {e}"))?,
            reprojector,
            kept: Default::default(),
            history: History::new(rules),
            warped: vec![(None, None); slots.max(1)],
            slot: 0,
            last: None,
            shape: None,
            counts: Counts::default(),
        })
    }

    /// Keeps `real` (the frame of `shot`) as its eye's newest real frame and warps it into the other
    /// eye, the gaps filled from that eye's own newest real frame, on `queue` (after what the game
    /// submitted there). `Ok(None)` when no command list is free (it never waits).
    pub fn warp(&mut self, queue: &ID3D12CommandQueue, shot: &Shot, real: &RealEye) -> Result<Option<Pair>, String> {
        let result = self.warp_inner(queue, shot, real);
        match &result {
            Ok(Some(_)) => self.counts.pairs += 1,
            Ok(None) => self.counts.busy += 1,
            Err(_) => self.counts.failures += 1,
        }
        result
    }

    fn warp_inner(&mut self, queue: &ID3D12CommandQueue, shot: &Shot, real: &RealEye) -> Result<Option<Pair>, String> {
        let (eye, other) = (shot.eye, shot.other_eye());
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
            self.kept = Default::default();
            self.history.forget();
            self.warped.iter_mut().for_each(|w| *w = (None, None));
            self.last = None;
            self.shape = Some(shape);
        }
        let Some(list) = self.recorder.begin().map_err(|e| e.to_string())? else { return Ok(None) };
        self.history.begin();
        let (width, height, depth_width, depth_height, _) = shape;
        let none = D3D12_RESOURCE_FLAG_NONE;
        let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        let device = &self.device;
        let kept = &mut self.kept[eye];
        let color = d3d12::fitting(&mut kept.color, device, width as u64, height, format, none).ok_or("no frame copy")?;
        let kept_depth = d3d12::fitting(&mut kept.depth, device, depth_width as u64, depth_height, DXGI_FORMAT_R32_TYPELESS, none).ok_or("no depth copy")?;
        (kept.rect, kept.jitter) = (real.depth_rect, real.jitter);
        let previous = [self.history.kept(0), self.history.kept(1)];
        self.history.keep(shot);
        d3d12::copy_whole(&list, &color, real.color, real.color_state);
        d3d12::copy_plane0(&list, &kept_depth, real.depth, real.depth_state);
        let slot = &mut self.warped[self.slot];
        let warped = d3d12::fitting(&mut slot.0, device, width as u64, height, format, uav).ok_or("no warped image")?;
        let warped_depth = d3d12::fitting(&mut slot.1, device, depth_width as u64, depth_height, DXGI_FORMAT_R32_TYPELESS, uav).ok_or("no warped depth")?;
        self.slot = (self.slot + 1) % self.warped.len();
        let warp = self.history.warp(shot);
        let view = warp.target.view(width, height);
        let primary = Source { color: &color, depth: &kept_depth, depth_rect: real.depth_rect, jitter: real.jitter, projection: warp.primary.projection, camera: warp.primary.camera };
        let fill = match (warp.fill, &self.kept[other]) {
            (Some(rendered), Kept { color: Some(color), depth: Some(depth), rect, jitter }) => {
                Some(Source { color, depth, depth_rect: *rect, jitter: *jitter, projection: rendered.projection, camera: rendered.camera })
            }
            _ => None,
        };
        let filled = fill.is_some();
        let targets = Targets { color: &warped, depth: Some(&warped_depth), depth_rect: real.depth_rect };
        self.reprojector.record(&list, &view, &primary, fill.as_ref(), &targets).map_err(|e| e.to_string())?;
        self.recorder.submit(queue).map_err(|e| e.to_string())?;
        if filled {
            self.counts.filled += 1;
        }
        let pair = Pair {
            real_eye: eye,
            images: in_eye_order(eye, color, warped),
            depths: in_eye_order(eye, kept_depth, warped_depth),
            views: shot.views(),
            depth_rect: real.depth_rect,
            previous,
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
        self.history.forget();
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
