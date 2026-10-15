//! The look delivered by a facing the mod owns ([`super::Delivery::Facing`]): the look owns which
//! way the view faces; the right stick turns it, the view is it plus the head, at the game camera's
//! position, and the character's look targets are steered to it plus the head (or the aiming hand).
//! The game says where its look targets are and how they point ([`Facing`]), and at each player
//! camera update its hook hands the update, with the head pose of its frame, to [`steer`].
//!
//! Whatever else turns the character (a conversation facing the speaker, an attack's sway, a
//! climb-up) turns the character and its camera but never the view: a view made from the game
//! camera's direction moves with each of those, and they are too many to detect one by one. The
//! head yaw each update's camera carries beyond the facing is recorded per update
//! ([`super::baked`]), so the eyes turn it back out.
//!
//! While a cutscene has the player (as the game's look provider tells it,
//! [`super::Looks::stand_down`]) nothing is written: the view is the cutscene's camera plus the
//! head, and after it the facing goes on from its last framing. Climbing ([`crate::traverse`]), the
//! head turns the character through the right stick.

use super::aim::{AimSettings, TOOL, aimed};
use super::{Delivery, Looks, Share, StandDown};
use crate::hand_rig::HandRig;
use crate::hands::{self, At, In, Point};
use monaka_core::aim::Convention;
use monaka_core::camera::head_yaw_pitch;
use monaka_core::math::wrap_radians as wrapped;
use monaka_core::pad::{StickTurn, TurnLatch};
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_producer::game::{Lack, Start};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};

#[cfg(test)]
mod tests;

/// How a game takes the turn as a facing the mod owns: its look targets, read and written, and
/// where the player asks for a facing.
#[derive(Clone, Copy, Debug)]
pub struct Facing {
    /// Its look targets' units.
    pub units: TargetUnits,
    /// The character's target yaw now, if readable.
    pub read: fn(usize) -> Option<f32>,
    /// Sets the character's targets (yaw, pitch).
    pub write: fn(usize, f32, f32),
    /// A facing the player asked for at this update (a saved spot to come back to), given the
    /// moment, the character and the facing now (radians).
    pub command: fn(u64, usize, f32) -> Option<f32>,
}

/// No facing is ever asked for.
pub fn no_command(_now: u64, _character: usize, _facing: f32) -> Option<f32> {
    None
}

/// How a game's look targets point, in its own units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetUnits {
    /// Units per radian of yaw and of pitch, and the pitch limit.
    pub convention: Convention,
    /// The target yaw that looks along the world's yaw 0.
    pub yaw_zero: f32,
    /// A full turn of target yaw.
    pub turn: f32,
}

impl TargetUnits {
    /// The target yaw that turns the camera to look `look` radians in the world (positive left),
    /// kept nearest the target yaw `current` the character has: a player turned round in the room
    /// does not send it a full turn off (the game would turn the character all the way round).
    pub fn yaw(&self, current: f32, look: f32) -> f32 {
        let half = self.turn * 0.5;
        current + ((self.yaw_zero + look * self.convention.yaw_per_radian - current + half).rem_euclid(self.turn) - half)
    }

    /// The target pitch for a look `pitch` radians up, within the game's limit.
    pub fn pitch(&self, pitch: f32) -> f32 {
        let limit = self.convention.pitch_limit;
        (pitch * self.convention.pitch_per_radian).clamp(-limit, limit)
    }
}

/// Game `G`'s facing, where its look is by one.
fn facts<G: Looks>() -> Option<Facing> {
    match G::DELIVERY {
        Delivery::Facing(facing) => Some(facing),
        _ => None,
    }
}

/// What watches head aim's updates (a game's probes): each update before steering, and each
/// steering.
#[derive(Clone, Copy, Debug)]
pub struct Observer {
    pub traced: fn(&Traced),
    pub steered: fn(&Steered),
}

static OBSERVER: Mutex<Option<Observer>> = Mutex::new(None);

/// Has `observer` watch head aim's updates.
pub fn observe(observer: Observer) {
    *OBSERVER.locked() = Some(observer);
}

fn observer() -> Option<Observer> {
    *OBSERVER.locked()
}

/// What aims (the run's `aim` choice and the aim switches) and the stick's smooth turn speed, then
/// the game's look provider.
pub(super) fn install<G: Looks>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    configure(AimSettings::from_options(start.options, start.shared.aim, true), start.shared.stick_speed[0]);
    G::install(start)
}

/// What aims this run (`settings`) and the stick's smooth turn speed (degrees per second).
pub fn configure(settings: AimSettings, stick_speed: f32) {
    super::configure(settings);
    STICK_SPEED.store(stick_speed.to_bits(), Relaxed);
}

/// A player camera update, as the look provider's hook gets it.
#[derive(Clone, Copy, Debug)]
pub struct PlayerUpdate {
    pub character: usize,
    /// The look targets the camera was built from (the previous update's).
    pub yaw: f32,
    pub pitch: f32,
    /// The levelled camera's yaw (radians, positive left).
    pub camera_yaw: f32,
    /// The head pose the eyes of this update will be built with.
    pub head: HeadPose,
}

/// One update as head aim met it (degrees; `facing` none before head aim knows it).
#[derive(Clone, Copy, Debug)]
pub struct Traced {
    pub frame: u64,
    pub yaw: f32,
    pub pitch: f32,
    pub camera_yaw: f32,
    pub facing: Option<f32>,
    pub head_yaw: f32,
    pub cutscene: bool,
}

/// One steering update of head aim (angles in degrees; yaws positive left, the targets as the
/// character keeps them).
#[derive(Clone, Copy, Debug)]
pub struct Steered {
    pub frame: u64,
    pub camera_yaw: f32,
    pub facing: f32,
    pub aim_yaw: f32,
    pub aim_pitch: f32,
    /// The character's target yaw and pitch as the game left them, and as head aim wrote them.
    pub yaw: f32,
    pub target: f32,
    pub pitch: f32,
    pub target_pitch: f32,
    pub character: usize,
}

/// A climb under way. Yaws in the world: radians, positive left.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Climb {
    /// The game camera's yaw at the last update.
    camera: f32,
    /// The right stick head aim pushes (-1..=1, x right).
    push: f32,
    /// When the look was last traced.
    traced: u64,
}

/// Head aim's stick turns the game camera toward where the head looks once they are this far
/// apart (radians, 3 degrees)...
const PUSH_FROM: f32 = 0.052;
/// ...pushed fully this far apart (35 degrees)...
const PUSH_FULL: f32 = 0.61;
/// ...and never less than the stick's dead zone (XInput's recommended right-stick one, 8689).
const PUSH_DEAD_ZONE: f32 = 0.27;
/// The player's own stick turns past this much of its travel.
const STICK_TURNS: f32 = 0.25;
/// How old a stick read may be (ms), and the longest step of stick turning between two updates
/// (a stall must not spin the view).
const STICK_MAX_AGE_MS: u64 = 200;
const MAX_STEP_MS: u64 = 100;

/// The right stick that turns a camera `off` radians right of where the player looks (positive:
/// the look is left of it) toward the look: x right in -1..=1, 0 when close enough.
fn push_for(off: f32) -> f32 {
    if off.abs() < PUSH_FROM {
        return 0.0;
    }
    let amount = PUSH_DEAD_ZONE + (1.0 - PUSH_DEAD_ZONE) * ((off.abs() - PUSH_FROM) / (PUSH_FULL - PUSH_FROM)).min(1.0);
    -off.signum() * amount
}

/// The head yaw (radians) the camera of update `frame` carries, for the eyes.
fn record(frame: u64, yaw: f32) {
    super::record(Some(frame), Some(Share { yaw, pitch: 0.0 }));
}

struct State {
    character: usize,
    climb: Option<Climb>,
    /// Which way the view faces (radians in the world, positive left): head aim's own, turned only
    /// by the player's stick (climbing, by the stick's turn of the game camera).
    facing: Option<f32>,
    /// In a cutscene, the head's yaw as it began (radians): the cutscene's framing sits straight
    /// ahead of where the player faced then, and the head looks around in it.
    movie_head: Option<f32>,
    /// When the facing was last turned (`monaka_channel::tick`), for the stick's step, and the
    /// stick turn's latch.
    turned_at: u64,
    turn_latch: TurnLatch,
}

/// What a climbing update did.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Climbing {
    climb: Climb,
    /// The climb began at this update.
    began: bool,
    facing: f32,
    /// How far the game camera is from where the head looks (radians), and the player's own stick.
    off: f32,
    thumb: f32,
}

impl State {
    const fn new() -> Self {
        Self { character: 0, climb: None, facing: None, movie_head: None, turned_at: 0, turn_latch: TurnLatch::new() }
    }

    /// The stick's step since the last update (seconds, at most [`MAX_STEP_MS`]).
    fn stepped(&mut self, now: u64) -> f32 {
        let step = now.saturating_sub(self.turned_at).min(MAX_STEP_MS) as f32 / 1000.0;
        self.turned_at = now;
        step
    }

    /// An update of `character`: a new one (a load, a respawn) faces where its camera does; a
    /// cutscene beginning keeps the head's yaw, one ending hands the facing its last framing.
    fn meet(&mut self, character: usize, asked: StandDown, head_yaw: f32, camera_yaw: f32) {
        if self.character != character {
            self.character = character;
            self.facing = None;
            self.climb = None;
        }
        if asked.changed {
            if asked.now {
                self.movie_head = Some(head_yaw);
            } else if let Some(began) = self.movie_head.take() {
                // The view goes on from the cutscene's last framing.
                self.facing = Some(wrapped(camera_yaw - began));
            }
        }
    }

    /// A cutscene's update: nothing is written; the view is the cutscene's camera levelled, the
    /// head's turn as it began kept out, plus the head.
    fn hold_movie(&mut self, frame: u64, head_yaw: f32) {
        let began = *self.movie_head.get_or_insert(head_yaw);
        record(frame, began);
    }

    /// Climbing or hanging: the head pushes the right stick (`stick`, the player's own x) until
    /// the game's camera looks where the head does; the player's own stick turns the facing by what
    /// it turns the game's camera.
    fn climb(&mut self, frame: u64, camera_yaw: f32, facing: f32, aim_yaw: f32, stick: Option<f32>) -> Climbing {
        let began = self.climb.is_none();
        let mut climb = self.climb.unwrap_or(Climb { camera: camera_yaw, push: 0.0, traced: 0 });
        let turned = wrapped(camera_yaw - climb.camera);
        climb.camera = camera_yaw;
        let thumb = stick.map_or(0.0, |x| x - climb.push);
        let off = wrapped(facing + aim_yaw - camera_yaw);
        let mut facing = facing;
        if thumb.abs() > STICK_TURNS {
            // The player turns: the facing goes with the game camera, head aim lets go of the stick.
            facing = wrapped(facing + turned);
            climb.push = 0.0;
        } else {
            climb.push = push_for(off);
        }
        self.facing = Some(facing);
        record(frame, wrapped(camera_yaw - facing));
        Climbing { climb, began, facing, off, thumb }
    }

    /// Steering: the right stick (`stick`, x) turns the facing over `step` seconds; what the camera
    /// carries beyond the facing is recorded, to be turned back out.
    fn turn(&mut self, frame: u64, camera_yaw: f32, facing: f32, stick: Option<f32>, speed: f32, step: f32) -> f32 {
        let mut facing = facing;
        if let Some(x) = stick {
            facing = wrapped(facing - StickTurn::smooth(speed).turn(x, step, &mut self.turn_latch).to_radians());
        }
        self.facing = Some(facing);
        record(frame, wrapped(camera_yaw - facing));
        facing
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());
/// The stick's smooth turn speed (degrees per second, f32 bits), from the component's install.
static STICK_SPEED: AtomicU32 = AtomicU32::new(0);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static CLIMB_TRACES: AtomicU64 = AtomicU64::new(0);
static KNOWN: AtomicBool = AtomicBool::new(false);
static CUTSCENE: AtomicBool = AtomicBool::new(false);

fn stick_speed() -> f32 {
    f32::from_bits(STICK_SPEED.load(Relaxed))
}

/// Whether head aim steered the character within the last half second (not in a cutscene).
pub fn live() -> bool {
    !cutscene() && super::live()
}

/// Whether a cutscene had the player at the latest update.
pub fn cutscene() -> bool {
    CUTSCENE.load(Relaxed)
}

/// Whether the view is made with the share the camera carries ([`super::baked`]): once head aim
/// knows the facing, and in a cutscene (which may be under way when VR starts).
pub fn share_in_view() -> bool {
    KNOWN.load(Relaxed) || cutscene()
}

/// How many player camera updates head aim has steered.
pub fn updates() -> u64 {
    FRAMES.load(Relaxed)
}

/// Gives the right stick back to the game while head aim does not steer (a menu stops the
/// player's camera updates, a cutscene, a climb), and left trigger presses while it does not
/// steer at all: called every present.
pub fn keep_stick() {
    if !live() || crate::traverse::traversing() {
        monaka_pad::take_right_stick(false);
    }
    if !live() {
        TOOL.release();
    }
}

/// The player camera update `update` of game `G`: the facing turned and the character's look
/// targets steered to it plus the head (or hand).
pub fn steer<G: Looks + HandRig>(update: PlayerUpdate) {
    let PlayerUpdate { character, yaw, pitch, camera_yaw, head: pose } = update;
    let Some(facing_facts) = facts::<G>() else { return };
    if pose.valid == 0 {
        return;
    }
    // Hand aim: the controller's direction replaces the head's (`super::aim`); the left hand's tool
    // aims with the left controller, whatever the right hand holds.
    let settings = super::settings();
    let now = monaka_channel::tick();
    // The hands as sampled with the frame the head is: with the rig the weapon sits in the palm,
    // without it the controller's aim pose aims.
    let point = if settings.hand_rig { Point::Palm } else { Point::Controller };
    let aim = aimed(&settings, now, pose.orientation, |side| hands::pose::<G>(side, point, At::Frame, In::Tracking), |aim| crate::launch::aims_itself(aim.tool, settings.aim_hand));
    let (aim_yaw, aim_pitch, tool) = (aim.yaw, aim.pitch, aim.tool);
    let head_yaw = head_yaw_pitch(pose.orientation).0;
    let Ok(mut state) = STATE.lock() else { return };
    // This update's camera was built from the targets the previous update wrote.
    let frame = FRAMES.fetch_add(1, Relaxed) + 1;
    let step = state.stepped(now);
    let asked = G::stand_down(character);
    CUTSCENE.store(asked.now, Relaxed);
    state.meet(character, asked, head_yaw, camera_yaw);
    let movie = asked.now;
    if super::stand_down_logged(asked) {
        log!(
            "head aim: {} (the camera looks {:.1} degrees)",
            if movie { "a cutscene has the player; the view is its camera plus the head" } else { "the cutscene let go; steering" },
            camera_yaw.to_degrees()
        );
    }
    let watch = observer();
    let traced = Traced { frame, yaw, pitch, camera_yaw: camera_yaw.to_degrees(), facing: state.facing.map(f32::to_degrees), head_yaw: head_yaw.to_degrees(), cutscene: movie };
    if let Some(watch) = watch {
        (watch.traced)(&traced);
    }
    // A left trigger press waits for the left controller's aim; climbing and in a cutscene nothing
    // waits (`super::aim::ToolAim`).
    TOOL.gate(!movie && TOOL.possible(&settings), tool, frame);
    if movie {
        state.hold_movie(frame, head_yaw);
        return;
    }
    let mut facing = *state.facing.get_or_insert(camera_yaw);
    KNOWN.store(true, Relaxed);
    if let Some(asked) = (facing_facts.command)(now, character, facing) {
        facing = asked;
    }
    // Climbing or hanging, the game turns the character by where it looks (letting go with one
    // hand to reach back, taking hold again) and fights look angles written into it: those kick
    // the character back. The stick turns it there cleanly, so the head goes in as the stick.
    if crate::traverse::traversing() {
        monaka_pad::take_right_stick(false);
        let climbing = state.climb(frame, camera_yaw, facing, aim_yaw, monaka_pad::right_stick(100).map(|s| s[0]));
        let Climbing { mut climb, began, facing, off, thumb } = climbing;
        if began {
            log!("head aim: climbing; the head turns the character through the right stick");
        }
        monaka_pad::press("climb", monaka_contract::gamepad::Gamepad { thumb_rx: (climb.push * 32767.0) as i16, ..Default::default() }, 100);
        // Still live: the arms and the vertical look input go on as while steering.
        super::steered(now);
        if now.saturating_sub(climb.traced) >= 100 && CLIMB_TRACES.fetch_add(1, Relaxed) < 1500 {
            climb.traced = now;
            log!(
                "  climb look: camera yaw {:.1}, the head looks {:.1} (facing {:.1} + head {:.1}): {:.1} apart, stick pushed {:.2}, the player's {thumb:.2}",
                camera_yaw.to_degrees(),
                (facing + aim_yaw).to_degrees(),
                facing.to_degrees(),
                aim_yaw.to_degrees(),
                off.to_degrees(),
                climb.push
            );
        }
        state.climb = Some(climb);
        return;
    }
    if state.climb.take().is_some() {
        monaka_pad::let_go("climb");
        log!("head aim: the climb ended");
    }
    // The right stick turns the facing; the game reads it centred.
    monaka_pad::take_right_stick(true);
    let stick = monaka_pad::right_stick(STICK_MAX_AGE_MS).map(|s| s[0]);
    let facing = state.turn(frame, camera_yaw, facing, stick, stick_speed(), step);
    // The character looks where the facing plus the head (or hand) does.
    let target = facing_facts.units.yaw(yaw, wrapped(facing + aim_yaw));
    let target_pitch = facing_facts.units.pitch(aim_pitch);
    (facing_facts.write)(character, target, target_pitch);
    super::steered(now);
    let steered = Steered {
        frame,
        camera_yaw: camera_yaw.to_degrees(),
        facing: facing.to_degrees(),
        aim_yaw: aim_yaw.to_degrees(),
        aim_pitch: aim_pitch.to_degrees(),
        yaw,
        target,
        pitch,
        target_pitch,
        character,
    };
    if let Some(watch) = watch {
        (watch.steered)(&steered);
    }
}

/// Hands game `G`'s character a look without the head (end of a run): the facing, level.
pub fn release<G: Looks>() {
    monaka_pad::take_right_stick(false);
    TOOL.release();
    let Some(facing_facts) = facts::<G>() else { return };
    let Ok(state) = STATE.lock() else { return };
    let (character, Some(facing)) = (state.character, state.facing) else { return };
    if cutscene() {
        return;
    }
    let Some(yaw) = (facing_facts.read)(character) else { return };
    (facing_facts.write)(character, facing_facts.units.yaw(yaw, facing), 0.0);
}

/// Logs where the aim came from (end of a run).
pub fn report() {
    super::aim::report_sources(super::settings().hand_aim);
}
