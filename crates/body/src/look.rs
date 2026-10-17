//! The look as one ready-made component ([`look`]): the player looks, aims and attacks where the
//! head (or a hand) points, while mouse and stick turning keep working. A game lists
//! `monaka_body::look::look()` among its components (with what it works on top of, by struct
//! update) and says how the turn reaches it ([`Looks::DELIVERY`]):
//!
//! - into the character's look targets ([`Delivery::Targets`], [`targets`]);
//! - into the game's input command as it is built ([`Delivery::Command`], [`command`]);
//! - as the game's mouse ([`Delivery::Mouse`], [`mouse`]);
//! - by a facing the mod owns, which the head turns the character to ([`Delivery::Facing`],
//!   [`facing`]).
//!
//! What every delivery shares is here: what aims ([`aim`]: the head, a hand, the left hand's tool,
//! what aims itself), the run's settings ([`settings`]), standing down while another camera has the
//! player ([`StandDown`]), whether the look steers now ([`live`]), vertical look input held neutral
//! while it does ([`neutral_pitch_if_live`]), and the share each frame's camera carries ([`baked`]):
//! the game camera a frame is built from already holds a share of the head's turn, which the eye
//! site takes back out to put the current head in.

pub mod aim;
pub mod command;
pub mod facing;
pub mod mouse;
pub mod targets;

use aim::AimSettings;
use monaka_core::feature::Feature;
use monaka_contract::manifest::{AIM_HAND, AIM_LOOK, AIM_MOTION, AIM_MOUSE, AIM_POINT, Opt, aims};
use monaka_contract::protocol::HeadPose;
use monaka_core::situation::Situation;
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Game, Lack, Need, On, Start};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};

#[cfg(test)]
mod tests;

/// The look's options for a game whose arms go on the controllers: what aims (motion controls,
/// look-based, pointing, the mouse) and which hand.
pub const OPTIONS: &[Opt] = &[monaka_contract::manifest::aim(&[AIM_MOTION, AIM_LOOK, AIM_POINT, AIM_MOUSE], aims::HAND_RIG), AIM_HAND];

/// A game with the look: how the turn reaches it, and its hooks.
pub trait Looks: Game {
    /// Where the game takes the turn.
    const DELIVERY: Delivery<Self>;
    /// The situations its look reports (another camera taking the view, a climb).
    const REPORTS: &'static [Situation] = &[];
    /// Its hooks (after what aims is set): where the camera update or the command is built, its
    /// input converter, its cutscene query.
    fn install(_start: &mut Start<'_, Self>) -> Result<(), Lack> {
        Ok(())
    }
    /// After the hooks are out.
    fn release() {}
    /// Whether another camera has `character` now (a cutscene, a takedown), and whether that
    /// changed since it was last asked.
    fn stand_down(_character: usize) -> StandDown {
        StandDown::NEVER
    }
}

/// Where a game takes the turn.
pub enum Delivery<G: Game> {
    /// The head's turn written into the character's look targets each player camera update
    /// ([`targets::steer`]); `carried`: which share the eyes take out of a frame's camera.
    Targets { carried: Carried },
    /// The head's turn added to the look angles the game's input command accumulates, as each
    /// command is built ([`command::built`]); `head`: the head pose now; `levels`: whether the
    /// player's own pitch may be kept level while only the head pitches it, asked as steering
    /// starts (a game that levels only with its own pad on says whether it is). Another camera has
    /// the view while [`Looks::stand_down`] says so, and the share then goes out; `holds`: whether
    /// the share stays in the angles as it was now (a camera that turns by the angles' changes, a
    /// mounted gun, would take the share going out as a turn; a menu during which the game leaves
    /// its camera's angles alone keeps the share they had); `pitch`: whether the player's own pitch
    /// stays under the head's ([`Pitch`]).
    Command { head: fn() -> Option<HeadPose>, levels: fn() -> bool, holds: fn() -> bool, pitch: Pitch },
    /// The head's turn sent as mouse counts, the game kept on its pad where it says how
    /// ([`mouse::on_build`] at each view build).
    Mouse(mouse::Mouse<G>),
    /// The yaw owned outright: the stick turns a facing of the mod's own, and the character's
    /// look targets are written from it plus the head each player camera update
    /// ([`facing::steer`]).
    Facing(facing::Facing),
}

/// Which share of the head's turn the eyes take out of a frame's camera ([`baked`]), by when the
/// game builds its camera against its look update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carried {
    /// The one the targets held as the update began, recorded under the game's frame number (the
    /// latest without one): the camera of that frame was built from them.
    AtUpdate,
    /// The one the targets hold now, as the latest update left them: the game builds its camera
    /// after the update steers.
    Applied,
}

/// The look component for game `G`: on with head aim.
pub const fn look<G: Looks>() -> Component<G> {
    match G::DELIVERY {
        Delivery::Targets { .. } => Component {
            needs: &[Need::Feature(Feature::Stereo)],
            name: "look",
            features: &[Feature::Look],
            reports: G::REPORTS,
            options: OPTIONS,
            needs_channel: true,
            on: |run| On::when(run.shared.aim.head, "aim is mouse and stick only"),
            install: targets::install::<G>,
            give_back: || targets::release(500),
            release: release::<G>,
            ..Component::NONE
        },
        Delivery::Command { .. } => Component {
            name: "look",
            features: &[Feature::Look],
            reports: G::REPORTS,
            on: |run| On::when(run.shared.head_tracking && run.shared.aim.head, "aim is mouse and stick only"),
            settings: |options, _| {
                options.switch("pause_hold", true);
            },
            install: command::install::<G>,
            arm: command::arm::<G>,
            quiet: command::quiet,
            give_back: command::give_back,
            release: G::release,
            summary: Some(command::summary),
            ..Component::NONE
        },
        Delivery::Mouse(_) => mouse::look::<G>(),
        Delivery::Facing(_) => Component {
            name: "look",
            features: &[Feature::Look],
            reports: G::REPORTS,
            options: OPTIONS,
            on: |run| On::when(run.shared.aim.head, "aim is mouse and stick only"),
            install: facing::install::<G>,
            ..Component::NONE
        },
    }
}

fn release<G: Looks>() {
    G::release();
    crate::controllers::close();
}

/// Whether another camera has the player at an update, as the game tells it, and whether that
/// changed since the update before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StandDown {
    pub now: bool,
    pub changed: bool,
}

impl StandDown {
    /// Never: no other camera takes the player.
    pub const NEVER: Self = Self { now: false, changed: false };
}

/// What the command delivery does with the pitch the game's own input adds ([`Delivery::Command`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pitch {
    /// Kept, with the head's on top (the mouse still pitches), and brought back toward level while
    /// only the head pitches, where the game lets it (the delivery's `levels`).
    Own,
    /// The head's alone while it aims: what the game's own input pitched since the last command is
    /// dropped (no pitch but the head's in a headset, the mouse's included).
    HeadAlone,
}

/// The head's turn as one share (radians: yaw positive left, pitch positive up).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Share {
    pub yaw: f32,
    pub pitch: f32,
}

// ---- What aims, and whether the look steers ----

/// What aims this run, from the component's install ([`configure`]).
static SETTINGS: Mutex<Option<AimSettings>> = Mutex::new(None);
/// When the look last steered (`monaka_channel::tick`).
static STEERED_AT: AtomicU64 = AtomicU64::new(0);
static STAND_DOWN_SWITCHES: AtomicU64 = AtomicU64::new(0);
static PITCH_BLOCKED: AtomicU64 = AtomicU64::new(0);

/// How long after a steering update the look counts as steering (ms): a pause, a loading screen
/// or a cutscene stops the updates, and the head's share is then left in place.
pub const LIVE_MS: u64 = 500;

/// At start: what aims.
pub fn configure(settings: AimSettings) {
    *SETTINGS.locked() = Some(settings);
}

/// What aims this run (nothing until configured).
pub fn settings() -> AimSettings {
    SETTINGS.lock().ok().and_then(|s| *s).unwrap_or(AimSettings::NONE)
}

/// The look steered at `now` (`monaka_channel::tick`).
pub(crate) fn steered(now: u64) {
    STEERED_AT.store(now, Release);
}

/// Whether the look steered within the last [`LIVE_MS`].
pub fn live() -> bool {
    monaka_channel::tick().saturating_sub(STEERED_AT.load(Acquire)) < LIVE_MS
}

/// Whether to log a stand-down's change (`asked`): each change, the first 200.
pub(crate) fn stand_down_logged(asked: StandDown) -> bool {
    asked.changed && STAND_DOWN_SWITCHES.fetch_add(1, Relaxed) < 200
}

/// The game's input-action converter got `value` for the action at `binding`: while the look
/// steers, a vertical look action (`is_pitch`) gets its neutral value (the head alone pitches the
/// character); the value as given otherwise.
pub fn neutral_pitch_if_live(binding: usize, value: f32, is_pitch: impl Fn(u32) -> bool) -> f32 {
    if live()
        && let Some(neutral) = aim::neutral_pitch(binding, is_pitch)
    {
        if value != neutral {
            PITCH_BLOCKED.fetch_add(1, Relaxed);
        }
        return neutral;
    }
    value
}

/// Vertical look inputs neutralised so far.
pub fn pitch_blocked() -> u64 {
    PITCH_BLOCKED.load(Relaxed)
}

// ---- The share each frame's camera carries ----

/// The shares recorded per frame: by frame number for the last few, and the latest.
struct Baked {
    frames: [(u64, Share); 8],
    next: usize,
    latest: Option<Share>,
    misses: u64,
}

impl Baked {
    /// No frame matches an empty slot (frame `u64::MAX` is never recorded).
    const EMPTY: Self = Self { frames: [(u64::MAX, Share { yaw: 0.0, pitch: 0.0 }); 8], next: 0, latest: None, misses: 0 };

    fn record(&mut self, frame: Option<u64>, share: Option<Share>) {
        if let (Some(frame), Some(share)) = (frame, share) {
            self.frames[self.next] = (frame, share);
            self.next = (self.next + 1) % self.frames.len();
        }
        self.latest = share;
    }

    /// The share recorded for `frame`, else (counted as a miss) the last one recorded by frame.
    fn at(&mut self, frame: u64) -> Share {
        match self.frames.iter().find(|(f, _)| *f == frame) {
            Some(&(_, share)) => share,
            None => {
                self.misses += 1;
                self.frames[(self.next + self.frames.len() - 1) % self.frames.len()].1
            }
        }
    }
}

static BAKED: Mutex<Baked> = Mutex::new(Baked::EMPTY);

/// The share the camera of frame `frame` (the game's frame number, none: the latest) carries;
/// none for a frame that carries none.
pub(crate) fn record(frame: Option<u64>, share: Option<Share>) {
    BAKED.locked().record(frame, share);
}

/// The share recorded for `frame` (a miss: the last recorded by frame), or the latest recorded
/// (none before the first).
pub(crate) fn recorded(frame: Option<u64>) -> Option<Share> {
    let mut baked = BAKED.locked();
    match frame {
        Some(frame) => Some(baked.at(frame)),
        None => baked.latest,
    }
}

/// Frames whose share was not found by their number.
pub(crate) fn baked_misses() -> u64 {
    BAKED.locked().misses
}

/// The share of the head's turn the game camera of frame `frame` (the game's frame number, where
/// it has one) carries, by game `G`'s delivery: the eye site turns it back out of the view and puts
/// the current head in, so a head turn is not counted twice. None while the camera carries none
/// (the look not steering, another camera having the view).
pub fn baked<G: Looks>(frame: Option<u64>) -> Option<Share> {
    match G::DELIVERY {
        Delivery::Targets { carried: Carried::AtUpdate } => targets::baked_in_view(frame),
        Delivery::Targets { carried: Carried::Applied } => targets::current_share().map(|yaw| Share { yaw, pitch: 0.0 }),
        Delivery::Command { .. } => command::baked(),
        Delivery::Mouse(_) | Delivery::Facing(_) => recorded(None),
    }
}

/// Logs the run's look tally: neutralised inputs, frames not found, and where the aim came from.
pub(crate) fn report(updates: u64) {
    log!("head aim: {} updates; vertical look inputs neutralised {}; baked yaw not found by game counter {} times", updates, pitch_blocked(), baked_misses());
    aim::report_sources(settings().hand_aim);
}
