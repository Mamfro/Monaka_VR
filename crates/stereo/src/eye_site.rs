//! The eye site: the hook where a game's camera becomes the eye a present loop chose
//! ([`crate::feature::DRIVER`]'s pending eye and pose, or the pair of the same-frame loop,
//! [`crate::same_frame::PAIRS`]). The view itself is `monaka_core::camera::EyeView`; a game reads
//! its camera in its own form, makes the eye's view ([`draw`]), and writes it back its own way (in
//! place and restored after the call, into a copy of its own, or through the engine's setter).
//!
//! What a game supplies once is the provider [`EyeSite`]: its site's hooks (installed by the loop
//! its frames go out by), its counts, and how each eye is drawn (its frustum, the eyes'
//! separation). The present loops read it: the pose each pair goes out with ([`report`]) and the
//! rate line's count of eye views ([`work`]). Here too: the stereo
//! component ready-made ([`eyes`]), the site's counts ([`Tally`]) and whether the world is being
//! drawn by them ([`world_hidden`]).

pub mod checks;

use crate::driver::Flat;
use crate::feature::DRIVER;
use crate::providers::EyeGeometry;
use monaka_core::camera::{Base, EyeView, Frustum, Mat34};
use monaka_core::feature::Feature;
use monaka_contract::manifest::{EYE_SEPARATION, HEAD_MOVEMENT, LATENCY};
use monaka_core::options::Shared;
use monaka_contract::protocol::HeadPose;
use monaka_producer::game::{Component, Game, Lack, Start, Switch};
use monaka_producer::log;
use std::fmt::Display;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Whether stereo is on for the run starting (the [`eyes`] component's switch, `stereo`): an eye
/// site installed with its loop writes no eye without it.
pub fn stereo_on<G: Game>(start: &Start<'_, G>) -> bool {
    eyes::<G>().switch.is_none_or(|switch| switch.on(start.options, start.manifest))
}

/// An install step of a game's own, run with a loop's (its eye site's, its present entry's).
pub type InstallFn<G> = fn(&mut Start<'_, G>) -> Result<(), Lack>;

/// A game's eye site: where its camera is read and an eye written into it, as the present loops
/// see it. The loop the game's frames go out by installs it ([`EyeSite::install`]: the same-frame
/// loop always, the alternate-eye loop as its `Loop::eye_site` says).
pub trait EyeSite: EyeGeometry {
    /// The site's hooks into the game, and what the run sets for it (at the loop's install, after
    /// the loop's own).
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack> {
        let _ = start;
        Ok(())
    }
    /// The site's counts (its calls, the eye views drawn and refused).
    fn tally() -> &'static Tally;
    /// The frustum a pair's eye goes out with in its pose record ([`report`]): `None` keeps the
    /// field of view the pose carries. By default the frustum it was drawn with
    /// ([`EyeGeometry::frustum`]).
    fn reported_frustum(pose: &HeadPose, eye: usize) -> Option<Frustum> {
        Self::frustum(pose, eye)
    }
}

/// The pose a pair drawn at `G`'s site goes out with: the eye distance used, and each eye's field
/// of view as it was drawn (left as it is where the site does not know it).
pub fn report<G: EyeSite>(pose: &mut HeadPose) {
    if let Some(separation) = G::separation(pose) {
        pose.ipd = separation;
    }
    let drawn = *pose;
    for (eye, fov) in pose.fov.iter_mut().enumerate() {
        if let Some(frustum) = G::reported_frustum(&drawn, eye) {
            *fov = frustum.fov();
        }
    }
}

/// The present loop's rate line for `G`: what the site's calls are, and its eye views drawn.
pub fn work<G: EyeSite>() -> (&'static str, &'static AtomicU64) {
    let tally = G::tally();
    (tally.what, &tally.drawn)
}

/// Presents without an eye view drawn at `G`'s site after which the world is not being drawn (a
/// loading screen).
pub const WORLD_GONE_AFTER: u64 = 8;

/// Whether `G`'s site drew an eye view within the alternate-eye loop's last few presents
/// ([`WORLD_GONE_AFTER`]): the world is being drawn (not a loading screen, where levels are
/// unloaded and their UI freed).
pub fn world_drawn_recently<G: EyeSite>() -> bool {
    drawn_recently(DRIVER.presents(), G::tally().drawn_at.load(Relaxed))
}

/// Whether at present `now` an eye view drawn at present `drawn_at` is recent ([`WORLD_GONE_AFTER`]).
fn drawn_recently(now: u64, drawn_at: u64) -> bool {
    now.saturating_sub(drawn_at) <= WORLD_GONE_AFTER
}

/// Why the frame about to present goes to the flat screen, as `G`'s site knows it (a loop's flat
/// detector, `crate::feature::Loop::flat`): no eye view has drawn the world for a few presents.
pub fn world_hidden<G: EyeSite>() -> Option<Flat> {
    (!world_drawn_recently::<G>()).then_some(Flat::WorldHidden)
}

/// One call (`call`) at a site: eye `eye` of the game camera `game` (canonical, taken as `base`,
/// its full angles `game_fov`) for the head `pose` (`monaka_core::camera::EyeView::of`), counted on
/// `tally`: drawn, with head aim in the camera, or refused with `why` the game says.
#[allow(clippy::too_many_arguments)]
pub fn draw(tally: &Tally, call: u64, game: &Mat34, base: Base, pose: &HeadPose, eye: usize, shared: &Shared, centred: bool, game_fov: [f32; 2], why: impl Display) -> Option<EyeView> {
    if base != Base::Game {
        tally.aimed();
    }
    match EyeView::of(game, base, pose, eye, shared, centred, game_fov) {
        Some(view) => {
            tally.drawn();
            Some(view)
        }
        None => {
            tally.refused(call, why);
            None
        }
    }
}

/// The stereo component for game `G`: its options and its switch (`stereo=0`: both eyes get the
/// game's own camera). A game adds its site's hook (`install`), what it works on top of (`needs`)
/// and its counts (`summary`) by struct update.
pub const fn eyes<G: Game>() -> Component<G> {
    Component {
        name: "eyes",
        features: &[Feature::Stereo],
        options: &[HEAD_MOVEMENT, LATENCY, EYE_SEPARATION],
        switch: Some(Switch::new("stereo", true, "stereo is off: both eyes get the game's own camera")),
        ..Component::NONE
    }
}

/// An eye site's counts: its calls (the present loop's eye-view count), the eye views drawn, the
/// ones refused, and those whose game camera carried head aim.
pub struct Tally {
    /// What a call is, for the summary ("view builds").
    pub what: &'static str,
    pub calls: AtomicU64,
    pub drawn: AtomicU64,
    pub refused: AtomicU64,
    pub aimed: AtomicU64,
    /// The alternate-eye loop's present when the last eye view was drawn ([`world_drawn_recently`]).
    drawn_at: AtomicU64,
}

impl Tally {
    pub const fn new(what: &'static str) -> Self {
        Self { what, calls: AtomicU64::new(0), drawn: AtomicU64::new(0), refused: AtomicU64::new(0), aimed: AtomicU64::new(0), drawn_at: AtomicU64::new(0) }
    }

    /// A call: its number.
    pub fn call(&self) -> u64 {
        self.calls.fetch_add(1, Relaxed)
    }

    /// An eye view drawn: how many before it.
    pub fn drawn(&self) -> u64 {
        self.drawn_at.store(DRIVER.presents(), Relaxed);
        self.drawn.fetch_add(1, Relaxed)
    }

    /// A call whose game camera carried head aim.
    pub fn aimed(&self) {
        self.aimed.fetch_add(1, Relaxed);
    }

    /// An eye view not drawn at call `call`, logged with `why` the first three times.
    pub fn refused(&self, call: u64, why: impl Display) {
        if self.refused.fetch_add(1, Relaxed) < 3 {
            log!("{} {call}: eye view not drawn: {why}", self.what);
        }
    }

    /// The stop's line.
    pub fn summary(&self) -> String {
        format!(
            "{} {} ({} with head aim in the game camera), {} eye views drawn, {} refused",
            self.calls.load(Relaxed),
            self.what,
            self.aimed.load(Relaxed),
            self.drawn.load(Relaxed),
            self.refused.load(Relaxed)
        )
    }
}
