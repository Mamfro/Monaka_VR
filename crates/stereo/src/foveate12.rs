//! Foveated rendering on D3D12: the 3D scene shaded at a lower rate away from where the eyes look,
//! through variable rate shading (VRS) with a shading-rate image, as a ready-made component
//! ([`foveation`]).
//!
//! - **The image**: one texel per tile of the scene's render targets (`R8_UINT`, the tile size the
//!   device reports; VRS tier 2 is needed): full rate (1x1) within an inner angle of the gaze, 2x2
//!   in a ring around it, a coarser rate beyond ([`Rings`], [`fill`]). Each eye has its own image,
//!   rewritten when its gaze moves to another tile (or its frustum changes): written into an upload
//!   buffer and copied on the game's own command list just before the pass that uses it.
//! - **The gaze**: the one in the head pose the frame is rendered with (the game's frame labels,
//!   [`crate::providers::FrameLabels::rendering`]: `HeadPose::gaze`, head-relative), projected through the frustum
//!   that eye is rendered with ([`crate::providers::FrameLabels::frustum`]). The eye cameras keep the head's axes, so a head-relative direction
//!   lands straight on the eye's image. Without a live gaze the sharp area stays straight ahead in
//!   that eye's frustum, made wider ([`FIXED_MARGIN_DEGREES`]).
//! - **The passes**: only the 3D scene's, told by size. The game renders its scene at the render
//!   size and its upscaler takes it to the output size, where the HUD and the final passes draw
//!   (the render size in the game's frame inputs, `monaka_framegen::inputs::render_sizes`). VRS is set while a pass's render targets are the
//!   render size, and cleared otherwise: `OMSetRenderTargets` and `BeginRenderPass` give the
//!   targets (each view's size noted at the game device's `CreateRenderTargetView`; a view made
//!   before the hooks went in is sized by the pass's viewport, from `RSSetViewports`), and `Reset`
//!   starts a list afresh. The list calls come from the game's own lists ([`crate::lists12`], from
//!   its queue, [`crate::queue12::game_queue`]); where the game records on Streamline's proxy lists, VRS and the
//!   image writes go on the native list behind the proxy.
//! - None for a frame shown flat (a menu), a frame whose eye is not known, or a game rendering its
//!   scene at the output size (its scene passes cannot be told from the rest).
//!
//! Its parts: the rings and the component (here), the scene's size and each frame's eye (`frame`),
//! the hooks (`hooks`), the images (`images`) and the counts (`counts`).

use monaka_core::feature::Feature;
use crate::driver::{Label, Pending};
use crate::feature::DRIVER;
use crate::lists12::{self, Subscriber};
use crate::providers::FrameInputs;
use monaka_channel::d3d12::{self, barrier, placed, subresource};
use monaka_core::camera::Frustum;
use monaka_contract::manifest::{Kind, Opt, Shown, choice, line, opt};
use monaka_core::options::Options;
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_hook::{Hooks, InFlight, Original, mem};
use monaka_producer::game::{Component, Every, Lack, Missing, Need, OrMissing, Start, Switch};
use monaka_producer::{log, log_first};
use monaka_streamline::imports;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem::offset_of;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8_UINT, DXGI_SAMPLE_DESC};
use windows::core::{BOOL, Interface};

mod counts;
mod frame;
mod hooks;
mod images;
#[cfg(test)]
mod tests;

pub use frame::labelled_frame;
use counts::*;
use frame::*;
use hooks::*;
use images::*;

/// The switch: off unless turned on, until its hooks are proven in a game.
pub const FOVEATED_RENDERING: Opt = opt(
    "foveated_rendering",
    "Foveated rendering",
    "Draws the edges of your view in less detail, so the game runs faster. Sharp where you look with eye tracking, else straight ahead.",
    Kind::Switch { default: false },
)
.on(Shown::Rendering);

/// Full detail within this angle of the gaze (degrees).
pub const FOVEATED_INNER: Opt =
    line("foveated_inner", "Sharp area (degrees)", "Full detail within this angle of where you look.", Kind::Number { min: 2.0, max: 60.0, step: 1.0, default: 15.0 });

/// 2x2 out to this angle of the gaze (degrees); the outer rate beyond.
pub const FOVEATED_MIDDLE: Opt = line(
    "foveated_middle",
    "Middle ring (degrees)",
    "Medium detail out to this angle of where you look; the outer detail beyond it.",
    Kind::Number { min: 2.0, max: 90.0, step: 1.0, default: 30.0 },
);

/// The rate beyond the middle ring: kept moderate by default, since the game's temporal upscaler
/// runs after and a coarse edge can shimmer through it.
pub const FOVEATED_OUTER: Opt = line(
    "foveated_outer",
    "Outer detail",
    "How coarse the edges are. The game's upscaler runs after, so stronger can shimmer at the edges.",
    choice(&[("2x2", "Light"), ("2x4", "Moderate"), ("4x4", "Strong")], "2x4"),
);

/// Without a live gaze both rings are this much wider (degrees): the eyes move without the sharp
/// area following them.
pub const FIXED_MARGIN_DEGREES: f32 = 10.0;

/// A gaze sample older than this (by the time the viewer read it) is not live.
const GAZE_FRESH_MS: u64 = 500;

/// `D3D12_SHADING_RATE` values, as the image holds them.
pub mod rate {
    pub const R1X1: u8 = 0x0;
    pub const R2X2: u8 = 0x5;
    /// Two pixels across, four down: horizontal detail matters more for the eyes' disparity.
    pub const R2X4: u8 = 0x6;
    pub const R4X4: u8 = 0xa;
}

/// The rings around the gaze: full rate within `inner`, 2x2 within `middle` (radians from the
/// gaze), `outer` beyond.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rings {
    pub inner: f32,
    pub middle: f32,
    pub outer: u8,
}

impl Rings {
    /// The rings the options ask for.
    pub fn read(options: &Options) -> Rings {
        let inner = options.number("foveated_inner", 2.0, 60.0, 15.0);
        let middle = options.number("foveated_middle", 2.0, 90.0, 30.0).max(inner);
        let outer = options.choice("foveated_outer", &[("2x2", rate::R2X2), ("2x4", rate::R2X4), ("4x4", rate::R4X4)], rate::R2X4);
        Rings { inner: inner.to_radians(), middle: middle.to_radians(), outer }
    }

    /// The rings without a live gaze: both wider by [`FIXED_MARGIN_DEGREES`].
    pub fn fixed(self) -> Rings {
        let margin = FIXED_MARGIN_DEGREES.to_radians();
        Rings { inner: self.inner + margin, middle: self.middle + margin, ..self }
    }

    /// The rings on a device without the coarser rates (2x4, 4x4): 2x2 beyond the middle ring too.
    pub fn basic(self) -> Rings {
        Rings { outer: rate::R2X2, ..self }
    }

    fn rate(&self, cos_angle: f32) -> u8 {
        if cos_angle >= self.inner.cos() {
            rate::R1X1
        } else if cos_angle >= self.middle.cos() {
            rate::R2X2
        } else {
            self.outer
        }
    }
}

/// Straight ahead in head-relative (OpenXR view) space.
pub const AHEAD: [f32; 3] = [0.0, 0.0, -1.0];

/// Tiles across and down an image of `pixels` at `tile` pixels a tile.
pub fn tiles(pixels: (u32, u32), tile: u32) -> (u32, u32) {
    (pixels.0.div_ceil(tile.max(1)), pixels.1.div_ceil(tile.max(1)))
}

/// Where `direction` (head-relative: x right, y up, -z ahead) lands on an image rendered with
/// `frustum`: as fractions across and down (0 to 1 inside the image). `None` for a direction not
/// ahead.
pub fn image_point(frustum: &Frustum, direction: [f32; 3]) -> Option<[f32; 2]> {
    let ahead = -direction[2];
    if !direction.iter().all(|v| v.is_finite()) || ahead <= 1e-3 {
        return None;
    }
    let (x, y) = (direction[0] / ahead, direction[1] / ahead);
    Some([(x - frustum.left) / (frustum.right - frustum.left), (frustum.up - y) / (frustum.up - frustum.down)])
}

/// The tile `direction` lands in on an image of `pixels` rendered with `frustum` (clamped into the
/// image: a gaze past its edge takes the edge tile).
pub fn tile_of(frustum: &Frustum, pixels: (u32, u32), tile: u32, direction: [f32; 3]) -> Option<(u32, u32)> {
    let [u, v] = image_point(frustum, direction)?;
    let (across, down) = tiles(pixels, tile);
    let at = |fraction: f32, size: u32, count: u32| ((fraction.clamp(0.0, 1.0) * size as f32) as u32 / tile.max(1)).min(count.saturating_sub(1));
    Some((at(u, pixels.0, across), at(v, pixels.1, down)))
}

/// The unit direction through the centre of tile `at` (head-relative).
fn tile_direction(frustum: &Frustum, pixels: (u32, u32), tile: u32, at: (u32, u32)) -> [f32; 3] {
    let centre = |index: u32, size: u32| ((index as f32 + 0.5) * tile as f32).min(size as f32) / size.max(1) as f32;
    let x = frustum.left + centre(at.0, pixels.0) * (frustum.right - frustum.left);
    let y = frustum.up - centre(at.1, pixels.1) * (frustum.up - frustum.down);
    let length = (x * x + y * y + 1.0).sqrt();
    [x / length, y / length, -1.0 / length]
}

/// Fills a shading-rate image of `pixels` at `tile` pixels a tile, rendered with `frustum`: each
/// tile's rate by the angle between its centre and the centre of tile `centre` ([`Rings`]). `out`
/// holds rows of `pitch` bytes.
pub fn fill(frustum: &Frustum, pixels: (u32, u32), tile: u32, centre: (u32, u32), rings: &Rings, out: &mut [u8], pitch: usize) {
    let (across, down) = tiles(pixels, tile);
    let gaze = tile_direction(frustum, pixels, tile, centre);
    for y in 0..down {
        let Some(row) = out.get_mut(y as usize * pitch..y as usize * pitch + across as usize) else { return };
        for (x, texel) in row.iter_mut().enumerate() {
            let d = tile_direction(frustum, pixels, tile, (x as u32, y));
            *texel = rings.rate(d[0] * gaze[0] + d[1] * gaze[1] + d[2] * gaze[2]);
        }
    }
}

/// The live gaze in `pose`: valid, fresh (by when the viewer read it, if it says) and ahead.
fn live_gaze(pose: &HeadPose) -> Option<[f32; 3]> {
    let gaze = pose.gaze;
    let fresh = gaze.received_tick == 0 || monaka_channel::tick().saturating_sub(gaze.received_tick) <= GAZE_FRESH_MS;
    (pose.valid != 0 && gaze.valid != 0 && fresh && image_point(&Frustum { left: -1.0, right: 1.0, up: 1.0, down: -1.0 }, gaze.direction).is_some())
        .then_some(gaze.direction)
}

// --- The component -------------------------------------------------------------------------------

const OPTIONS: [Opt; 4] = [FOVEATED_RENDERING, FOVEATED_INNER, FOVEATED_MIDDLE, FOVEATED_OUTER];
const EVERY: [Every; 1] = [Every::millis(2000, report)];

/// Foveated rendering for game `G` (`foveated_rendering`, on by default), from its frame labels
/// (each frame's eye, head pose and frustum), inputs (the render size) and queue (its device and
/// command lists). A game says what it works on top of (`needs`: its eyes) by struct update.
pub const fn foveation<G: FrameInputs>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::Stereo)],
        name: "foveation",
        options: &OPTIONS,
        settings: read_settings,
        switch: Some(Switch::new("foveated_rendering", false, "foveated rendering is off")),
        install: install::<G>,
        arm,
        quiet,
        every: &EVERY,
        release,
        summary: Some(summary),
        ..Component::NONE
    }
}

fn read_settings(options: &Options, _: &monaka_contract::manifest::Manifest) {
    Rings::read(options);
}

#[derive(Clone, Copy)]
struct GameFns {
    frame: fn() -> Option<Pending>,
    frustum: fn(&HeadPose, usize) -> Option<Frustum>,
}

fn install<G: FrameInputs>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let rings = Rings::read(start.options);
    let queue = crate::queue12::game_queue(None).or_missing("the game queue is not known (D3D11, or no present yet)")?;
    let device = d3d12::device_of(&queue).map_err(|e| Lack::Missing(format!("the game's device: {e}")))?;
    {
        let mut state = STATE.locked();
        state.game = Some(GameFns { frame: G::rendering, frustum: G::frustum });
        state.device = Some(device.clone());
        state.rings = rings;
        state.support = Support::Unknown;
        state.counted = None;
    }
    *TRACK.locked() = Track::NEW;
    for counter in TALLY.all().into_iter().chain(&CALLS) {
        counter.store(0, Relaxed);
    }
    VIEWPORT_SIZES.locked().clear();
    SCENE.store(0, Relaxed);
    SAME_SIZE.store(false, Relaxed);
    // SAFETY: at install, before the hooks are enabled; the game's live queue and its device.
    unsafe { hook(start.hooks, &queue, &device) }?;
    log!(
        "foveation: full rate within {:.0} degrees of the gaze, 2x2 to {:.0}, {} beyond; without eye tracking straight ahead, {FIXED_MARGIN_DEGREES:.0} degrees wider",
        rings.inner.to_degrees(),
        rings.middle.to_degrees(),
        rate_name(rings.outer)
    );
    Ok(())
}

fn arm() {
    ARMED.store(true, Release);
}

/// No VRS from here; a list it is set on is cleared at its next pass.
fn quiet() {
    ARMED.store(false, Release);
}

/// The images stay: the GPU may still read them for frames in flight, and the next run reuses them.
fn release() {
    lists12::leave(&SUBSCRIBER, 1000);
    STATE.locked().game = None;
    LISTS.locked().clear();
    ON_LISTS.store(0, Relaxed);
}

fn rate_name(rate: u8) -> &'static str {
    match rate {
        rate::R1X1 => "1x1",
        rate::R2X2 => "2x2",
        rate::R2X4 => "2x4",
        rate::R4X4 => "4x4",
        _ => "?",
    }
}
