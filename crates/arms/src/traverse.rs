//! Climbing, hanging from a ledge, a ladder: the game's own animation raises the hands to the
//! eyes, where the arms never go while walking or fighting (both wrists stay 0.17 m or more below
//! the camera in Dying Light 1). Hanging holds on with both hands or
//! with one, the other reaching back. The arms then go back to the game's animation, carried
//! under the real head, blended over [`BLEND_MS`]; swings do not attack and a producer leaves the
//! character's look to the game ([`traversing`]): turning on a ledge (to change hands, to jump
//! back) is the game's own. Until the hands come down again.

use monaka_core::camera::Mat34;
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};

/// Both animated wrists above this (metres, camera space, y up), or one above [`ENTER_ONE_Y`],
/// hand the arms to the game...
const ENTER_Y: f32 = -0.05;
const ENTER_ONE_Y: f32 = 0.05;
/// ...and they stay with it while either wrist still holds on ([`holds_on`]).
const STAY_Y: f32 = -0.12;

/// Whether a wrist (camera space) is where only a climb puts it: up at the eyes (a climb pulls
/// one hand down at a time), or behind them. Hanging turned away from the ledge, the hand still
/// on it sits 0.55-0.71 m out to its side and 0.13-0.45 m behind the eyes, the other reaching
/// ahead low; turning between the poses swings it up to 0.72 m behind (Dying Light 1). Walking
/// and fighting, no wrist goes further back than 0.03 m.
fn holds_on(wrist: &[f32; 3]) -> bool {
    let [x, y, z] = *wrist;
    y > STAY_Y || z > 0.3 || (x.abs() > 0.45 && z > 0.05)
}
/// How long each must hold before the arms switch (ms).
const ENTER_MS: u64 = 80;
const LEAVE_MS: u64 = 300;
/// How long the arms take to blend between the controllers and the animation (ms).
pub const BLEND_MS: f32 = 200.0;
/// After a swing attacks, the attack's own animation may raise both hands: no switch for this long (ms).
const AFTER_SWING_MS: u64 = 1000;
/// Until this tick (`monaka_channel::tick`) the producer knows the character is fighting, not
/// climbing ([`hold_off`]).
static HOLD_OFF_UNTIL: AtomicU64 = AtomicU64::new(0);

/// The character is fighting, not climbing, for the next `ms`: the arms stay on the controllers
/// whatever the game's animation does with the hands (Kingdom Come raises them to the eyes for a
/// guard over the head and for its attacks). A producer renews it while the fight goes
/// on; the arms already with the game come back once the hands come down.
pub fn hold_off(ms: u64) {
    HOLD_OFF_UNTIL.fetch_max(monaka_channel::tick() + ms, Relaxed);
}

/// Set by a game without climbing ([`declare_no_climbing`]).
static NO_CLIMBING: AtomicBool = AtomicBool::new(false);

/// The game has no climbing, hanging or ladders that raise the hands: they stay on the controllers
/// whatever its animation does with them (Kingdom Come raises them for guards, attacks, doors and
/// drinking). Declared once at start, as a fact of the game.
pub fn declare_no_climbing() {
    NO_CLIMBING.store(true, Relaxed);
}

/// Whether the game drives the arms and how far the rig has blended in.
#[derive(Clone, Copy, Debug)]
struct Traversal {
    game: bool,
    /// When the switch the wrists ask for was first asked for.
    pending: Option<u64>,
    /// The rig's share of the arms: 1 on the controllers, 0 as the game animates them.
    weight: f32,
    last: u64,
}

impl Traversal {
    const fn new() -> Self {
        Self { game: false, pending: None, weight: 1.0, last: 0 }
    }

    /// One update at `now` (ms) with both animated wrists (camera space); `after_swing` keeps the
    /// arms on the controllers. The rig's weight, and whether the arms switched.
    fn step(&mut self, now: u64, wrists: [[f32; 3]; 2], after_swing: bool) -> (f32, bool) {
        let wants = match self.game {
            true => wrists.iter().any(holds_on),
            false => !after_swing && (wrists.iter().all(|w| w[1] > ENTER_Y) || wrists.iter().any(|w| w[1] > ENTER_ONE_Y)),
        };
        let mut switched = false;
        if wants == self.game {
            self.pending = None;
        } else {
            let since = *self.pending.get_or_insert(now);
            if now.saturating_sub(since) >= if self.game { LEAVE_MS } else { ENTER_MS } {
                self.game = wants;
                self.pending = None;
                switched = true;
            }
        }
        // A long gap (a pause, a loading screen) finishes the blend at once.
        let elapsed = if self.last == 0 { 0 } else { now.saturating_sub(self.last) };
        self.last = now;
        let target = if self.game { 0.0 } else { 1.0 };
        let step = if elapsed > 1000 { 1.0 } else { elapsed as f32 / BLEND_MS };
        self.weight = if self.weight < target { (self.weight + step).min(target) } else { (self.weight - step).max(target) };
        (self.weight, switched)
    }
}

static STATE: Mutex<Traversal> = Mutex::new(Traversal::new());
/// How far the arms have blended to the climb (0 on the controllers, 1 as the game animates them), as `f32` bits.
static CLIMB_SHARE: AtomicU32 = AtomicU32::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
/// The highest both wrists were seen together (mm above the camera, offset by a metre to stay unsigned).
static HIGHEST: AtomicU64 = AtomicU64::new(0);

/// One arms update with both wrists as the game animated them (camera space): the rig's share
/// of the arms now (1 on the controllers, 0 as animated).
pub fn update(wrists: [[f32; 3]; 2], game: &Mat34) -> f32 {
    if NO_CLIMBING.load(Relaxed) {
        return 1.0;
    }
    let lower = wrists[0][1].min(wrists[1][1]);
    if lower.is_finite() {
        HIGHEST.fetch_max(((lower + 1.0).max(0.0) * 1000.0) as u64, Relaxed);
    }
    let Ok(mut state) = STATE.lock() else { return 1.0 };
    let now = monaka_channel::tick();
    let fighting = crate::melee::swung_within(AFTER_SWING_MS) || now < HOLD_OFF_UNTIL.load(Relaxed);
    let (weight, switched) = state.step(now, wrists, fighting);
    CLIMB_SHARE.store((1.0 - weight).to_bits(), Relaxed);
    if switched && state.game {
        *WALL.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    if state.game && wrists.iter().all(|w| w[1] > ENTER_Y) {
        note_wall(wrists, game);
    }
    let [l, r] = wrists;
    if switched && SWITCHES.fetch_add(1, Relaxed) < 100 {
        let what = if state.game { "hands up: the arms go to the game's animation" } else { "hands down: the arms go back on the controllers" };
        log!("hand rig: {what} (wrists left {l:.2?}, right {r:.2?} against the camera: x right, y up, z back)");
    }
    // The wrists through a climb and the few seconds after, to tell the game's poses apart.
    if switched && !state.game {
        LEFT_AT.store(now, Relaxed);
    }
    let recent = state.game || now.saturating_sub(LEFT_AT.load(Relaxed)) < 3000;
    if recent && now.saturating_sub(LAST_SAMPLE.load(Relaxed)) >= 250 && SAMPLES.fetch_add(1, Relaxed) < 1500 {
        LAST_SAMPLE.store(now, Relaxed);
        log!("  climb sample ({}): wrists left {l:.2?}, right {r:.2?}", if state.game { "game" } else { "rig" });
    }
    weight
}

/// Which way the wall is from the camera during the latest climb (world x and z, unit): toward
/// where both hands hold on, smoothed. Where the player looks on grabbing is no guide: the player
/// often grabs looking along the wall, the hands taking hold beside the head.
static WALL: Mutex<Option<[f32; 2]>> = Mutex::new(None);
/// How much of each update's reading goes into [`WALL`].
const WALL_SMOOTHING: f32 = 0.2;
static WALLS_LOGGED: AtomicU64 = AtomicU64::new(0);

/// Turns the wrists' middle (camera space) into the world's horizontal direction from the camera
/// `game` and folds it into [`WALL`].
fn note_wall(wrists: [[f32; 3]; 2], game: &Mat34) {
    let middle = [0, 1, 2].map(|i| (wrists[0][i] + wrists[1][i]) / 2.0);
    let world = [0, 2].map(|row| (0..3).map(|c| game[row * 4 + c] * middle[c]).sum::<f32>());
    let length = (world[0] * world[0] + world[1] * world[1]).sqrt();
    if !(length.is_finite() && length > 0.05) {
        return;
    }
    let reading = [world[0] / length, world[1] / length];
    let Ok(mut wall) = WALL.lock() else { return };
    let mixed = match *wall {
        Some(old) => [0, 1].map(|i| old[i] * (1.0 - WALL_SMOOTHING) + reading[i] * WALL_SMOOTHING),
        None => {
            // The camera looks along -back: (-game[2], -game[10]) horizontally.
            let turn = (reading[0] * -game[10] - reading[1] * -game[2]).atan2(reading[0] * -game[2] + reading[1] * -game[10]);
            if WALLS_LOGGED.fetch_add(1, Relaxed) < 100 {
                log!("hand rig: the wall is {:.0} degrees {} of where the camera looks (from where the hands hold on)", turn.to_degrees().abs(), if turn > 0.0 { "left" } else { "right" });
            }
            reading
        }
    };
    let length = (mixed[0] * mixed[0] + mixed[1] * mixed[1]).sqrt();
    if length > 1e-3 {
        *wall = Some([mixed[0] / length, mixed[1] / length]);
    }
}

/// Which way the wall of the latest climb is from the camera (world x and z, unit), once both
/// hands have held on: a producer backs its view off it.
pub fn wall_direction() -> Option<[f32; 2]> {
    *WALL.lock().ok()?
}

static LEFT_AT: AtomicU64 = AtomicU64::new(0);
static LAST_SAMPLE: AtomicU64 = AtomicU64::new(0);
static SAMPLES: AtomicU64 = AtomicU64::new(0);

/// How far into a climb the arms have blended (0 off it, 1 climbing, in between for [`BLEND_MS`]
/// either way): what eases a producer's climbing view in and out.
pub fn climb_share() -> f32 {
    f32::from_bits(CLIMB_SHARE.load(Relaxed))
}

/// Whether the game drives the arms now (climbing, hanging): aim with the head, swings do not attack.
pub fn traversing() -> bool {
    STATE.lock().is_ok_and(|state| state.game)
}

pub fn report() {
    let highest = HIGHEST.load(Relaxed);
    if highest > 0 {
        log!("  both hands up {}x; the lower wrist was at most {:.2} m against the camera", SWITCHES.load(Relaxed), highest as f32 / 1000.0 - 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOWN: [[f32; 3]; 2] = [[-0.3, -0.7, -0.1], [0.3, -0.37, -0.36]];
    const UP: [[f32; 3]; 2] = [[-0.3, 0.1, -0.4], [0.3, 0.12, -0.4]];

    #[test]
    fn both_hands_up_hand_the_arms_to_the_game_and_back() {
        let mut t = Traversal::new();
        assert_eq!(t.step(1000, DOWN, false), (1.0, false));
        assert_eq!(t.step(1016, UP, false), (1.0, false), "not before the hold");
        let (weight, switched) = t.step(1100, UP, false);
        assert!(switched && t.game && weight < 1.0);
        let (weight, _) = t.step(1300, UP, false);
        assert_eq!(weight, 0.0, "blended out");
        // One hand down mid-climb keeps it.
        let one_down = [UP[0], DOWN[1]];
        assert!(!t.step(1400, one_down, false).1);
        assert!(!t.step(2000, one_down, false).1 && t.game);
        assert!(!t.step(2050, [[-0.3, -0.1, -0.4], DOWN[1]], false).1 && t.game, "just above the stay line");
        // Both down: back after the hold, blending in.
        assert!(!t.step(2100, DOWN, false).1);
        assert!(!t.step(2390, DOWN, false).1);
        let (weight, switched) = t.step(2400, DOWN, false);
        assert!(switched && !t.game && weight > 0.0 && weight < 1.0);
        assert_eq!(t.step(2700, DOWN, false).0, 1.0);
    }

    #[test]
    fn one_hand_holding_on_the_other_reaching_back() {
        let mut t = Traversal::new();
        let left_holds = [UP[0], [0.35, -0.5, 0.2]];
        t.step(1000, left_holds, false);
        assert!(t.step(1100, left_holds, false).1 && t.game);
    }

    #[test]
    fn turned_away_from_the_ledge_the_hand_behind_still_holds_on() {
        let mut t = Traversal::new();
        t.step(1000, UP, false);
        assert!(t.step(1100, UP, false).1 && t.game);
        // Measured: left reaching ahead, right still on the ledge behind; then mirrored; then mid-turn.
        let poses = [[[-0.13, -0.39, -0.49], [0.69, -0.23, 0.21]], [[-0.67, -0.24, 0.27], [0.10, -0.39, -0.49]], [[0.48, -0.12, -0.27], [0.38, -0.19, 0.65]]];
        for (i, pose) in poses.iter().enumerate() {
            for now in (0..1000).step_by(16) {
                assert!(!t.step(2000 + i as u64 * 1000 + now, *pose, false).1 && t.game, "{pose:?}");
            }
        }
        // Back to walking with a weapon.
        t.step(6000, DOWN, false);
        assert!(t.step(6300, DOWN, false).1 && !t.game);
    }

    #[test]
    fn a_hand_raised_short_of_the_eyes_or_an_attack_keeps_the_rig() {
        let mut t = Traversal::new();
        let one_up = [[-0.3, 0.0, -0.4], DOWN[1]];
        for now in (1000..2000).step_by(16) {
            assert_eq!(t.step(now, one_up, false), (1.0, false));
        }
        for now in (2000..3000).step_by(16) {
            assert_eq!(t.step(now, UP, true), (1.0, false), "an attack's own animation");
        }
    }

    #[test]
    fn a_long_gap_finishes_the_blend() {
        let mut t = Traversal::new();
        t.step(1000, UP, false);
        t.step(1100, UP, false);
        assert_eq!(t.step(5000, UP, false).0, 0.0);
    }
}
