//! The look delivered into the character's look targets ([`super::Delivery::Targets`]): the
//! character's own look targets follow the head, so it looks, aims and attacks where the head
//! points. At each player camera update the game's hook hands the character to [`steer`], with
//! where its targets are ([`LookFields`]).
//!
//! On each update the target pitch becomes the head's and the head's yaw is added to the target yaw
//! (minus what was added before), so mouse and stick turning keep accumulating on top; vertical look
//! input is neutralised meanwhile ([`super::neutral_pitch_if_live`], from the game's input
//! converter). The camera an update sets was built from the previous update's targets: the eyes are
//! built on it levelled, with that head yaw turned back out ([`super::baked`], recorded per game
//! frame). What was added is taken out again when VR stops ([`release`]). While another camera has
//! the player (the game's cutscene, as its look provider tells it) head aim stands down: the share
//! goes back and the game's camera and stick are its own.

use super::aim::{AimSettings, TOOL, aimed};
use super::{Looks, Share, StandDown};
use monaka_core::aim::{Convention, HeadAim, Look, PitchMode};
use monaka_core::camera::{Mat34, tracking_origin as levelled_origin};
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_hook::mem;
use monaka_producer::game::{Lack, Start};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};

#[cfg(test)]
mod tests;

/// Where a game's character keeps its look targets (floats at these offsets) and how its angles
/// run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookFields {
    pub yaw: usize,
    pub pitch: usize,
    pub convention: Convention,
}

impl LookFields {
    pub fn read(&self, character: usize) -> Option<Look> {
        Some(Look { yaw: mem::read::<f32>(character + self.yaw)?, pitch: mem::read::<f32>(character + self.pitch)? })
    }

    pub fn write(&self, character: usize, look: Look) {
        mem::write(character + self.yaw, look.yaw);
        mem::write(character + self.pitch, look.pitch);
    }
}

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

/// The steering: the targets' fields as the latest update gave them, and the head's share in them.
struct Steering {
    fields: LookFields,
    applied: HeadAim,
    /// The look written at the last update and the character it was written on.
    written: Option<(usize, Look)>,
    /// The look the game left over the one written at the last update, and the character.
    left: Option<(usize, Look)>,
    /// While the game holds the look itself ([`Steering::taken`]).
    taken: Option<Taken>,
}

/// The game holds `character`'s look itself. Each update a pitch too small to see (`probe`) is
/// written; the game writing its own look over it each frame shows it still holds it.
#[derive(Clone, Copy)]
struct Taken {
    character: usize,
    probe: Option<f32>,
    /// Probes in a row the game left as written.
    kept: u32,
}

/// A look head aim wrote and found otherwise at the next update moved by more than this (degrees):
/// the player's turning or the game. Vertical look input is held neutral while head aim steers, so
/// a pitch moved is the game's; a yaw moved may be either.
const TAKEN_PITCH: f32 = 0.5;
const TAKEN_YAW: f32 = 5.0;
/// Two looks the game left are the same within this (degrees): it holds a look of its own.
const HELD_SAME: f32 = 0.01;
/// The probe's pitch (degrees), and how many probes in a row the game must leave alone before
/// head aim steers again: a game holding a still look writes the same angles each frame, which
/// reading alone cannot tell from a game that let go.
const PROBE_PITCH: f32 = 0.05;
const PROBES_KEPT: u32 = 3;

/// Logs each change of whether the game holds the look itself (the first few); taken, with the look
/// it holds and the one head aim had written.
fn note_taken(taken: Option<(Look, Look)>) {
    static LAST: AtomicBool = AtomicBool::new(false);
    static CHANGES: AtomicU64 = AtomicU64::new(0);
    if LAST.swap(taken.is_some(), Relaxed) != taken.is_some() && CHANGES.fetch_add(1, Relaxed) < 40 {
        match taken {
            Some((held, written)) => log!("head aim: the game holds a look of its own (a scripted moment): {held:?} over the written {written:?}; standing down"),
            None => log!("head aim: the game let go of the look; steering"),
        }
    }
}

/// Whether `stored` is no longer the look written (`written`) at the last update.
fn written_over(stored: Look, written: Look) -> bool {
    (stored.pitch - written.pitch).abs() > TAKEN_PITCH || monaka_core::math::wrap_degrees(stored.yaw - written.yaw).abs() > TAKEN_YAW
}

/// Whether two looks the game left are the same: a look of its own it holds. A look the game
/// moves on from the one written (the player's turning, aim assist turning toward a target) is
/// another at each update.
fn same(a: Look, b: Look) -> bool {
    (a.pitch - b.pitch).abs() <= HELD_SAME && monaka_core::math::wrap_degrees(a.yaw - b.yaw).abs() <= HELD_SAME
}

impl Steering {
    /// Whether the game holds `character`'s look itself now (a scripted moment, a cutscene setting
    /// the look every frame): two updates in a row it left the same look over the one head aim
    /// wrote, and it still writes over the probes since. Then the head's share is gone from it, and
    /// only the probe is written. A look the game only moves on (aim assist, the player's turning)
    /// stays head aim's: what it moved is the character's own, as a turn is.
    fn taken(&mut self, character: usize, stored: Look) -> bool {
        if self.taken.is_some_and(|t| t.character != character) {
            self.taken = None;
        }
        let over = match self.written {
            Some((who, written)) if who == character && written_over(stored, written) => Some(written),
            _ => None,
        };
        let held = over.filter(|_| matches!(self.left, Some((who, left)) if who == character && same(left, stored)));
        self.left = over.map(|_| (character, stored));
        if let Some(written) = held {
            self.applied.forget();
            self.taken = Some(Taken { character, probe: None, kept: 0 });
            self.left = None;
            note_taken(Some((stored, written)));
        }
        self.written = None;
        let Some(taken) = &mut self.taken else {
            note_taken(None);
            return false;
        };
        match taken.probe {
            Some(probe) if probe.to_bits() == stored.pitch.to_bits() => taken.kept += 1,
            Some(_) => taken.kept = 0,
            None => {}
        }
        if taken.kept >= PROBES_KEPT {
            self.taken = None;
            note_taken(None);
            return false;
        }
        // Up and down by turns, so probes the game leaves do not add up.
        let probe = stored.pitch + if taken.kept % 2 == 0 { PROBE_PITCH } else { -PROBE_PITCH };
        mem::write(character + self.fields.pitch, probe);
        taken.probe = Some(probe);
        true
    }

    /// The head yaw (radians) the targets carry for `character` now.
    fn share(&self, character: usize) -> f32 {
        if character == self.applied.owner() { self.applied.yaw_share_radians(&self.fields.convention) } else { 0.0 }
    }

    /// Steers `character` toward `yaw` and `pitch` (radians, positive left and up). Returns the
    /// look written and the character's own (what it had without the head), or `None` when its
    /// stored angles are implausible (nothing written; a new player object starts clean).
    fn steer(&mut self, character: usize, yaw: f32, pitch: f32) -> Option<(Look, Look)> {
        if !yaw.is_finite() || !pitch.is_finite() {
            return None;
        }
        let stored = self.fields.read(character)?;
        if self.taken(character, stored) {
            return None;
        }
        let (write, own) = self.applied.apply(character, stored, yaw, pitch, PitchMode::HeadOnly, &self.fields.convention)?;
        self.fields.write(character, write);
        self.written = Some((character, write));
        Some((write, own))
    }

    /// Hands `character` its own look back; true when there was a share to hand back.
    fn release(&mut self, character: usize) -> bool {
        // What is in the targets from here on is not head aim's last write.
        self.written = None;
        self.left = None;
        if !self.applied.holds_share() {
            return false;
        }
        let Some(stored) = self.fields.read(character) else { return false };
        match self.applied.release(character, stored) {
            Some(own) => {
                self.fields.write(character, own);
                true
            }
            None => false,
        }
    }
}

static STEERING: Mutex<Steering> = Mutex::new(Steering { fields: LookFields { yaw: 0, pitch: 0, convention: Convention::DEGREES_YAW_RIGHT }, applied: HeadAim::NONE, written: None, left: None, taken: None });
/// While set, the next update hands the head's share back.
static RELEASING: AtomicBool = AtomicBool::new(false);
static UPDATES: AtomicU64 = AtomicU64::new(0);

/// What aims (the run's `aim` choice and the aim switches), then the game's look provider; the
/// controllers opened for a pointing hand.
pub(super) fn install<G: Looks>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let settings = AimSettings::from_options(start.options, start.shared.aim, true);
    super::configure(settings);
    RELEASING.store(false, Release);
    log!("{settings:?}");
    G::install(start)?;
    if settings.hand_aim {
        crate::controllers::open(start.channel()?);
    }
    Ok(())
}

/// The share the latest update's camera carries, while head aim steers.
pub(super) fn current_share() -> Option<f32> {
    let steering = STEERING.lock().ok()?;
    (super::live() && steering.applied.holds_share()).then(|| steering.share(steering.applied.owner()))
}

/// One player camera update: steers `character`, its targets at `fields`, for the next update
/// toward the head (or the aiming controller), recording the share this update's camera carries
/// under `counter` (the game frame, when the game numbers them). `aims_itself`: what is in hand
/// aims itself (a throw, a bow by hand), so the game's aim stays the head's. With no `head` (VR
/// stopped, no pose), while releasing, or while another camera has the player (`stand_down`),
/// hands the game's own angles and the left trigger back; `None` then.
pub fn steer(fields: LookFields, character: usize, head: Option<HeadPose>, counter: Option<u64>, stand_down: StandDown, aims_itself: impl FnOnce() -> bool) -> Option<Steered> {
    let settings = super::settings();
    let mut steering = STEERING.locked();
    steering.fields = fields;
    // The share this update's camera carries (written by the previous update).
    let baked = steering.share(character);
    super::record(counter, Some(Share { yaw: baked, pitch: 0.0 }));
    if super::stand_down_logged(stand_down) {
        log!("head aim: {}", if stand_down.now { "a cutscene has the player; standing down" } else { "the cutscene let go; steering" });
    }
    let head = head.filter(|h| h.valid != 0 && settings.head_aim && !RELEASING.load(Acquire) && !stand_down.now);
    if let Some(head) = head {
        let aim = aimed(&settings, monaka_channel::tick(), head.orientation, crate::controllers::palm, |_| settings.hand_rig && aims_itself());
        match steering.steer(character, aim.yaw, aim.pitch) {
            Some((written, own)) => {
                // The character's look follows the aim, the view the head: the game walks along
                // its look, so the stick is turned back to where the player looks.
                let head_yaw = monaka_core::camera::head_yaw_pitch(head.orientation).0;
                monaka_pad::turn_walk(monaka_core::math::wrap_radians(aim.yaw - head_yaw));
                let update = UPDATES.fetch_add(1, Relaxed);
                // A left trigger press waits for the left controller's aim.
                TOOL.gate(TOOL.possible(&settings), aim.tool, update);
                super::steered(monaka_channel::tick());
                if update == 0 {
                    log!("head aim: steering the character ({} aim; target look {:?})", if aim.by_hand { "hand" } else { "head" }, steering.fields.read(character));
                }
                return Some(Steered { written, own, baked, update, by_hand: aim.by_hand });
            }
            None => monaka_producer::log_first(3, format_args!("head aim: not steering: look {:?}, head yaw {:.2} pitch {:.2}", steering.fields.read(character), aim.yaw, aim.pitch)),
        }
    }
    TOOL.release();
    steering.release(character);
    None
}

/// The head yaw (radians) baked into the game camera of frame `counter` (the latest known when the
/// frame is not on record, or with no counter).
pub fn baked_at(counter: Option<u64>) -> f32 {
    super::recorded(counter).unwrap_or_default().yaw
}

/// The share the game camera of frame `counter` carries, to be turned back out of the view so a
/// head turn is not counted twice: [`baked_at`]'s while head aim steers, none otherwise.
pub(super) fn baked_in_view(counter: Option<u64>) -> Option<Share> {
    super::live().then(|| Share { yaw: baked_at(counter), pitch: 0.0 })
}

/// The tracking origin the eyes and arms are built on: the game camera levelled, the head yaw
/// baked into frame `counter` turned back out (while head aim steers), and moved by room-scale
/// following ([`crate::roomscale::shift_origin`]).
pub fn tracking_origin(game: &Mat34, counter: Option<u64>) -> Option<Mat34> {
    let mut origin = levelled_origin(game, baked_in_view(counter).map_or(0.0, |share| share.yaw))?;
    crate::roomscale::shift_origin(&mut origin);
    Some(origin)
}

/// Hands the head's share back at the next update (waits up to `timeout_ms` for it) and logs the
/// run's tally.
pub fn release(timeout_ms: u64) {
    let settings = super::settings();
    RELEASING.store(true, Release);
    TOOL.release();
    if settings.head_aim && !monaka_producer::wait_until(timeout_ms, || !STEERING.lock().map(|s| s.applied.holds_share()).unwrap_or(false)) {
        log!("head aim: the head's share left in the character (no update during stop)");
    }
    super::report(UPDATES.load(Relaxed));
}
