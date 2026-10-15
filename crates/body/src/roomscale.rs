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
//! Off while the game moves the body itself or the view is not the player's (the game says why:
//! [`Wait`]) or the player climbs, and when no head is tracked. A recentre (the head's position
//! jumping) moves the anchor with it, so the character does not walk off to the new centre.
//!
//! A step hook that cannot measure its step inside it has it measured at the next step
//! ([`begin_step`]): the body's move since, beyond the game's own velocity over the step.

use crate::feature::Steps;
use crate::traverse;
use monaka_core::camera::{Base, Mat34};
use monaka_core::math::{Vec3, dot};
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
    *origin = moved_back(origin, origin, anchor());
}

/// Room-scale following on a view drawn from the game camera `game` taken as `base`, the head
/// drawn at `head` (tracking space, as the head model places it): the view noted ([`note_view`])
/// and `game` moved back by what following has walked, along the tracking origin's own x and z
/// axes ([`shift_origin`]), so the eyes built on it with `base` keep the head where it is against
/// the character. Every eye site on following calls this.
pub fn follow_view(game: &Mat34, base: Base, head: Vec3) -> Option<Mat34> {
    let origin = base.origin(game)?;
    note_view(&origin, head);
    Some(moved_back(game, &origin, anchor()))
}

/// `frame` moved back by `anchor` (tracking x and z) along the tracking origin `origin`'s x and z
/// axes.
fn moved_back(frame: &Mat34, origin: &Mat34, [x, z]: [f32; 2]) -> Mat34 {
    let mut out = *frame;
    for row in 0..3 {
        out[row * 4 + 3] -= origin[row * 4] * x + origin[row * 4 + 2] * z;
    }
    out
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
    let axes = ([origin[0], origin[4], origin[8]], [origin[2], origin[6], origin[10]]);
    // A turn (the stick, the game turning the character) turns the world about the character,
    // not the head: the head's offset from it is turned back with it, so the head stays where it
    // is in the world and the turn pivots on it (the character then follows as for a step).
    if let Some(old) = state.axes {
        state.anchor = turned_anchor(state.anchor, head, old, axes);
    }
    state.head = Some(head);
    state.axes = Some(axes);
}

/// The anchor that keeps the head's offset from the character (`head - anchor`, tracking x and z)
/// pointing the same way in the world when the tracking axes x and z turn from `old` to `new`.
fn turned_anchor(anchor: [f32; 2], head: [f32; 3], old: ([f32; 3], [f32; 3]), new: ([f32; 3], [f32; 3])) -> [f32; 2] {
    let offset = [head[0] - anchor[0], head[2] - anchor[1]];
    let world = [0, 1, 2].map(|k| old.0[k] * offset[0] + old.1[k] * offset[1]);
    let along = |axis: [f32; 3]| {
        let length2 = dot(axis, axis);
        if length2 > 1e-9 { dot(world, axis) / length2 } else { 0.0 }
    };
    let turned = [along(new.0), along(new.1)];
    if turned.iter().all(|c| c.is_finite()) { [head[0] - turned[0], head[2] - turned[1]] } else { anchor }
}

/// Why following waits on a step, as the game says it ([`Steps::waiting`], or a step hook asking
/// [`follow`] itself); climbing is following's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    /// The mod is not drawing the view.
    NotRunning,
    /// The step takes the body to a target of the game's own (a scripted move).
    ScriptedMove,
    /// The game holds the body's position.
    PositionLocked,
    /// The body's physics is off.
    PhysicsOff,
    /// The body is attached to something (a vehicle, a rope) and goes with it.
    Attached,
    /// An animation moves the body.
    Animated,
    /// The step takes no time.
    NoTime,
    /// A cutscene has the player.
    Cutscene,
}

/// The velocity to follow with this step of the player's character (world x and z, m/s): zero
/// while the game says following waits (`waiting`), while climbing, or with no head.
pub fn follow(waiting: Option<Wait>) -> [f32; 2] {
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

/// A step measured at the next one: the body's position as the previous step began, and what
/// that step asked (the game's own velocity, the following velocity and the step's length).
struct LastStep {
    body: Option<[f32; 3]>,
    ask: Option<([f32; 3], [f32; 2], f32)>,
}

impl LastStep {
    const fn new() -> Self {
        Self { body: None, ask: None }
    }

    /// A step `dt` long begins with the body at `now`: what the previous step moved beyond the
    /// game's own velocity goes to `credit`, then this step's velocity comes from `follow`, asked
    /// with why following waits (`waiting`).
    fn begin(
        &mut self,
        now: Option<[f32; 3]>,
        dt: f32,
        waiting: impl FnOnce() -> Option<Wait>,
        follow: impl FnOnce(Option<Wait>) -> [f32; 2],
        credit: impl FnOnce([f32; 3], [f32; 2], f32),
    ) -> [f32; 2] {
        let now = now.filter(|p| p.iter().all(|c| c.is_finite()));
        if let (Some(now), Some(before), Some((game, follow, last_dt))) = (now, self.body, self.ask) {
            credit([now[0] - before[0] - game[0] * last_dt, 0.0, now[2] - before[2] - game[2] * last_dt], follow, last_dt);
        }
        self.body = now;
        let follow = follow(waiting());
        // The game's own velocity is filled in by `game`, once the step has it.
        self.ask = Some(([0.0; 3], follow, dt));
        follow
    }

    /// The game's own velocity on the step just begun.
    fn game(&mut self, game: [f32; 3]) {
        if let Some(ask) = self.ask.as_mut() {
            ask.0 = if game.iter().all(|c| c.is_finite()) { game } else { [0.0; 3] };
        }
    }
}

static LAST_STEP: Mutex<LastStep> = Mutex::new(LastStep::new());

/// A step of the player's body `body` (the step hook's handle on it), `dt` seconds long, begins:
/// what the previous step moved beyond the game's own velocity is credited (the body's move since,
/// where [`Steps::position`] reads it), and the velocity to follow with this step is returned
/// (world x and z, m/s), waiting while [`Steps::waiting`] gives a reason. The hook then says the
/// game's own velocity ([`game_velocity`]).
pub fn begin_step<G: Steps>(body: usize, dt: f32) -> [f32; 2] {
    if !active() {
        return [0.0; 2];
    }
    let Ok(mut last) = LAST_STEP.lock() else { return [0.0; 2] };
    last.begin(G::position(body), dt, || G::waiting(body), follow, credit)
}

/// The game's own velocity on the step [`begin_step`] began (what its velocity was added to;
/// world, m/s).
pub fn game_velocity(game: [f32; 3]) {
    if let Ok(mut last) = LAST_STEP.lock() {
        last.game(game);
    }
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

#[cfg(test)]
mod tests;
