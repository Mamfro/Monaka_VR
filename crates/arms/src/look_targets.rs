//! Head aim by steering the character's look targets, as a ready-made component ([`look`]): the
//! character's own look targets follow the head, so it looks, aims and attacks where the head
//! points. A game lists `monaka_arms::look_targets::look()` among its components (with what it works
//! on top of, by struct update) and says once where its look targets are and where it steers
//! ([`LooksByTargets`]: its [`TargetAim`] and its look provider's install). At each player camera
//! update its hook hands the character to [`TargetAim::steer`].
//!
//! On each update the target pitch becomes the head's and the head's yaw is added to the target yaw
//! (minus what was added before), so mouse and stick turning keep accumulating on top; vertical look
//! input is neutralised meanwhile ([`TargetAim::neutral_pitch_if_live`], from the game's input
//! converter). The camera an update sets was built from the previous update's targets: the eyes are
//! built on it levelled, with that head yaw turned back out ([`TargetAim::baked_in_view`], recorded
//! per game frame). What was added is taken out again when VR stops ([`TargetAim::release`]). While
//! another camera has the player (the game's cutscene, as its look provider tells it) head aim
//! stands down: the share goes back and the game's camera and stick are its own.
//!
//! Hand aim: the aiming controller's direction replaces the head's (`crate::aim`); what aims itself
//! by hand (a throw, a bow) keeps the game's aim on the head.

use crate::aim::{AimSettings, LookFields, LookSteer, TOOL, neutral_pitch, note_source};
use monaka_core::aim::{BakedYaw, Convention, Look};
use monaka_core::camera::{Mat34, head_yaw_pitch, tracking_origin};
use monaka_core::feature::Feature;
use monaka_core::manifest::{AIM_HAND, AIM_LOOK, AIM_MOTION, AIM_MOUSE, AIM_POINT, Opt, aim, aims};
use monaka_core::protocol::HeadPose;
use monaka_core::situation::Situation;
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Game, Lack, On, Start};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};

#[cfg(test)]
mod lift_checks;
#[cfg(test)]
mod tests;

/// The look's options: what aims (motion controls, look-based, pointing, the mouse) and which hand.
pub const OPTIONS: &[Opt] = &[aim(&[AIM_MOTION, AIM_LOOK, AIM_POINT, AIM_MOUSE], aims::HAND_RIG), AIM_HAND];

/// A game whose look is steered through its character's look targets.
pub trait LooksByTargets: Game {
    /// The situations its look provider reports (another camera taking the view, where it tells).
    const REPORTS: &'static [Situation] = &[];
    /// Its target aim: where the targets are, and the steering's state.
    fn aim() -> &'static TargetAim;
    /// Its look provider's install, after what aims is set ([`TargetAim::settings`]): where it
    /// steers (its hook calls [`TargetAim::steer`]), its input converter, where it tells another
    /// camera has the player.
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack>;
    /// After the hooks are out.
    fn release() {}
}

/// The head aim component for game `G`: on with head aim, the controllers in for a pointing hand.
pub const fn look<G: LooksByTargets>() -> Component<G> {
    Component {
        name: "look",
        features: &[Feature::Look],
        reports: G::REPORTS,
        options: OPTIONS,
        needs_channel: true,
        on: |run| On::when(run.shared.aim.head, "aim is mouse and stick only"),
        install: install::<G>,
        give_back: give_back::<G>,
        release: release::<G>,
        ..Component::NONE
    }
}

fn install<G: LooksByTargets>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let settings = AimSettings::from_options(start.options, start.shared.aim, true);
    G::aim().configure(settings);
    log!("{settings:?}");
    G::install(start)?;
    if settings.hand_aim {
        crate::controllers::open(start.channel()?);
    }
    Ok(())
}

/// The next camera update hands the head's share back.
fn give_back<G: LooksByTargets>() {
    G::aim().release(500);
}

fn release<G: LooksByTargets>() {
    G::release();
    crate::controllers::close();
}

/// How long after an update head aim counts as steering (ms): a pause, a loading screen or a
/// cutscene stops the updates, and the head's share is then left in place.
const LIVE_MS: u64 = 500;

/// What a steering update did.
#[derive(Clone, Copy, Debug)]
pub struct Steered {
    /// The look written and the character's own (what it had without the head), degrees.
    pub written: Look,
    pub own: Look,
    /// The head yaw (radians) the camera this update set carries (the previous update's share).
    pub baked: f32,
    /// The update's number this run.
    pub update: u64,
    pub by_hand: bool,
}

/// Whether another camera has the player at an update, as the game's look provider tells it, and
/// whether that changed since the update before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StandDown {
    pub now: bool,
    pub changed: bool,
}

/// The steering of one game's character's look targets.
pub struct TargetAim {
    steer: Mutex<LookSteer>,
    settings: Mutex<Option<AimSettings>>,
    /// While set, the next update hands the head's share back.
    releasing: AtomicBool,
    /// When head aim last steered the character (`monaka_channel::tick`).
    tick: AtomicU64,
    updates: AtomicU64,
    pitch_blocked: AtomicU64,
    baked_misses: AtomicU64,
    /// The head yaw (radians) baked into the game camera of each game frame, by game counter,
    /// and the latest one (f32 bits) for games that number no frames.
    baked: Mutex<BakedYaw>,
    latest_baked: AtomicU32,
    stand_down_switches: AtomicU64,
}

impl TargetAim {
    /// The targets at `yaw` and `pitch` in the character (degrees, yaw turning right as it grows).
    pub const fn new(yaw: usize, pitch: usize) -> Self {
        Self {
            steer: Mutex::new(LookSteer::new(LookFields { yaw, pitch, convention: Convention::DEGREES_YAW_RIGHT })),
            settings: Mutex::new(None),
            releasing: AtomicBool::new(false),
            tick: AtomicU64::new(0),
            updates: AtomicU64::new(0),
            pitch_blocked: AtomicU64::new(0),
            baked_misses: AtomicU64::new(0),
            baked: Mutex::new(BakedYaw::EMPTY),
            latest_baked: AtomicU32::new(0),
            stand_down_switches: AtomicU64::new(0),
        }
    }

    /// At start: what aims.
    pub fn configure(&self, settings: AimSettings) {
        *self.settings.locked() = Some(settings);
        self.releasing.store(false, Release);
    }

    /// What aims this run (nothing until configured).
    pub fn settings(&self) -> AimSettings {
        self.settings.lock().ok().and_then(|s| *s).unwrap_or(AimSettings { head_aim: false, hand_aim: false, aim_hand: 1, hand_rig: false, melee_aim_head: true, tool_aim: false })
    }

    /// Whether head aim steered the character within the last half second.
    pub fn live(&self) -> bool {
        monaka_channel::tick().saturating_sub(self.tick.load(Acquire)) < LIVE_MS
    }

    /// The head yaw (radians) the targets carry for `character` now.
    pub fn share(&self, character: usize) -> f32 {
        self.steer.lock().map(|s| s.share(character)).unwrap_or(0.0)
    }

    /// The share the latest update's camera carries, while head aim steers.
    pub fn current_share(&self) -> Option<f32> {
        let steer = self.steer.lock().ok()?;
        (self.live() && steer.holds_share()).then(|| steer.share(steer.applied.owner()))
    }

    /// One player camera update: steers `character` for the next update toward the head (or the
    /// aiming controller), recording the share this update's camera carries under `counter` (the
    /// game frame, when the game numbers them). `aims_itself`: what is in hand aims itself (a throw,
    /// a bow by hand), so the game's aim stays the head's. With no `head` (VR stopped, no pose),
    /// while releasing, or while another camera has the player (`stand_down`), hands the game's own
    /// angles and the left trigger back; `None` then.
    pub fn steer(&self, character: usize, head: Option<HeadPose>, counter: Option<u64>, stand_down: StandDown, aims_itself: impl FnOnce() -> bool) -> Option<Steered> {
        let settings = self.settings();
        let mut steer = self.steer.locked();
        // The share this update's camera carries (written by the previous update).
        let baked = steer.share(character);
        self.latest_baked.store(baked.to_bits(), Relaxed);
        if let Some(counter) = counter {
            self.baked.locked().record(counter, baked);
        }
        if stand_down.changed && self.stand_down_switches.fetch_add(1, Relaxed) < 200 {
            log!("head aim: {}", if stand_down.now { "a cutscene has the player; standing down" } else { "the cutscene let go; steering" });
        }
        let head = head.filter(|h| h.valid != 0 && settings.head_aim && !self.releasing.load(Acquire) && !stand_down.now);
        if let Some(head) = head {
            let mut aim = settings.aim_at(monaka_channel::tick(), head.orientation, crate::controllers::palm);
            // A throw or a bow by hand aims itself: the game's aim (the flashlight with it) stays
            // the head's.
            if settings.hand_rig && aims_itself() {
                (aim.yaw, aim.pitch) = head_yaw_pitch(head.orientation);
                aim.by_hand = false;
                aim.tool = false;
            }
            note_source(aim.by_hand, settings.hand_aim);
            match steer.steer(character, aim.yaw, aim.pitch) {
                Some((written, own)) => {
                    let update = self.updates.fetch_add(1, Relaxed);
                    // A left trigger press waits for the left controller's aim.
                    TOOL.gate(TOOL.possible(&settings), aim.tool, update);
                    self.tick.store(monaka_channel::tick(), Release);
                    if update == 0 {
                        log!("head aim: steering the character ({} aim; target look {:?})", if aim.by_hand { "hand" } else { "head" }, steer.fields.read(character));
                    }
                    return Some(Steered { written, own, baked, update, by_hand: aim.by_hand });
                }
                None => monaka_producer::log_first!(3, "head aim: not steering: look {:?}, head yaw {:.2} pitch {:.2}", steer.fields.read(character), aim.yaw, aim.pitch),
            }
        }
        TOOL.release();
        steer.release(character);
        None
    }

    /// The head yaw (radians) baked into the game camera of frame `counter` (the latest known
    /// when the frame is not on record, or with no counter).
    pub fn baked_at(&self, counter: Option<u64>) -> f32 {
        match counter {
            Some(counter) => {
                let (yaw, found) = self.baked.locked().lookup(counter);
                if !found {
                    self.baked_misses.fetch_add(1, Relaxed);
                }
                yaw
            }
            None => f32::from_bits(self.latest_baked.load(Relaxed)),
        }
    }

    /// The head yaw (radians) the game camera of frame `counter` carries, to be turned back out of
    /// the view so a head turn is not counted twice: [`TargetAim::baked_at`] while head aim steers,
    /// none otherwise.
    pub fn baked_in_view(&self, counter: Option<u64>) -> f32 {
        if self.live() { self.baked_at(counter) } else { 0.0 }
    }

    /// The tracking origin the eyes and arms are built on: the game camera levelled, the head yaw
    /// baked into frame `counter` turned back out ([`TargetAim::baked_in_view`]), and moved by
    /// room-scale following ([`crate::roomscale::shift_origin`]).
    pub fn tracking_origin(&self, game: &Mat34, counter: Option<u64>) -> Option<Mat34> {
        let mut origin = tracking_origin(game, self.baked_in_view(counter))?;
        crate::roomscale::shift_origin(&mut origin);
        Some(origin)
    }

    /// Hands the head's share back at the next update (waits up to `timeout_ms` for it) and logs
    /// the run's tally.
    pub fn release(&self, timeout_ms: u64) {
        let settings = self.settings();
        self.releasing.store(true, Release);
        TOOL.release();
        if settings.head_aim && !monaka_producer::wait_until(timeout_ms, || !self.steer.lock().map(|s| s.holds_share()).unwrap_or(false)) {
            log!("head aim: the head's share left in the character (no update during stop)");
        }
        log!(
            "head aim: {} updates; vertical look inputs neutralised {}; baked yaw not found by game counter {} times",
            self.updates.load(Relaxed),
            self.pitch_blocked.load(Relaxed),
            self.baked_misses.load(Relaxed)
        );
        crate::aim::report_sources(settings.hand_aim);
    }

    /// The game's input-action converter got `value` for the action at `binding`: while head aim
    /// steers, a vertical look action (`is_pitch`) gets its neutral value (the head alone pitches
    /// the character); the value as given otherwise.
    pub fn neutral_pitch_if_live(&self, binding: usize, value: f32, is_pitch: impl Fn(u32) -> bool) -> f32 {
        if self.live()
            && let Some(neutral) = neutral_pitch(binding, is_pitch)
        {
            if value != neutral {
                self.pitch_blocked.fetch_add(1, Relaxed);
            }
            return neutral;
        }
        value
    }

    /// Vertical look inputs neutralised so far.
    pub fn pitch_blocked(&self) -> u64 {
        self.pitch_blocked.load(Relaxed)
    }
}
