//! Room-scale following (`room_scale`): the character follows the player's real steps and leans.
//!
//! The view and the arms are the game camera plus the tracked head and controllers, and nothing
//! else moves the character, so a step back puts the eyes and arms behind the body enemies reach
//! for and grab. Here, each step of the player's character, the head's horizontal offset from the
//! character becomes extra velocity ([`follow`]), which the game adds to its own move, so the
//! character walks to where the player is, with the game's collision (walls, props and enemies
//! stop it). Each game hooks its own character step and says what its step moved ([`credit`]).
//!
//! What the character walks for the head is taken out of the head's position again: the anchor,
//! in tracking space, is how far the head has moved that the character has followed, and the
//! tracking origin is shifted back by it ([`shift_origin`], for the eyes and the arms alike), so
//! the view does not move twice. Blocked by a wall, the character stays and the anchor with it:
//! the head can then lean past the character, as without following.
//!
//! Off while the game moves the body itself (the game says why) or the player climbs, and when no
//! head is tracked. A recentre (the head's position jumping) moves the anchor with it, so the
//! character does not walk off to the new centre.

use crate::traverse;
use monaka_core::camera::Mat34;
use monaka_core::options::{Options, Shared};
use monaka_producer::log;
use std::sync::Mutex;
use std::time::Instant;

/// A head position this far from the previous view's is a recentre, not a step (metres).
const RECENTRE_JUMP: f32 = 0.3;
/// How often the summary is logged (seconds).
const SUMMARY_EVERY: f32 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// The fastest the character walks to follow (m/s).
    pub max_speed: f32,
    /// How quickly it closes the gap: the speed is the gap over this (seconds).
    pub response: f32,
    /// A gap this small is left alone (metres), so standing still does not shuffle the feet.
    pub deadzone: f32,
    /// A step the head takes 3 s in (over a second), for following alone (a test without a
    /// headset).
    pub test_step: Option<[f32; 2]>,
}

impl Settings {
    /// `room_scale` (default on, with the hand rig and a tracked head), up to `room_scale_speed`
    /// m/s, closing the gap over `room_scale_response` s, past `room_scale_deadzone` m.
    /// `room_scale_test=x,z` (tracking metres) glides the head that far over a second, 3 s into
    /// the run.
    pub fn from_options(options: &Options, shared: &Shared) -> Option<Self> {
        let on = options.switch("room_scale", true);
        let settings = Settings {
            max_speed: options.number("room_scale_speed", 0.5, 6.0, 3.0),
            response: options.number("room_scale_response", 0.03, 1.0, 0.12),
            deadzone: options.number("room_scale_deadzone", 0.0, 0.3, 0.03),
            test_step: options.floats::<2>("room_scale_test", 3.0),
        };
        (shared.aim.rig && shared.head_tracking && on).then_some(settings)
    }
}

#[derive(Default)]
struct Stats {
    steps: u64,
    following: u64,
    gated: u64,
    followed: f32,
    widest: f32,
    recentres: u64,
}

struct State {
    settings: Option<Settings>,
    /// Tracking-space x and z of the head's movement the character has followed.
    anchor: [f32; 2],
    /// The latest head position (tracking space) and the tracking axes x and z in the world.
    head: Option<[f32; 3]>,
    axes: Option<([f32; 3], [f32; 3])>,
    stats: Stats,
    last_summary: Option<Instant>,
    started: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    settings: None,
    anchor: [0.0; 2],
    head: None,
    axes: None,
    stats: Stats { steps: 0, following: 0, gated: 0, followed: 0.0, widest: 0.0, recentres: 0 },
    last_summary: None,
    started: None,
});

pub fn install(settings: Settings) {
    if let Ok(mut state) = STATE.lock() {
        state.settings = Some(settings);
        state.started = Some(Instant::now());
    }
    log!("room-scale following: up to {} m/s, closing the gap over {} s, past {} m", settings.max_speed, settings.response, settings.deadzone);
}

/// Whether following is on (installed and not stopped).
pub fn active() -> bool {
    STATE.lock().is_ok_and(|state| state.settings.is_some())
}

/// How far the head has moved that the character has followed (tracking-space x and z; zero
/// without following).
pub fn anchor() -> [f32; 2] {
    STATE.lock().ok().filter(|state| state.settings.is_some()).map_or([0.0; 2], |state| state.anchor)
}

/// Moves a tracking origin back by what following has walked (nothing without it): the anchor
/// along the origin's own x and z axes. Every tracking origin the eyes and arms are built on goes
/// through here, so they move together.
pub fn shift_origin(origin: &mut Mat34) {
    let [x, z] = anchor();
    origin[3] -= origin[0] * x + origin[2] * z;
    origin[7] -= origin[4] * x + origin[6] * z;
    origin[11] -= origin[8] * x + origin[10] * z;
}

/// A view was placed from the tracking origin `origin` (before [`shift_origin`]; its axes are
/// what matter) with the head at `head` (tracking space).
pub fn note_view(origin: &Mat34, head: [f32; 3]) {
    let Ok(mut state) = STATE.lock() else { return };
    let Some(settings) = state.settings else { return };
    if !head.iter().all(|c| c.is_finite()) {
        return;
    }
    let mut head = head;
    // Taken over a second, like a step (all at once it would be a recentre).
    if let (Some([x, z]), Some(started)) = (settings.test_step, state.started) {
        let share = (started.elapsed().as_secs_f32() - 3.0).clamp(0.0, 1.0);
        head[0] += x * share;
        head[2] += z * share;
    }
    match state.head {
        // The first head: where the player stands is where the character is.
        None => state.anchor = [head[0], head[2]],
        Some(last) => {
            let jump = [head[0] - last[0], head[2] - last[2]];
            if jump[0].hypot(jump[1]) > RECENTRE_JUMP {
                state.anchor[0] += jump[0];
                state.anchor[1] += jump[1];
                state.stats.recentres += 1;
            }
        }
    }
    state.head = Some(head);
    state.axes = Some(([origin[0], origin[4], origin[8]], [origin[2], origin[6], origin[10]]));
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The velocity to follow with this step of the player's character (world x and z, m/s): zero
/// while the game moves the body itself (`waiting`, its reason), while climbing, or with no head.
pub fn follow(waiting: Option<&'static str>) -> [f32; 2] {
    let Ok(mut state) = STATE.lock() else { return [0.0; 2] };
    let Some(settings) = state.settings else { return [0.0; 2] };
    state.stats.steps += 1;
    let waiting = waiting.is_some() || traverse::traversing();
    let follow = match (waiting, state.head, state.axes) {
        (false, Some(head), Some((x_axis, z_axis))) => {
            let gap = [head[0] - state.anchor[0], head[2] - state.anchor[1]];
            let world = [x_axis[0] * gap[0] + z_axis[0] * gap[1], x_axis[2] * gap[0] + z_axis[2] * gap[1]];
            let distance = world[0].hypot(world[1]);
            state.stats.widest = state.stats.widest.max(distance);
            let speed = ((distance - settings.deadzone) / settings.response).clamp(0.0, settings.max_speed);
            // Under a centimetre a second is standing still.
            if speed > 0.01 {
                state.stats.following += 1;
                [world[0] / distance * speed, world[1] / distance * speed]
            } else {
                [0.0; 2]
            }
        }
        _ => {
            state.stats.gated += 1;
            [0.0; 2]
        }
    };
    let summary_due = state.last_summary.is_none_or(|at| at.elapsed().as_secs_f32() >= SUMMARY_EVERY);
    if summary_due {
        if state.last_summary.is_some() {
            summarise(&state);
        }
        state.last_summary = Some(Instant::now());
    }
    follow
}

/// A step that asked to follow at `follow` (m/s) for `dt` moved the character `beyond` (world)
/// further than the game's own move: the part along the following velocity, no further than it
/// asked (a wall stops it short), goes into the anchor.
pub fn credit(beyond: [f32; 3], follow: [f32; 2], dt: f32) {
    let speed = follow[0].hypot(follow[1]);
    if speed <= 1e-4 || dt <= 0.0 || !beyond.iter().all(|c| c.is_finite()) {
        return;
    }
    let Ok(mut state) = STATE.lock() else { return };
    let Some((x_axis, z_axis)) = state.axes else { return };
    let beyond = [beyond[0], 0.0, beyond[2]];
    let along = [follow[0] / speed, 0.0, follow[1] / speed];
    let followed = dot(beyond, along).clamp(0.0, speed * dt);
    let world = [along[0] * followed, 0.0, along[2] * followed];
    // Back into tracking space (the axes are orthonormal).
    state.anchor[0] += dot(world, x_axis);
    state.anchor[1] += dot(world, z_axis);
    state.stats.followed += followed;
}

fn summarise(state: &State) {
    let s = &state.stats;
    log!(
        "room-scale following: {} steps, {} following, {} waiting (the game moving the body); followed {:.2} m in all, widest gap {:.2} m, {} recentres; anchor {:.2?}",
        s.steps,
        s.following,
        s.gated,
        s.followed,
        s.widest,
        s.recentres,
        state.anchor
    );
}

/// Following stops (a game's step hook may still be in; it then asks for nothing more).
pub fn stop() {
    if let Ok(mut state) = STATE.lock()
        && state.settings.take().is_some()
    {
        summarise(&state);
    }
}

