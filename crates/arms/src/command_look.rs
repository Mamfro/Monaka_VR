//! Head aim through the game's input command as a ready-made component ([`look`]): the player
//! looks, aims and shoots where the head points, for a game whose command carries the view angles
//! the player's input accumulates (as `crate::mouse_look` steers through the mouse). A game lists
//! `monaka_arms::command_look::look()` among its components (with its own `aim` choices and what it
//! works on top of, by struct update), says once where its commands are built ([`ByCommand`]), and
//! from that hook hands each command's accumulated angles to [`command_built`] and writes back what
//! it returns.
//!
//! After the game has added the keyboard, mouse and stick look input into the accumulated angles,
//! the head's turn is added the same way, as a change: the accumulator is only meaningful up to the
//! game's own offset, so absolute values are never used. Each command holds the head's yaw and pitch
//! on top of the game's own (mouse and stick keep working), so movement, weapons and the game camera
//! all follow. The game camera then already carries the share of the command it was made from;
//! [`baked`] tells the eye site how much, to swap it for the current head pose.
//!
//! While another camera has the view (a cutscene, a takedown) the head is not put into the angles.
//! After a pause the share is held for the first commands ([`PAUSE_HOLD_COMMANDS`]): the game lines
//! its angles up with them. With only the head pitching (the pad's stick pitch taken) the player's
//! own pitch is brought back toward level a step at a time. Its option (not shown by the launcher):
//! `pause_hold=0` drops the share at once after a pause.

use monaka_core::aim::Levelling;
use monaka_core::feature::Feature;
use monaka_core::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Game, Lack, On, Run, Start};
use monaka_producer::{log, wait_until};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::time::{Duration, Instant};

#[cfg(test)]
mod lift_checks;
#[cfg(test)]
mod tests;

/// What a game's head aim through its input command reads of it.
pub struct ByCommand<G: Game> {
    /// Hooks where the game builds each command (its hook calls [`command_built`]) and what tells
    /// whether another camera has the view.
    pub install: fn(&mut Start<'_, G>) -> Result<(), Lack>,
    /// Whether another camera controls the view now: the head goes into no command meanwhile.
    pub camera_taken: fn() -> bool,
    /// The head pose now.
    pub head: fn() -> Option<HeadPose>,
}

/// A game whose look is steered through its input command.
pub trait LooksByCommand: Game {
    const COMMAND: ByCommand<Self>;
}

/// The head aim component for game `G`: on with head tracking and head aim.
pub const fn look<G: LooksByCommand>() -> Component<G> {
    Component {
        name: "look",
        features: &[Feature::Look],
        on: on::<G>,
        settings: |options, _| {
            options.switch("pause_hold", true);
        },
        install: install::<G>,
        arm,
        quiet: || AIMING.store(false, Release),
        give_back,
        summary: Some(summary),
        ..Component::NONE
    }
}

fn on<G: Game>(run: &Run<'_, G>) -> On {
    On::when(run.shared.head_tracking && run.shared.aim.head, "aim is mouse and stick only")
}

/// What the method reads of its game, set at install.
#[derive(Clone, Copy)]
struct GameFns {
    camera_taken: fn() -> bool,
    head: fn() -> Option<HeadPose>,
}

static GAME: Mutex<Option<GameFns>> = Mutex::new(None);

fn install<G: LooksByCommand>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    PAUSE_HOLD.store(start.options.switch("pause_hold", true), Release);
    *GAME.locked() = Some(GameFns { camera_taken: G::COMMAND.camera_taken, head: G::COMMAND.head });
    (G::COMMAND.install)(start)
}

/// Steering starts; while only the head pitches the view (the pad's stick pitch taken,
/// `monaka_pad::feature::pitch_from_head`), the player's own pitch is kept level.
fn arm() {
    LEVELLING.store(monaka_pad::feature::pitch_from_head(), Release);
    AIMING.store(true, Release);
}

/// The next command gives the game back its own angles.
fn give_back() {
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

/// The head's yaw and pitch held in a command, radians (yaw positive left, pitch positive up).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Share {
    pub yaw: f32,
    pub pitch: f32,
}

impl Share {
    /// As angle changes in the command's degrees: yaw turning left, pitch positive down.
    fn degrees(self) -> (f32, f32) {
        (self.yaw.to_degrees(), -self.pitch.to_degrees())
    }
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
pub fn baked() -> Option<Share> {
    let taken = GAME.locked().is_some_and(|game| (game.camera_taken)());
    (HOLDING.load(Acquire) && !taken).then(|| STATE.locked().held)
}

/// The accumulated angles (the command's degrees: pitch, yaw) with the held share replaced by
/// `next`.
pub fn reshare(pitch: f32, yaw: f32, held: Share, next: Share) -> (f32, f32) {
    let (held_yaw, held_pitch) = held.degrees();
    let (next_yaw, next_pitch) = next.degrees();
    (pitch - held_pitch + next_pitch, yaw - held_yaw + next_yaw)
}

/// The head's yaw and pitch (radians; yaw positive left, pitch positive up) from its orientation.
pub fn head_share(orientation: [f32; 4]) -> Share {
    let (yaw, pitch) = monaka_core::camera::pointing_yaw_pitch(orientation);
    Share { yaw, pitch }
}

/// What one command gets, given the share held and the moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Next {
    pub share: Share,
    /// The command resumes after a pause: the share is the one held.
    pub resuming: bool,
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
pub fn command_built(pitch: f32, yaw: f32) -> Option<(f32, f32)> {
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
    let resuming = state.holding_after_pause > 0;
    state.holding_after_pause = state.holding_after_pause.saturating_sub(1);
    let taken = (game.camera_taken)();
    let head = if aiming && !resuming && !taken { (game.head)().map(|pose| head_share(pose.orientation)) } else { None };
    let next = next_share(aiming, resuming, taken, state.held, head);
    let (mut pitch, yaw) = reshare(pitch, yaw, state.held, next);
    // Each view's measure is used once: the next view shows the change.
    let own = f32::from_bits(OWN_PITCH.swap(f32::NAN.to_bits(), Relaxed));
    if aiming && !resuming && LEVELLING.load(Relaxed) && !taken && own.is_finite() {
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
