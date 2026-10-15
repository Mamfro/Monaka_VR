//! The mouse delivery's steering, for games whose look angles are out of reach: the head's turn (or
//! a controller's direction) goes in as mouse counts ([`monaka_pad::mouse`]), so the game's aim
//! and interaction follow it like any mouse turn; the mouse and stick keep working on top. The eye
//! site calls [`super::on_build`] once a frame with the game camera's angles and swaps the share
//! the camera carries for the head pose it draws.
//!
//! The counts per degree are measured ([`Calibration`]): the mouse is moved by a known amount on
//! each axis and the game camera's turn is read at the next builds, then moved back. A game with a
//! known scale starts from it and measures only when the aim does not follow. After that the
//! game's aim is steered, not predicted: the body has a direction of its own ([`State::body`]:
//! yaw, and pitch kept level), each build the camera's real angles are compared with body plus
//! aim and the difference (less the counts still in flight) is queued as mouse counts. The view is
//! body plus head whatever the game made of the counts, so the world stays locked: a game may turn
//! a little more per count on fast turns (mouse acceleration or smoothing; Dishonored 2 does), which
//! would slip the world by degrees if the share were predicted from the counts.
//!
//! What aims: the head; or with a pointer ([`MouseAim::pointer`]) one controller all the time, the
//! view staying the head's (the viewer's ring on that controller shows where it points); or with
//! power aim, the left controller while the left trigger is held (the left hand's power in
//! Dishonored: Blink, Far Reach, Domino...) and a little after it is let go.
//!
//! A game may weaken its stick look while mouse counts come in, so once calibrated the right
//! stick is taken from the pad ([`monaka_pad::take_right_stick`]) and turns the body instead
//! ([`StickTurn`]): the game sees one look device.

use monaka_core::camera::{direction_yaw_pitch, pointing_yaw_pitch};
use monaka_core::pad::{StickTurn, TurnLatch};
use monaka_contract::protocol::{HeadPose, LEFT_HAND};
use monaka_pad::mouse;
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::*};
use std::time::Instant;
use monaka_core::sync::Locked;

/// Set while the head aims; cleared at stop, after which [`release`] gives the game back its own
/// view.
pub(super) static AIMING: AtomicBool = AtomicBool::new(false);

/// Counts moved per axis to measure the scale.
const PROBE_COUNTS: f64 = 300.0;
/// Builds to wait after a probe's counts are read before measuring (the game may smooth).
const SETTLE_BUILDS: u32 = 12;
/// Mouse-reading builds before a calibration step gives up (counts read but the camera never
/// settles). Builds without a mouse read (the game window without focus) do not count.
const STEP_TIMEOUT: u32 = 400;
/// Calibrations tried before head aim gives up (each fails when the mouse moves meanwhile).
const ATTEMPTS: u32 = 3;
/// Degrees a probe must turn the camera to count: less means the game read the counts without
/// turning (window focus arriving, a menu), which is retried without using up an attempt.
const NO_TURN: f32 = 0.5;
/// Builds to wait before trying again after probes that turned nothing (about a second).
const RETRY_BUILDS: u32 = 100;
/// Degrees per count believed: anything else is a failed measurement (the player moved the mouse).
const PLAUSIBLE_SCALE: std::ops::RangeInclusive<f32> = 0.005..=1.0;
/// The two axes' scales within this factor of each other (a good calibration gives about the same
/// on both; a probe the player's own turning runs into gives scales far apart).
const PLAUSIBLE_RATIO: f32 = 3.0;
/// The aim does not follow: the game turned the player by itself this often within this long.
/// (An aim that stays far off is no sign: at every start the game reads the mouse for a while
/// without turning.)
const STORM_RESYNCS: usize = 3;
const STORM_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
/// How old a stick read may be.
const STICK_MAX_AGE_MS: u64 = 200;
/// The longest step of stick turning between two builds (a stall must not spin the view).
const MAX_STEP_SECONDS: f32 = 0.1;
/// Share of the aim's error queued per build: the game may show counts a build or two after it
/// reads them, so a full correction each build would overshoot.
const STEER_GAIN: f32 = 0.3;
/// The pitch the aim is steered to stays inside the game's limits (degrees).
const PITCH_LIMIT: f32 = 80.0;
/// An aim error this large (degrees) that is not still in flight is the game turning the player
/// itself (a scripted turn, a teleport): the body takes the game's direction instead of fighting it.
const RESYNC: f32 = 45.0;
/// Builds after a jump of the aim's target (head to hand and back) during which a large error is
/// the steering catching up, not the game turning the player.
const JUMP_GRACE: u32 = 60;
const JUMP: f32 = 10.0;
/// The left trigger held at least this far aims the power with the left hand, until this long
/// after it is let go.
const POWER_TRIGGER: f32 = 30.0 / 255.0;
/// While a left trigger press is held back from the game, the aim is steered harder and the
/// press goes through once the game's aim is this close to the hand's (degrees).
const GATE_GAIN: f32 = 0.6;
const GATE_CLOSE: f32 = 2.0;
const POWER_HOLD: std::time::Duration = std::time::Duration::from_millis(400);

/// How a run aims ([`start`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct MouseAim {
    /// Degrees of yaw (left positive) per x count and of pitch (up positive) per y count, from the
    /// options: no calibration is run.
    pub scale: Option<[f32; 2]>,
    /// The game's usual scale (measured at its default sensitivity): steering starts from it at
    /// once, and the calibration runs only when the aim does not follow (the sensitivity changed, Y
    /// inverted). The aim is steered, so a scale somewhat off only slows it a little, and the start
    /// needs no probes twitching the view. `None`: measured at the start.
    pub usual_scale: Option<[f32; 2]>,
    /// How the right stick turns the body, yaw and pitch (a pitch at speed 0 never pitches it), or
    /// with `native_stick` the game turns with it (stick pitch is never the game's).
    pub stick_turn: [StickTurn; 2],
    pub native_stick: bool,
    /// The left controller aims while the left trigger is held ([`power_hand`]); `rig`: the arms
    /// are on the controllers (the palm's line converges with the game's aim ray).
    pub power_aim: bool,
    pub rig: bool,
    /// This controller (`LEFT_HAND`, `RIGHT_HAND`) aims all the time instead of the head.
    pub pointer: Option<usize>,
    /// Whether a weapon is in the left hand, for the power aim's log.
    pub left_weapon: Option<fn() -> bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Calibration {
    /// Waiting for the view to settle after attaching (or after probes that turned nothing).
    Settling { builds: u32 },
    /// Probe counts queued on `axis` (0 x, 1 y) from `angle`; `target` total delivered counts on
    /// that axis mean they were read.
    Probing { axis: usize, angle: f32, target: i64, builds: u32, read_at: Option<u32> },
    /// Probe counts taken back; waiting for them to be read.
    Returning { axis: usize, target: i64, builds: u32 },
    Done,
    Failed,
}

struct State {
    calibration: Calibration,
    /// The game's usual scale, the pointing controller and the left weapon test ([`MouseAim`]).
    usual: Option<[f32; 2]>,
    pointer: Option<usize>,
    left_weapon: Option<fn() -> bool>,
    /// Degrees of yaw (left positive) per x count and of pitch (up positive) per y count; a
    /// game's own sensitivity and inversion included.
    scale: [f32; 2],
    /// The body's direction (yaw, pitch; degrees): where the head's zero points in the game.
    /// Set when the calibration ends; the stick turns it.
    body: Option<[f32; 2]>,
    /// The head's yaw and pitch (degrees) at the last build, and what the aim was steered to
    /// relative to the body (the head's, or the left hand's): taken back out at release.
    head: [f32; 2],
    aimed: [f32; 2],
    /// Power aim: on, with the hand rig (the palm's line converges with the game's aim ray), and
    /// until when the left hand aims.
    power_aim: bool,
    rig: bool,
    power_until: Option<Instant>,
    /// The last target, builds left of the grace after a jump, and builds aimed by the hand.
    last_target: Option<[f32; 2]>,
    grace: u32,
    hand_builds: u64,
    /// The trigger at the last build, and presses logged.
    trigger_held: bool,
    presses: u32,
    /// When the game last turned the player by itself.
    resyncs: Vec<Instant>,
    /// Delivered counts at start: a calibration cut short is taken back at release.
    start: [i64; 2],
    /// Mouse reads at the last build, and whether the wait for them has been logged.
    reads: u64,
    waiting_logged: bool,
    attempts: u32,
    /// Probes the game read without turning, and how many builds a settle lasts.
    no_turns: u32,
    settle_builds: u32,
    /// Stick turning (yaw and pitch), a snap's latch, and the last build's time.
    stick_turn: [StickTurn; 2],
    latch: TurnLatch,
    last_build: Option<Instant>,
    /// The game turns with the stick itself ([`take_stick`]); while the stick is pushed (and until
    /// [`NATIVE_SETTLE`] after) the body follows the game's turn. `stick_push`: since when, and
    /// the camera's yaw then, for the log's turn rate.
    native_stick: bool,
    native_until: Option<Instant>,
    stick_push: Option<(Instant, f32)>,
    pushes: u32,
}

static STATE: Mutex<State> =
    Mutex::new(State {
    calibration: Calibration::Settling { builds: 0 },
    usual: None,
    pointer: None,
    left_weapon: None,
    scale: [0.0; 2],
    body: None,
    head: [0.0; 2],
    aimed: [0.0; 2],
    power_aim: false,
    rig: false,
    power_until: None,
    last_target: None,
    grace: 0,
    hand_builds: 0,
    trigger_held: false,
    presses: 0,
    resyncs: Vec::new(),
    start: [0; 2],
    reads: 0,
    waiting_logged: false,
    attempts: 0,
    no_turns: 0,
    settle_builds: 20,
    stick_turn: [StickTurn::smooth(0.0); 2],
    latch: TurnLatch::new(),
    last_build: None,
    native_stick: false,
    native_until: None,
    stick_push: None,
    pushes: 0,
});

/// With the game's own stick turning: how long after the stick is let go the game may still be
/// turning (its stick look eases out).
const NATIVE_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

/// Gives the right stick to head aim (it turns the body) or, with `native`, its turning to the game
/// (stick pitch is never the game's: the head alone pitches).
fn take_stick(native: bool) {
    monaka_pad::take_right_stick(!native);
    monaka_pad::take_right_stick_y(native);
}

/// Gives the whole right stick back to the game.
fn give_stick() {
    monaka_pad::take_right_stick(false);
    monaka_pad::take_right_stick_y(false);
}

/// Starts steering the game's aim (see [`MouseAim`]).
pub(super) fn start(aim: MouseAim) {
    let MouseAim { scale, usual_scale, stick_turn, native_stick, power_aim, rig, pointer, left_weapon } = aim;
    let mut state = STATE.locked();
    state.usual = usual_scale;
    state.pointer = pointer;
    state.left_weapon = left_weapon;
    state.stick_turn = stick_turn;
    state.latch = TurnLatch::new();
    state.native_stick = native_stick;
    state.native_until = None;
    state.stick_push = None;
    state.power_aim = power_aim;
    state.rig = rig;
    state.power_until = None;
    state.last_target = None;
    state.grace = 0;
    state.aimed = [0.0; 2];
    state.last_build = None;
    state.body = None;
    state.head = [0.0; 2];
    state.start = mouse::delivered();
    state.reads = mouse::reads();
    state.waiting_logged = false;
    state.attempts = 1;
    state.no_turns = 0;
    state.settle_builds = 20;
    state.resyncs.clear();
    match scale.or(usual_scale) {
        Some(known) => {
            state.scale = known;
            state.calibration = Calibration::Done;
            take_stick(native_stick);
            monaka_pad::gate_left_trigger(power_aim);
            if scale.is_some() {
                log!("head aim: mouse scale {known:?} degrees per count (from the options)");
            } else {
                log!("head aim: the usual mouse scale {known:?} degrees per count; measured if the aim does not follow");
            }
        }
        None => {
            // The stick stays the game's until the scale is known.
            state.scale = [0.0; 2];
            state.calibration = Calibration::Settling { builds: 0 };
            log!("head aim: measuring the mouse scale (keep the mouse still for a moment)");
        }
    }
    if let Some(side) = pointer {
        log!("aim: the {} controller aims", if side == LEFT_HAND { "left" } else { "right" });
    }
    mouse::ACTIVE.store(true, Release);
    AIMING.store(true, Release);
}

pub(super) fn summary() -> String {
    let state = STATE.locked();
    format!(
        "head aim: {:?}, scale {:?} degrees per count, {} builds aimed by the left hand, {}",
        state.calibration,
        state.scale,
        state.hand_builds,
        mouse::summary()
    )
}

/// `a - b` in degrees, wrapped to -180..180.
fn turned(a: f32, b: f32) -> f32 {
    monaka_core::math::wrap_degrees(a - b)
}

/// Once per view build, on the game thread, with the game camera's angles (degrees) before any
/// change and the head pose the frame is drawn from. Advances the calibration or queues the
/// head's counts. Returns the head share the game camera carries (yaw, pitch; radians) once head
/// aim runs: the camera's angles less the body's, so the view (camera, share swapped for the
/// head) is exactly body plus head. The body's pitch is level unless the stick pitches it (a
/// leftover pitch tilts the world sideways when looking aside).
pub(super) fn build(camera: [f32; 2], pose: &HeadPose) -> Option<[f32; 2]> {
    if !AIMING.load(Acquire) {
        return None;
    }
    let mut state = STATE.locked();
    // A release that came in since the check above has taken the share back: queue nothing more.
    if !AIMING.load(Acquire) {
        return None;
    }
    let delivered = mouse::delivered();
    let reads = mouse::reads();
    let reading = reads != state.reads;
    state.reads = reads;
    let calibrating = !matches!(state.calibration, Calibration::Done | Calibration::Failed);
    if calibrating && !reading {
        // The game is not reading its mouse (no focus): the calibration waits.
        if !state.waiting_logged {
            state.waiting_logged = true;
            log!("head aim: waiting for the game to read the mouse (click into the game window; keep the mouse still)");
        }
        return None;
    }
    state.calibration = match state.calibration {
        Calibration::Settling { builds } if builds < state.settle_builds => Calibration::Settling { builds: builds + 1 },
        Calibration::Settling { .. } => probe(0, camera, delivered),
        Calibration::Probing { axis, angle, target, builds, read_at } => {
            let read_at = read_at.or((delivered[axis] == target).then_some(builds));
            match read_at {
                Some(at) if builds >= at + SETTLE_BUILDS => {
                    let moved = turned(camera[axis], angle);
                    let scale = (moved as f64 / PROBE_COUNTS) as f32;
                    log!("head aim: {} probe turned the camera {moved:.2} degrees for {PROBE_COUNTS} counts", ["yaw", "pitch"][axis]);
                    state.scale[axis] = scale;
                    let mut back = [0.0; 2];
                    back[axis] = -PROBE_COUNTS;
                    mouse::add(back[0], back[1]);
                    Calibration::Returning { axis, target: delivered[axis] - PROBE_COUNTS as i64, builds: 0 }
                }
                _ if builds > STEP_TIMEOUT => fail("the probe counts were read but the camera never settled"),
                _ => Calibration::Probing { axis, angle, target, builds: builds + 1, read_at },
            }
        }
        Calibration::Returning { axis, target, builds } => {
            if delivered[axis] != target {
                if builds > STEP_TIMEOUT { fail("the return counts were never read") } else { Calibration::Returning { axis, target, builds: builds + 1 } }
            } else if builds < SETTLE_BUILDS {
                Calibration::Returning { axis, target, builds: builds + 1 }
            } else if axis == 0 {
                probe(1, camera, delivered)
            } else if state.scale.iter().all(|s| (s.abs() * PROBE_COUNTS as f32) < NO_TURN) {
                // Read but not turned: the game is not taking mouse look yet. Not an attempt.
                state.no_turns += 1;
                if state.no_turns == 1 {
                    log!("head aim: the game read the probe but did not turn (window focus, a menu?); trying again every second");
                }
                state.settle_builds = RETRY_BUILDS;
                Calibration::Settling { builds: 0 }
            } else if state.scale.iter().all(|s| PLAUSIBLE_SCALE.contains(&s.abs()))
                && (state.scale[0] / state.scale[1]).abs() <= PLAUSIBLE_RATIO
                && (state.scale[1] / state.scale[0]).abs() <= PLAUSIBLE_RATIO
            {
                // From now on the right stick turns the body, and left trigger presses wait for
                // the hand's aim.
                take_stick(state.native_stick);
                monaka_pad::gate_left_trigger(state.power_aim);
                log!(
                    "head aim: {:.5} degrees of yaw per x count, {:.5} of pitch per y count ({} probes read without turning before)",
                    state.scale[0],
                    state.scale[1],
                    state.no_turns
                );
                Calibration::Done
            } else if state.attempts < ATTEMPTS {
                state.attempts += 1;
                log!("head aim: implausible scale {:?} (the mouse moved?); calibrating again (attempt {})", state.scale, state.attempts);
                Calibration::Settling { builds: 0 }
            } else {
                fail(&format!("implausible scale {:?} {ATTEMPTS} times (was the mouse moved during calibration?)", state.scale))
            }
        }
        done => done,
    };
    if state.calibration == Calibration::Failed
        && state.body.is_some()
        && let Some(usual) = state.usual
    {
        state.scale = usual;
        state.calibration = Calibration::Done;
        take_stick(state.native_stick);
        monaka_pad::gate_left_trigger(state.power_aim);
        log!("head aim: steering on with the usual mouse scale {usual:?}");
    }
    let now = Instant::now();
    let step = state.last_build.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(MAX_STEP_SECONDS));
    state.last_build = Some(now);
    if state.calibration != Calibration::Done {
        return None;
    }
    let scale = state.scale;
    if pose.valid != 0 {
        let (yaw, pitch) = pointing_yaw_pitch(pose.orientation);
        state.head = [yaw.to_degrees(), pitch.to_degrees()];
    }
    let head = state.head;
    let hand = match state.pointer {
        Some(side) => pointing(side, state.rig),
        None if state.power_aim => power_hand(&mut state),
        None => None,
    };
    if hand.is_some() {
        state.hand_builds += 1;
    }
    let aimed = hand.unwrap_or(head);
    state.aimed = aimed;
    // The game carries no head share yet when the calibration ends: its yaw is the body's, and
    // the body's pitch is level (the mouse's pitch at attach is steered out).
    let mut body = state.body.unwrap_or([camera[0], 0.0]);
    let native_turning = state.native_stick && native_turn(&mut state, now, camera[0]);
    if let Some(stick) = monaka_pad::right_stick(STICK_MAX_AGE_MS).filter(|_| !state.native_stick) {
        // Stick right turns right (yaw, left positive, goes down); stick up looks up.
        let [yaw, pitch] = state.stick_turn;
        let (right, up) = (yaw.turn(stick[0], step, &mut state.latch), pitch.turn(stick[1], step, &mut TurnLatch::new()));
        body[0] = turned(body[0] - right, 0.0);
        body[1] = (body[1] + up).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }
    let in_flight = mouse::pending();
    let target = [turned(body[0] + aimed[0], 0.0), (body[1] + aimed[1]).clamp(-PITCH_LIMIT, PITCH_LIMIT)];
    if state.last_target.is_some_and(|last| turned(target[0], last[0]).abs() > JUMP || (target[1] - last[1]).abs() > JUMP) {
        state.grace = JUMP_GRACE;
    }
    state.last_target = Some(target);
    state.grace = state.grace.saturating_sub(1);
    let mut error = [0, 1].map(|k| turned(target[k], camera[k]) - (in_flight[k] as f32) * scale[k]);
    if native_turning {
        // The game turns with the stick: the body takes its turn (counts still in flight count as
        // turned), so the head's share stays what it was.
        body[0] = turned(camera[0] + in_flight[0] as f32 * scale[0] - aimed[0], 0.0);
        error[0] = 0.0;
    }
    if error[0].abs() > RESYNC && (in_flight[0] as f32 * scale[0]).abs() < 1.0 && state.grace == 0 {
        log!("head aim: the game turned the player {:.1} degrees by itself; the body follows", -error[0]);
        body[0] = turned(camera[0] - aimed[0], 0.0);
        error[0] = 0.0;
        state.resyncs.retain(|t| now.duration_since(*t) < STORM_WINDOW);
        state.resyncs.push(now);
        if state.resyncs.len() >= STORM_RESYNCS {
            state.body = Some(body);
            recalibrate(&mut state, "the game turned the player by itself again and again");
            return None;
        }
    }

    // A left trigger press waits for the aim (a shot leaves as the trigger passes the game's
    // threshold): steered harder meanwhile, let through once the game's aim is on the hand's.
    let gated = monaka_pad::left_trigger_held_back();
    if gated {
        let off = [0, 1].map(|k| turned(target[k], camera[k]).abs());
        if hand.is_none() || off.iter().all(|&d| d < GATE_CLOSE) {
            monaka_pad::open_left_trigger();
        }
    }
    let gain = if gated { GATE_GAIN } else { STEER_GAIN };
    mouse::add((gain * error[0] / scale[0]) as f64, (gain * error[1] / scale[1]) as f64);
    state.body = Some(body);
    Some([turned(camera[0], body[0]).to_radians(), (camera[1] - body[1]).to_radians()])
}

/// Where controller `side` points (yaw, pitch; degrees, tracking space): with the hand rig, at the
/// point 10 m along the palm's line, else along the controller's direction. `None` while it is
/// not tracked (the head aims meanwhile).
fn pointing(side: usize, rig: bool) -> Option<[f32; 2]> {
    let palm = crate::controllers::palm(side)?;
    let (yaw, pitch) = if rig {
        direction_yaw_pitch(crate::look::aim::weapon_point(&palm)).unwrap_or_else(|| pointing_yaw_pitch(palm.orientation))
    } else {
        pointing_yaw_pitch(palm.orientation)
    };
    Some([yaw.to_degrees(), pitch.to_degrees()])
}

/// Where the left hand aims its power (yaw, pitch; degrees, tracking space) while the left
/// trigger is held or was just let go: with the hand rig, at the point 10 m along the palm's line
/// (where the viewer's ring from the palm sits; the game's ray starts at the head), else along the
/// controller's direction (the ring from the head). `None`: the head aims.
fn power_hand(state: &mut State) -> Option<[f32; 2]> {
    let now = Instant::now();
    let trigger = monaka_pad::left_trigger(STICK_MAX_AGE_MS);
    let held = trigger.is_some_and(|t| t >= POWER_TRIGGER);
    if held {
        state.power_until = Some(now + POWER_HOLD);
    }
    if held != state.trigger_held {
        state.trigger_held = held;
        if held && state.presses < 20 {
            state.presses += 1;
            log!(
                "power aim: left trigger {:.2}; left palm {}, weapon {}",
                trigger.unwrap_or(0.0),
                if crate::controllers::palm(LEFT_HAND).is_some() { "tracked" } else { "NOT tracked" },
                match state.left_weapon {
                    Some(left_weapon) if left_weapon() => "in the left hand",
                    Some(_) => "none",
                    None => "not known",
                }
            );
        }
    }
    if state.power_until.is_none_or(|until| now >= until) {
        return None;
    }
    pointing(LEFT_HAND, state.rig)
}

/// With the game's own stick turning: whether the game may be turning with the stick now (pushed,
/// or let go within [`NATIVE_SETTLE`]). Logs each push's turn rate, for comparing the game's stick
/// look with and without mouse counts coming in.
fn native_turn(state: &mut State, now: Instant, camera_yaw: f32) -> bool {
    let pushed = monaka_pad::right_stick(STICK_MAX_AGE_MS).is_some_and(|[x, _]| state.stick_turn[0].pushed(x));
    if pushed {
        state.native_until = Some(now + NATIVE_SETTLE);
        state.stick_push.get_or_insert((now, camera_yaw));
    } else if let Some((since, from)) = state.stick_push.take() {
        let seconds = now.duration_since(since).as_secs_f32();
        let degrees = turned(from, camera_yaw);
        if state.pushes < 40 && seconds > 0.2 {
            state.pushes += 1;
            log!("native stick: the game turned {degrees:.1} degrees right in {seconds:.2} s ({:.0} degrees a second)", degrees / seconds);
        }
    }
    state.native_until.is_some_and(|until| now < until)
}

fn probe(axis: usize, camera: [f32; 2], delivered: [i64; 2]) -> Calibration {
    let mut counts = [0.0; 2];
    counts[axis] = PROBE_COUNTS;
    mouse::add(counts[0], counts[1]);
    Calibration::Probing { axis, angle: camera[axis], target: delivered[axis] + PROBE_COUNTS as i64, builds: 0, read_at: None }
}

/// The aim does not follow the counts at this scale: steering stops and the scale is measured
/// (the body is kept, so the view stays put).
fn recalibrate(state: &mut State, why: &str) {
    log!("head aim: {why}; measuring the mouse scale");
    give_stick();
    monaka_pad::gate_left_trigger(false);
    state.calibration = Calibration::Settling { builds: 0 };
    state.attempts = 1;
    state.no_turns = 0;
    state.settle_builds = 20;
    state.resyncs.clear();
    state.waiting_logged = false;
}

fn fail(why: &str) -> Calibration {
    log!("head aim off: calibration failed: {why}");
    give_stick();
    monaka_pad::gate_left_trigger(false);
    Calibration::Failed
}

/// Stops feeding the head and takes the head's share back out of the game's angles (the stick's
/// turns stay; a calibration cut short is undone whole). Returns whether the game read it within `ms` (it reads the
/// mouse only while running).
pub(super) fn release(ms: u64) -> bool {
    let back = {
        // Under the lock, so no build is between its check and its queueing.
        let mut state = STATE.locked();
        AIMING.store(false, Release);
        give_stick();
        monaka_pad::gate_left_trigger(false);
        match state.body.take() {
            // Steered: the game's aim is body plus head; the head's share comes out.
            Some(_) => [0, 1].map(|k| f64::from(state.aimed[k] / state.scale[k])),
            // A calibration cut short: everything it fed.
            None => {
                let (delivered, pending) = (mouse::delivered(), mouse::pending());
                [0, 1].map(|k| (delivered[k] - state.start[k]) as f64 + pending[k])
            }
        }
    };
    mouse::add(-back[0], -back[1]);
    let read = monaka_producer::wait_until(ms, || mouse::pending().iter().all(|p| p.abs() < 1.0));
    mouse::ACTIVE.store(false, Release);
    read
}

#[cfg(test)]
mod tests;
