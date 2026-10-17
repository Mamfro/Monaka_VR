//! The look delivered through the game's input command ([`super::Delivery::Command`]): the player
//! looks, aims and shoots where the head points, for a game whose command carries the view angles
//! the player's input accumulates. The game says where its commands are built (its
//! [`super::Looks::install`]), and from that hook hands each command's accumulated angles to
//! [`built`] and writes back what it returns.
//!
//! After the game has added the keyboard, mouse and stick look input into the accumulated angles,
//! the head's turn is added the same way, as a change: the accumulator is only meaningful up to the
//! game's own offset, so absolute values are never used. Each command holds the head's yaw and pitch
//! on top of the game's own (mouse and stick keep working), so movement, weapons and the game camera
//! all follow. The game camera then already carries the share of the command it was made from;
//! [`super::baked`] tells the eye site how much, to swap it for the current head pose.
//!
//! While another camera has the view (a cutscene, a takedown: [`super::Looks::stand_down`]) the
//! head is not put into the angles: the share goes out. It stays as it was while the game says so
//! (the delivery's `holds`): a camera that turns by the angles' changes, or a menu during which the
//! game leaves its camera's angles alone (the camera keeps the share it had, so the eye site turns
//! that share back out and the head looks around). After a pause the share is held for the first commands
//! ([`PAUSE_HOLD_COMMANDS`]): the game lines its angles up with them. With only the head pitching
//! (the pad's stick pitch taken) the player's own pitch is brought back toward level a step at a
//! time, where the game lets it (the delivery's `levels`). Its option (not shown by the launcher):
//! `pause_hold=0` drops the share at once after a pause.

use super::{Delivery, Looks, Pitch, Share};
use monaka_core::aim::Levelling;
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_producer::game::{Lack, Start};
use monaka_producer::{log, wait_until};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// What the delivery reads of its game, set at install.
#[derive(Clone, Copy)]
struct GameFns {
    camera_taken: fn() -> bool,
    head: fn() -> Option<HeadPose>,
    holds: fn() -> bool,
    pitch: Pitch,
}

static GAME: Mutex<Option<GameFns>> = Mutex::new(None);

/// Whether another camera has the view of game `G` now.
fn camera_taken<G: Looks>() -> bool {
    G::stand_down(0).now
}

/// No head pose (a game whose delivery is not the command's).
fn no_head() -> Option<HeadPose> {
    None
}

/// The share never held while another camera has the view.
fn never() -> bool {
    false
}

pub(super) fn install<G: Looks>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    PAUSE_HOLD.store(start.options.switch("pause_hold", true), Release);
    let (head, holds, pitch) = match G::DELIVERY {
        Delivery::Command { head, holds, pitch, .. } => (head, holds, pitch),
        _ => (no_head as fn() -> Option<HeadPose>, never as fn() -> bool, Pitch::Own),
    };
    *GAME.locked() = Some(GameFns { camera_taken: camera_taken::<G>, head, holds, pitch });
    G::install(start)
}

/// Steering starts; while only the head pitches the view (the pad's stick pitch taken,
/// `monaka_pad::feature::pitch_from_head`), the player's own pitch is kept level where game `G`
/// says it may be (its delivery's `levels`).
pub(super) fn arm<G: Looks>() {
    LEVELLING.store(levels::<G>() && monaka_pad::feature::pitch_from_head(), Release);
    AIMING.store(true, Release);
}

/// Whether game `G` lets the player's own pitch be kept level now.
fn levels<G: Looks>() -> bool {
    match G::DELIVERY {
        Delivery::Command { levels, .. } => levels(),
        _ => true,
    }
}

/// The stop's first step: the head goes into no more commands.
pub(super) fn quiet() {
    AIMING.store(false, Release);
}

/// The next command gives the game back its own angles.
pub(super) fn give_back() {
    if !wait_until(1000, || !HOLDING.load(Acquire)) {
        log!("no command while stopping; the head's last aim stays in the view angles");
    }
}

/// Set from the arm to the stop's first step: the head goes into the commands.
static AIMING: AtomicBool = AtomicBool::new(false);
/// Set while a head share may still be held in the angles.
static HOLDING: AtomicBool = AtomicBool::new(false);
/// Set while only the head pitches: the player's own pitch is then kept level. A seat's limits or
/// a cutscene can leave it tilted, and nothing else would ever bring it back.
static LEVELLING: AtomicBool = AtomicBool::new(false);
/// The player's own pitch (degrees, positive down: the game camera's without the head's share),
/// measured at the last view build; NaN once used or unknown.
static OWN_PITCH: AtomicU32 = AtomicU32::new(f32::NAN.to_bits());
static LEVELLED: AtomicU64 = AtomicU64::new(0);
/// Levelled only while the head is within this of level (degrees), well inside any seat's limits,
/// so the game camera carries the whole share; and by at most this much per command.
const LEVEL_HEAD_RANGE: f32 = 30.0;
const LEVEL_STEP: f32 = 0.25;
/// The player's own pitch (degrees, positive down) brought back toward level by at most
/// [`LEVEL_STEP`] a command (a change to the accumulated pitch); within 0.05 degrees it is level.
pub const LEVEL: Levelling = Levelling { dead_band: 0.05, step: LEVEL_STEP };

/// A share as angle changes in the command's degrees: yaw turning left, pitch positive down.
fn degrees(share: Share) -> (f32, f32) {
    (share.yaw.to_degrees(), -share.pitch.to_degrees())
}

struct State {
    /// What the angles hold now, on top of the game's own.
    held: Share,
    commands: u64,
    aimed: u64,
    /// When the last command was built, and how many more commands keep `held` after a pause.
    last_command: Option<Instant>,
    holding_after_pause: u32,
    pauses: u64,
}

static STATE: Mutex<State> = Mutex::new(State { held: Share { yaw: 0.0, pitch: 0.0 }, commands: 0, aimed: 0, last_command: None, holding_after_pause: 0, pauses: 0 });
/// The head pose whose share the latest command carries (none: its share is not the head's now:
/// held over a pause, another camera's view, not aiming).
static BAKED_HEAD: Mutex<Option<HeadPose>> = Mutex::new(None);

/// The head pose whose share the latest command carries: what a frame built from that command's
/// camera is drawn from. A game that draws the eyes from it (rather than a newer head sample) sends
/// each frame with the pose its picture follows, whichever of the camera's turn and its own the
/// engine draws with.
pub fn head_baked() -> Option<HeadPose> {
    *BAKED_HEAD.locked()
}

/// Set while the share is kept for the first commands after a pause ([`PAUSE_GAP`]).
static PAUSE_HOLD: AtomicBool = AtomicBool::new(true);
/// No command for this long: the game was paused (a menu, a journal) or loading. On resuming it
/// lines its view angles up with the first commands' (anything they changed since is kept as an
/// offset), so those keep the share they had before the pause; the head's turn since then follows
/// as an ordinary change.
pub const PAUSE_GAP: Duration = Duration::from_millis(150);
pub const PAUSE_HOLD_COMMANDS: u32 = 3;

/// The run's counts.
pub fn summary() -> String {
    let state = STATE.locked();
    format!("{} commands, {} aimed by the head, {} levelled, {} pauses with the share held over", state.commands, state.aimed, LEVELLED.load(Relaxed), state.pauses)
}

/// The game camera's pitch (degrees, positive up) at a view build carrying `baked`: what the
/// player's own pitch is without the head.
pub fn note_camera_pitch(pitch_up: f32, baked: Share) {
    OWN_PITCH.store(own_pitch(pitch_up, baked).to_bits(), Relaxed);
}

/// The player's own pitch (degrees, positive down) under a game camera pitched `pitch_up` (degrees)
/// carrying `baked`; NaN when the head is too far from level for the camera to carry its share.
pub fn own_pitch(pitch_up: f32, baked: Share) -> f32 {
    let own_down = -(pitch_up - baked.pitch.to_degrees());
    let usable = own_down.is_finite() && baked.pitch.to_degrees().abs() < LEVEL_HEAD_RANGE;
    if usable { own_down } else { f32::NAN }
}

/// The head share baked into the game camera being built now: the latest command's (commands and
/// views are built in turn on the game thread). None while another camera controls the view: that
/// camera carries none.
pub(super) fn baked() -> Option<Share> {
    let taken = GAME.locked().is_some_and(|game| (game.camera_taken)());
    (HOLDING.load(Acquire) && !taken).then(|| STATE.locked().held)
}

/// The accumulated angles (the command's degrees: pitch, yaw) with the held share replaced by
/// `next`.
pub fn reshare(pitch: f32, yaw: f32, held: Share, next: Share) -> (f32, f32) {
    let (held_yaw, held_pitch) = degrees(held);
    let (next_yaw, next_pitch) = degrees(next);
    (pitch - held_pitch + next_pitch, yaw - held_yaw + next_yaw)
}

/// The head's yaw and pitch (radians; yaw positive left, pitch positive up) from its orientation.
pub fn head_share(orientation: [f32; 4]) -> Share {
    let (yaw, pitch) = monaka_core::camera::pointing_yaw_pitch(orientation);
    Share { yaw, pitch }
}

/// The share command `held` goes on to: the one held while resuming after a pause, the head's
/// while aiming (`head` when tracked) and no other camera has the view, none otherwise.
pub fn next_share(aiming: bool, resuming: bool, camera_taken: bool, held: Share, head: Option<Share>) -> Share {
    if resuming && aiming {
        held
    } else if aiming && !camera_taken {
        head.unwrap_or(held)
    } else {
        Share::default()
    }
}

/// After the game built a command: its accumulated angles (the command's degrees: `pitch` positive
/// down, `yaw` turning left), with the player's own input in. The angles to write back (the head's
/// share in place of the one held), or none while the head neither aims nor holds a share.
pub fn built(pitch: f32, yaw: f32) -> Option<(f32, f32)> {
    let aiming = AIMING.load(Acquire);
    if !aiming && !HOLDING.load(Acquire) {
        return None;
    }
    if !pitch.is_finite() || !yaw.is_finite() {
        return None;
    }
    let game = (*GAME.locked())?;
    let mut state = STATE.locked();
    state.commands += 1;
    let now = Instant::now();
    if state.last_command.is_some_and(|last| now.duration_since(last) > PAUSE_GAP) && PAUSE_HOLD.load(Relaxed) {
        state.holding_after_pause = PAUSE_HOLD_COMMANDS;
        state.pauses += 1;
    }
    state.last_command = Some(now);
    let taken = (game.camera_taken)();
    // Kept as it was: resuming after a pause, or the game says so (a camera that turns by the
    // angles' changes, a menu meanwhile leaving the camera's angles as they were).
    let resuming = state.holding_after_pause > 0 || (game.holds)();
    state.holding_after_pause = state.holding_after_pause.saturating_sub(1);
    let sampled = if aiming && !resuming && !taken { (game.head)() } else { None };
    *BAKED_HEAD.locked() = sampled;
    let head = sampled.map(|pose| head_share(pose.orientation));
    let next = next_share(aiming, resuming, taken, state.held, head);
    let (mut pitch, yaw) = reshare(pitch, yaw, state.held, next);
    // The head's pitch alone: the game's own pitch input since the last command dropped.
    if game.pitch == Pitch::HeadAlone && aiming && !taken {
        pitch = degrees(next).1;
    }
    // Each view's measure is used once: the next view shows the change.
    let own = f32::from_bits(OWN_PITCH.swap(f32::NAN.to_bits(), Relaxed));
    if game.pitch == Pitch::Own && aiming && !resuming && LEVELLING.load(Relaxed) && !taken && own.is_finite() {
        let change = LEVEL.step(own);
        if change != 0.0 {
            pitch += change;
            LEVELLED.fetch_add(1, Relaxed);
        }
    }
    state.held = next;
    if aiming {
        state.aimed += 1;
        HOLDING.store(true, Release);
    } else {
        log!("head aim released");
        HOLDING.store(false, Release);
    }
    Some((pitch, yaw))
}
