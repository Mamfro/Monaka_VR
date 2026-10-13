//! AFW (alternate frame warping), the part every graphics API shares: alternate-eye rendering
//! where every present is a full pair of one moment. The real eye's frame is warped into the other
//! eye with its depth, and what the real eye could not see is filled from that other eye's own
//! newest real frame, which saw it from the right place, so the gaps get rendered pixels instead of
//! stretched ones. Nothing waits for a later frame: no latency is added.
//!
//! Two backends do the GPU work on it: [`crate::afw11`] on D3D11 (each frame captured at the game's
//! scene copy, the pair published over the keyed-mutex channel) and [`crate::afw12`] on D3D12
//! (each frame warped at its present request, the pair published over the fence channel, with each
//! eye's depth to the headset and frame generation on top). Both run the same reprojection
//! (`monaka_warp::reproject`). What they share is here:
//!
//! - which eye is real this frame, the head pose of its moment, how it was rendered and how the
//!   other eye would be ([`Shot`]: by the present loop's schedule and the cameras the eye site
//!   recorded, or by the frame's label and its inputs; [`Rendered`]);
//! - each eye's newest real frame and when it may fill the other eye's gaps ([`History`]), and what
//!   each warp draws from and into ([`Warp`]);
//! - what goes out at each present ([`Presents`]: the warped pair, a menu flat, an unlabelled frame
//!   to both eyes, a frame not warped beside the other eye's newest), and frame generation's
//!   history ([`Previous`]);
//! - the rules where the backends have differed ([`Rules`]);
//! - the counts and the report line ([`Counts`], [`Counters`], [`report_line`]);
//! - the ready-made component's shape ([`component`], [`on`]).

use crate::driver::Pending;
use monaka_core::alternate::{PresentRing, Schedule};
use monaka_core::camera::{Frustum, Mat34, Mat44, offset_right, plausible_camera};
use monaka_core::depth::{DepthMapping, depth_params};
use monaka_core::math::Vec3;
use monaka_core::protocol::{DepthParams, HeadPose};
use monaka_core::rendering::StereoMethod;
use monaka_framegen::inputs::Rendering;
use monaka_producer::game::{Component, Every, Game, Lack, On, Start};
use monaka_warp::View;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

#[cfg(test)]
mod lift_checks;
#[cfg(test)]
mod tests;

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

    /// Drawn into at `width` x `height`.
    pub fn view(&self, width: u32, height: u32) -> View {
        View { projection: self.projection, camera: self.camera, mapping: self.mapping, width, height }
    }
}

/// Both eyes' cameras at one moment (canonical), each eye's depth mapping, and the head pose they
/// come from: what a D3D11 game's eye site records for each frame it draws.
#[derive(Clone, Copy, Debug)]
pub struct EyeCameras {
    pub camera: [Mat34; 2],
    pub projection: [Mat44; 2],
    pub mapping: [DepthMapping; 2],
    pub pose: HeadPose,
}

impl EyeCameras {
    /// Eye `eye`'s (0 left), as recorded.
    pub fn rendered(&self, eye: usize) -> Rendered {
        let eye = eye.min(1);
        Rendered { camera: self.camera[eye], projection: self.projection[eye], mapping: self.mapping[eye] }
    }
}

/// `this` (eye `eye`'s, 0 left) and `other` (the other eye's) as (left, right).
pub fn in_eye_order<T>(eye: usize, this: T, other: T) -> [T; 2] {
    if eye == 0 { [this, other] } else { [other, this] }
}

/// One present's real eye: which eye it is (0 left), the head pose its moment was drawn from (the
/// pair's), how it was rendered and how the other eye would be at that moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub eye: usize,
    pub pose: HeadPose,
    pub rendered: Rendered,
    pub other: Rendered,
}

impl Shot {
    /// The frame present `n` shows on the alternate-eye loop by its `schedule` (the eye chosen at
    /// the present `latency` before), from the cameras the eye site recorded under that present.
    /// `None` without them.
    pub fn scheduled(schedule: Schedule, n: u64, recorded: &PresentRing<EyeCameras>) -> Option<Self> {
        let eye = schedule.eye_at(n);
        let cameras = recorded.get(schedule.chosen_at(n))?;
        Some(Self { eye, pose: cameras.pose, rendered: cameras.rendered(eye), other: cameras.rendered(1 - eye) })
    }

    /// The frame labelled `label`, rendered as `rendered`: the other eye `separation` along its
    /// right axis, with its own `frustum` when it has one.
    pub fn labelled(label: Pending, rendered: Rendered, separation: f32, frustum: Option<Frustum>) -> Self {
        let eye = label.eye.min(1);
        Self { eye, pose: label.pose, rendered, other: rendered.other_eye(eye, separation, frustum) }
    }

    pub fn other_eye(&self) -> usize {
        1 - self.eye
    }

    /// How each eye is seen (left, right): the real eye's, and the view it is warped into.
    pub fn views(&self) -> [Rendered; 2] {
        in_eye_order(self.eye, self.rendered, self.other)
    }
}

/// The rules where the backends have differed. Each backend keeps its game's (`afw11::RULES`,
/// `afw12::RULES`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// A real frame fills the other eye's gaps while it is at most this many warps old (`None`:
    /// whatever its age, kept through pauses).
    pub fill_age: Option<u64>,
    /// A frame shown flat (a menu) goes out flat, and an unlabelled one, after a moment, to both
    /// eyes, both eyes' frames forgotten first. Without, such a frame warps or publishes nothing as
    /// any other.
    pub flat_frames: bool,
    /// A labelled frame that was not warped goes out beside the other eye's newest frame, when that
    /// is the frame just before (plain alternate eyes). Without, it publishes nothing.
    pub plain_pairs: bool,
}

impl Rules {
    /// Every rule: an old frame (from before a pause or a cut) fills nothing, menus go flat, a
    /// frame not warped still goes out.
    pub const WHOLE: Self = Self { fill_age: Some(4), flat_frames: true, plain_pairs: true };
    /// Warped pairs only: the other eye's newest frame fills whatever its age, and a frame not
    /// warped (a menu, a loading screen, no depth) publishes nothing.
    pub const WARPED_ONLY: Self = Self { fill_age: None, flat_frames: false, plain_pairs: false };
}

/// Each eye's newest real frame: how it was rendered and the warp it came at. The backend keeps its
/// copies (colour, depth) beside it.
#[derive(Clone, Copy, Debug)]
pub struct History {
    fill_age: Option<u64>,
    kept: [Option<(Rendered, u64)>; 2],
    warps: u64,
}

/// One warp: the real eye, drawn from how it was rendered (`primary`) into the other eye's view
/// (`target`), its gaps from the other eye's newest real frame as it was rendered (`fill`), when
/// the rules let it fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    pub eye: usize,
    pub primary: Rendered,
    pub target: Rendered,
    pub fill: Option<Rendered>,
}

impl History {
    pub const fn new(rules: Rules) -> Self {
        Self { fill_age: rules.fill_age, kept: [None, None], warps: 0 }
    }

    /// A warp begins (its command list or capture under way): its number.
    pub fn begin(&mut self) -> u64 {
        self.warps += 1;
        self.warps
    }

    /// `shot`'s real eye has its frame copied: that eye's newest from the warp just begun.
    pub fn keep(&mut self, shot: &Shot) {
        self.kept[shot.eye] = Some((shot.rendered, self.warps));
    }

    /// Eye `eye`'s frame copy was lost (the copy failed): nothing of it fills.
    pub fn lose(&mut self, eye: usize) {
        self.kept[eye.min(1)] = None;
    }

    /// The warp of `shot`: its real eye into the other eye, the gaps from the other eye's newest
    /// real frame while it is young enough.
    pub fn warp(&self, shot: &Shot) -> Warp {
        let fill = self.kept[shot.other_eye()].filter(|(_, at)| self.fill_age.is_none_or(|age| self.warps - at <= age)).map(|(rendered, _)| rendered);
        Warp { eye: shot.eye, primary: shot.rendered, target: shot.other, fill }
    }

    /// Both eyes' frames forgotten (a menu, a cut, new sizes): the next warp fills its gaps from
    /// neighbours only.
    pub fn forget(&mut self) {
        self.kept = [None, None];
    }
}

/// What goes out at a present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// The present's warped pair.
    Warped,
    /// The frame flat (a menu).
    Flat,
    /// The frame to both eyes with the newest head pose (unlabelled for a while: the pause screen,
    /// loading, the player camera not updated).
    BothEyes,
    /// The frame, not warped, as its eye beside the other eye's newest frame.
    Plain,
    /// Nothing (a lone unlabelled frame in play: a flat one would flash; a frame not warped).
    Nothing,
}

impl Out {
    /// Both eyes' histories start afresh: the backend forgets its frames ([`History::forget`]) and
    /// whatever else it keeps of them.
    pub fn forgets(self) -> bool {
        matches!(self, Out::Flat | Out::BothEyes)
    }
}

/// An unlabelled frame this soon after a labelled one is skipped.
const UNLABELLED_SKIP: Duration = Duration::from_millis(200);

/// The last pair frame generation was given: its pose, real eye and present.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Previous {
    pub pose: HeadPose,
    pub eye: usize,
    pub present: u64,
}

/// What goes out at each present, by the rules: whether a frame was labelled lately, each eye's
/// last frame as presented (its pose and present, for plain pairs), frame generation's last pair.
#[derive(Clone, Copy, Debug)]
pub struct Presents {
    rules: Rules,
    last_labelled: Option<Instant>,
    plain: [Option<(HeadPose, u64)>; 2],
    previous: Option<Previous>,
}

impl Presents {
    pub const fn new(rules: Rules) -> Self {
        Self { rules, last_labelled: None, plain: [None, None], previous: None }
    }

    /// The frame about to present at `now`: shown `flat` (a menu), `labelled` (its eye known),
    /// `warped` (its pair made). When the answer [`Out::forgets`], frame generation's history is
    /// gone too.
    pub fn out(&mut self, flat: bool, labelled: bool, warped: bool, now: Instant) -> Out {
        if self.rules.flat_frames {
            if flat {
                self.forget();
                return Out::Flat;
            }
            if !labelled {
                if self.last_labelled.is_some_and(|t| now.saturating_duration_since(t) < UNLABELLED_SKIP) {
                    return Out::Nothing;
                }
                self.forget();
                return Out::BothEyes;
            }
            self.last_labelled = Some(now);
        }
        match (warped, self.rules.plain_pairs) {
            (true, _) => Out::Warped,
            (false, true) => Out::Plain,
            (false, false) => Out::Nothing,
        }
    }

    /// Frame generation's history dropped (a menu, a cut).
    pub fn forget(&mut self) {
        self.previous = None;
    }

    /// Eye `eye`'s last presented frame is being replaced (its copy taken for the new one).
    pub fn lose_plain(&mut self, eye: usize) {
        self.plain[eye.min(1)] = None;
    }

    /// Eye `eye`'s frame drawn from `pose`, presented at `present`, is copied as that eye's newest:
    /// the other eye's newest pose when it is the frame just before (a plain pair goes out).
    pub fn plain(&mut self, eye: usize, pose: HeadPose, present: u64) -> Option<HeadPose> {
        let eye = eye.min(1);
        self.plain[eye] = Some((pose, present));
        self.plain[1 - eye].filter(|(_, at)| present.saturating_sub(*at) <= 1).map(|(pose, _)| pose)
    }

    /// Frame generation starts on eye `eye`'s pair at `present`: the last pair (taken: it is this
    /// pair's once both streams were given it, [`Presents::generated`]), and whether the history
    /// runs on: the last pair was the other eye's frame of the present before, the game kept its
    /// own history (no `reset`) and the warped eye's gaps came from its own last frame (`filled`).
    pub fn generation(&mut self, eye: usize, present: u64, reset: bool, filled: bool) -> (Option<Previous>, bool) {
        let previous = self.previous.take();
        let continuous = previous.is_some_and(|p| p.eye != eye && p.present + 1 == present) && !reset && filled;
        (previous, continuous)
    }

    /// Both streams were given the pair: frame generation's history from now on.
    pub fn generated(&mut self, pair: Previous) {
        self.previous = Some(pair);
    }
}

/// The generated pair's pose: halfway between the last pair's and this one's.
pub fn halfway(before: &HeadPose, pose: &HeadPose) -> HeadPose {
    monaka_framegen::blend(before, pose, 0.5)
}

/// A backend's warps.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub pairs: u64,
    /// Pairs whose gaps were filled from the warped eye's own last real frame.
    pub filled: u64,
    /// Presents with no command list free (the GPU behind).
    pub busy: u64,
    pub failures: u64,
}

/// What went out, counted where it happens (the present, the HUD, frame generation's pacing).
pub struct Counters {
    /// Presents whose frame had no fresh depth and camera (not warped).
    pub no_inputs: AtomicU64,
    /// Pairs of the two newest real frames, a frame apart (a frame not warped).
    pub plain: AtomicU64,
    pub flat: AtomicU64,
    pub generated: AtomicU64,
    pub real: AtomicU64,
    /// Our images without the HUD (no command list free, or a format it cannot go over).
    pub no_hud: AtomicU64,
}

impl Counters {
    pub const fn new() -> Self {
        Self {
            no_inputs: AtomicU64::new(0),
            plain: AtomicU64::new(0),
            flat: AtomicU64::new(0),
            generated: AtomicU64::new(0),
            real: AtomicU64::new(0),
            no_hud: AtomicU64::new(0),
        }
    }

    /// One more of `counter`.
    pub fn add(counter: &AtomicU64) {
        counter.fetch_add(1, Relaxed);
    }
}

impl Default for Counters {
    fn default() -> Self {
        Self::new()
    }
}

/// The periodic report's line: each eye's frames taken (`inputs`), the warps, what went out, the
/// pairs that carried depth (`with_depth`), and with frame generation the eyes it skipped
/// (`framegen`).
pub fn report_line(inputs: [u64; 2], warped: Counts, counters: &Counters, with_depth: u64, framegen: Option<u64>) -> String {
    let count = |c: &AtomicU64| c.load(Relaxed);
    let mut line = format!(
        "frame warping: inputs L/R {}/{}, warped pairs {} (gaps from the eye's own frame {}), GPU busy {}, failed {}, frames without inputs {}, plain pairs {}, flat {}, without the HUD {}",
        inputs[0],
        inputs[1],
        warped.pairs,
        warped.filled,
        warped.busy,
        warped.failures,
        count(&counters.no_inputs),
        count(&counters.plain),
        count(&counters.flat),
        count(&counters.no_hud),
    );
    if with_depth > 0 {
        line += &format!(", pairs with depth {with_depth}");
    }
    if let Some(skipped) = framegen {
        line += &format!("; in-between pairs {}, real pairs {}, eyes not generated {skipped}", count(&counters.generated), count(&counters.real));
    }
    line
}

/// The component's name in the log.
pub const NAME: &str = "frame warping";
/// How often the report goes to the log.
pub const REPORT_MS: u64 = 2000;

/// The ready-made component's shape for game `G`, the same on every backend: its name, its
/// backend's `install` and `release`, its report every [`REPORT_MS`] (`every`) and at the stop
/// (`summary`), and whether it publishes on a channel of its own (`own_channel`). The game says when
/// it is on ([`on`]) and what it works on top of (`needs`: its eyes) by struct update.
pub const fn component<G: Game>(
    install: fn(&mut Start<'_, G>) -> Result<(), Lack>,
    release: fn(),
    every: &'static [Every],
    summary: fn() -> String,
    own_channel: bool,
) -> Component<G> {
    Component { name: NAME, needs_channel: own_channel, install, every, release, summary: Some(summary), ..Component::NONE }
}

/// On while the game's stereo method is AFW.
pub fn on(method: StereoMethod) -> On {
    On::when(method == StereoMethod::Afw, "AER chosen")
}
