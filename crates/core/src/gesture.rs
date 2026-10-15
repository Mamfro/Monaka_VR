//! Hand gestures: small state machines over the controllers in tracking space, each turning
//! something the player does with their hands into a value or a moment a producer hands to its
//! game (a reload, a lock's pick angle, a bow's draw, a stab, a crouch). None of them touches a
//! game: what a gesture does there is the producer's call (a game action, a written field).
//!
//! Each detector is a rules value (thresholds, with presets measured in play) and a state value fed
//! once per update with the time (seconds, any steady clock) and what it watches, as
//! [`crate::swing`] is. The motion-based ones read a [`Trail`].
//!
//! - [`TouchAway`]: one hand brought to another and taken away again (reloading a gun).
//! - [`Twist`]: a wrist's roll about where the hand points, since a start (turning a lock's pick).
//! - [`Pull`]: how far one hand is pulled back from another along a line (drawing a bow).
//! - [`Directed`]: the followed point moving fast along a direction (a thrust along the blade, a
//!   draw back toward the body).
//! - [`Release`]: the velocity a thrown thing leaves the hand with.
//! - [`Hold`]: a value that counts once held a moment (a guard zone, any jittery choice).
//! - [`Duck`]: the head lowered from standing, with a band so it does not flicker (crouching).
//! - [`reach_share`]: how far a hand has crossed from its own place to another's.

use crate::math::{Quat, Vec3, rotate, wrap_radians};
use crate::math::{dot, length};
use crate::trail::{Motion, Trail};

/// One hand brought within `near` of another (or of anything it reaches for) and taken past `away`
/// within `within`: a reload's "hand to the gun and away". Not again for `cooldown`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchAwayRules {
    /// Metres.
    pub near: f32,
    /// Metres.
    pub away: f32,
    /// Seconds from the last touch.
    pub within: f64,
    /// Seconds from the last gesture.
    pub cooldown: f64,
}

impl TouchAwayRules {
    /// The left controller to the gun hand's and away: a reload.
    pub const RELOAD: Self = Self { near: 0.15, away: 0.25, within: 1.0, cooldown: 1.5 };
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchAway {
    touched: Option<f64>,
    last: Option<f64>,
}

impl TouchAway {
    pub const fn new() -> Self {
        Self { touched: None, last: None }
    }

    /// The hands `distance` apart at `time`: whether the gesture just finished.
    pub fn feed(&mut self, time: f64, distance: f32, rules: &TouchAwayRules) -> bool {
        if !distance.is_finite() {
            return false;
        }
        if distance < rules.near {
            self.touched = Some(time);
            return false;
        }
        let fresh = self.touched.is_some_and(|at| time - at < rules.within);
        let rested = self.last.is_none_or(|at| time - at > rules.cooldown);
        if distance > rules.away && fresh && rested {
            self.touched = None;
            self.last = Some(time);
            return true;
        }
        false
    }

    /// Forgets a touch in progress (the gesture no longer applies: the gun was put away).
    pub fn reset(&mut self) {
        self.touched = None;
    }
}

/// The hand's roll about where it points (radians, clockwise as the player sees it from behind the
/// hand), against the world's up: 0 with the back of the hand up. `None` with the hand pointing
/// nearly straight up or down, where the roll has no reference. `up` is the world's up in the
/// space `orientation` is in; the hand points along its `-z` with its `y` up.
pub fn wrist_roll(orientation: Quat, up: Vec3) -> Option<f32> {
    let forward = rotate(orientation, [0.0, 0.0, -1.0]);
    let hand_up = rotate(orientation, [0.0, 1.0, 0.0]);
    // The world's up, square to where the hand points.
    let along = dot(up, forward);
    let level = [0, 1, 2].map(|k| up[k] - forward[k] * along);
    let level_length = length(level);
    if level_length < 0.2 {
        return None;
    }
    let level = level.map(|v| v / level_length);
    // Right of where the hand points: forward x level up.
    let right = [forward[1] * level[2] - forward[2] * level[1], forward[2] * level[0] - forward[0] * level[2], forward[0] * level[1] - forward[1] * level[0]];
    Some(dot(hand_up, right).atan2(dot(hand_up, level)))
}

/// A wrist's twist since a start ([`Twist::begin`]): the turn of a lock's pick or screwdriver.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Twist {
    start: Option<f32>,
}

impl Twist {
    pub const fn new() -> Self {
        Self { start: None }
    }

    /// Starts measuring from the hand's roll now; `false` (and no start) where the roll has no
    /// reference ([`wrist_roll`]).
    pub fn begin(&mut self, orientation: Quat, up: Vec3) -> bool {
        self.start = wrist_roll(orientation, up);
        self.start.is_some()
    }

    pub fn started(&self) -> bool {
        self.start.is_some()
    }

    /// How far the wrist has twisted since the start (radians, -pi..pi).
    pub fn since(&self, orientation: Quat, up: Vec3) -> Option<f32> {
        Some(wrap_radians(wrist_roll(orientation, up)? - self.start?))
    }

    pub fn reset(&mut self) {
        self.start = None;
    }
}

/// How a pull back along a line turns into a share (a bow's draw).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PullRules {
    /// Less than this (metres) is no pull.
    pub least: f32,
    /// This far (metres) is a full share.
    pub full: f32,
    /// A pull reads at most this far (metres): the arm's reach, past which a hand is lost.
    pub longest: f32,
}

impl PullRules {
    /// A bow drawn by the string hand.
    pub const BOW: Self = Self { least: 0.05, full: 0.5, longest: 0.7 };
}

/// One hand's pull back from another along a line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pull;

impl Pull {
    /// How far `hand` is behind `anchor` along `toward` (unit: the way the pull goes against,
    /// where an arrow points), clamped to `0..=rules.longest` (metres).
    pub fn back(anchor: Vec3, toward: Vec3, hand: Vec3, rules: &PullRules) -> f32 {
        let back = dot([0, 1, 2].map(|k| anchor[k] - hand[k]), toward);
        if back.is_finite() { back.clamp(0.0, rules.longest) } else { 0.0 }
    }

    /// The share (0..=1) a pull of `back` metres gives, `None` under `rules.least`.
    pub fn share(back: f32, rules: &PullRules) -> Option<f32> {
        (back >= rules.least).then(|| (back / rules.full).clamp(0.0, 1.0))
    }
}

/// The followed point moving fast along a direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectedRules {
    /// The least speed over the window (m/s).
    pub speed: f32,
    /// The least share of the motion along the direction (of 1: cos 40 degrees is 0.766).
    pub share: f32,
    /// The window the motion is read over (seconds).
    pub window: f64,
    /// The least time from one to the next (seconds).
    pub gap: f64,
}

impl DirectedRules {
    /// A thrust: a sword's tip driving along its own blade within about 40 degrees.
    pub const THRUST: Self = Self { speed: 2.0, share: 0.766_044_44, window: 0.05, gap: 0.4 };
    /// A draw: the tip pulled back toward the body, at least 0.4 of its way.
    pub const DRAW_BACK: Self = Self { speed: 1.0, share: 0.4, window: 0.1, gap: 0.0 };
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Directed {
    last: Option<f64>,
}

impl Directed {
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Whether `trail`'s point moved along `direction` (unit) as `rules` ask, as of its newest
    /// sample (`time`): the motion, once per `rules.gap`.
    pub fn feed(&mut self, time: f64, trail: &Trail, direction: Vec3, rules: &DirectedRules) -> Option<Motion> {
        if self.last.is_some_and(|at| time - at < rules.gap) {
            return None;
        }
        let motion = trail.motion(rules.window)?;
        let matched = motion.speed >= rules.speed && motion.share_along(direction) >= rules.share;
        matched.then(|| {
            self.last = Some(time);
            motion
        })
    }

    /// Something else began the next action (a swing attacked): the gap runs from now.
    pub fn note(&mut self, time: f64) {
        self.last = Some(time);
    }
}

/// Where a throw is read from the hand's trail: its fastest velocity just before the release, and
/// the way it went through that moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReleaseRules {
    /// The window before the release a throw's peak is looked for in (seconds).
    pub within: f64,
    /// The span a velocity is read over (seconds).
    pub span: f64,
    /// The stroke's way is read from this long before the peak to this long after it (seconds).
    pub before: f64,
    pub after: f64,
}

impl ReleaseRules {
    /// A thrown object or weapon.
    pub const THROW: Self = Self { within: 0.5, span: Trail::DEFAULT_SPAN, before: 0.1, after: 0.04 };
}

/// Straight down, where a hand at rest lets go.
const DOWN: Vec3 = [0.0, -1.0, 0.0];

/// A throw's release: the hand's velocity (m/s), as fast as its peak, along its way (unit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Release {
    pub velocity: Vec3,
    pub way: Vec3,
}

impl Release {
    /// The throw `trail` shows at its newest sample ([`Release::from_peak`]), `None` without a
    /// peak (too few samples).
    pub fn read(trail: &Trail, rules: &ReleaseRules) -> Option<Release> {
        let peak = trail.fastest(rules.within, rules.span)?;
        Some(Self::from_peak(peak.velocity, trail.stroke(rules.within, rules.span, rules.before, rules.after)))
    }

    /// The release of a hand whose fastest velocity lately was `fastest`: as fast as that, the way
    /// it travelled through that moment (its `stroke`: a short flick stops before the button is let
    /// go, and what comes after is not the throw), else the way of its fastest, else down.
    pub fn from_peak(fastest: Vec3, stroke: Option<Vec3>) -> Release {
        let speed = length(fastest);
        let way = stroke.unwrap_or(if speed > 0.0 { fastest.map(|c| c / speed) } else { DOWN });
        Release { velocity: way.map(|c| c * speed), way }
    }

    /// What the thrown thing leaves with ([`ThrowSpeed`]); `None` for a hand speed not finite.
    pub fn thrown(&self, speed: &ThrowSpeed) -> Option<Vec3> {
        speed.thrown(self.velocity)
    }
}

/// How fast a thrown thing leaves for the hand's speed: `gain` times it, at most `most` (m/s); a
/// hand slower than `still` (m/s) is at rest and lets it drop at `drop` (m/s) straight down. An
/// arm's throw carries its forearm's whip, which the controller in the palm does not show, and a
/// throw in a room is slower than the game's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThrowSpeed {
    pub gain: f32,
    pub most: f32,
    pub still: f32,
    pub drop: f32,
}

impl ThrowSpeed {
    /// `gain` times the hand's speed, with no limit; a hand at rest gives a throw at rest.
    pub const fn gain(gain: f32) -> Self {
        Self { gain, most: f32::INFINITY, still: 0.0, drop: 0.0 }
    }

    /// What a hand moving at `hand` (m/s) throws at: along it, its speed times the gain, at most
    /// the most; down for a hand at rest. `None` for a hand speed not finite.
    pub fn thrown(&self, hand: Vec3) -> Option<Vec3> {
        let speed = length(hand);
        if !speed.is_finite() {
            return None;
        }
        if speed < self.still {
            return Some(DOWN.map(|c| c * self.drop));
        }
        let scale = if speed * self.gain > self.most { self.most / speed } else { self.gain };
        Some(hand.map(|c| c * scale))
    }
}

/// A value that counts once it has been held `hold` seconds: a choice read from the hands (a guard
/// zone from where a sword points) without its jitter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hold<T: Copy + PartialEq> {
    held: Option<T>,
    candidate: Option<(T, f64)>,
}

impl<T: Copy + PartialEq> Hold<T> {
    pub const fn new() -> Self {
        Self { held: None, candidate: None }
    }

    /// The value read now (`None`: no reading, the held value stays). Returns the value when it
    /// has just come to count.
    pub fn feed(&mut self, time: f64, value: Option<T>, hold: f64) -> Option<T> {
        let Some(value) = value else {
            self.candidate = None;
            return None;
        };
        if self.held == Some(value) {
            self.candidate = None;
            return None;
        }
        match self.candidate {
            Some((candidate, since)) if candidate == value => {
                if time - since >= hold {
                    self.held = Some(value);
                    self.candidate = None;
                    return Some(value);
                }
            }
            _ => self.candidate = Some((value, time)),
        }
        None
    }

    /// The value that counts.
    pub fn held(&self) -> Option<T> {
        self.held
    }

    /// Sets the value that counts (the game took another).
    pub fn set(&mut self, value: Option<T>) {
        self.held = value;
        self.candidate = None;
    }
}

impl<T: Copy + PartialEq> Default for Hold<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Crouching by lowering the head.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuckRules {
    /// The head this far (metres) below standing crouches...
    pub crouch: f32,
    /// ...and back within this far stands again.
    pub stand: f32,
    /// Standing height follows a head lower than it this fast (m/s) while standing: a player who
    /// slouches, or sits down, is not crouched for good.
    pub sink: f32,
}

impl DuckRules {
    pub const CROUCH: Self = Self { crouch: 0.35, stand: 0.22, sink: 0.02 };
}

/// The head's height against the highest it has stood at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Duck {
    standing: Option<f32>,
    seen: f64,
    crouched: bool,
}

impl Duck {
    pub const fn new() -> Self {
        Self { standing: None, seen: 0.0, crouched: false }
    }

    /// The head at `height` (metres, up) at `time`: whether the player is crouched.
    pub fn feed(&mut self, time: f64, height: f32, rules: &DuckRules) -> bool {
        if !height.is_finite() {
            return self.crouched;
        }
        let dt = (time - self.seen).clamp(0.0, 0.1) as f32;
        self.seen = time;
        let standing = self.standing.get_or_insert(height);
        if height > *standing {
            *standing = height;
        } else if !self.crouched {
            *standing = (*standing - rules.sink * dt).max(height);
        }
        let drop = *standing - height;
        self.crouched = if self.crouched { drop > rules.stand } else { drop > rules.crouch };
        self.crouched
    }

    pub fn crouched(&self) -> bool {
        self.crouched
    }

    /// Forgets the standing height (a new run, a recentred playspace).
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// How far a hand at `hand` has crossed from its own place `own` toward another place `other`: 0
/// at its own, 1 at the other, through a smoothstep between `from` and `to` of the way (Wolfenstein
/// with two guns: 0.3 to 0.7, each hand reloading the other's gun).
pub fn reach_share(hand: Vec3, own: Vec3, other: Vec3, from: f32, to: f32) -> f32 {
    let distance = |a: Vec3, b: Vec3| length([0, 1, 2].map(|k| a[k] - b[k]));
    let (to_own, to_other) = (distance(hand, own), distance(hand, other));
    let ratio = to_own / (to_own + to_other).max(1e-3);
    let t = ((ratio - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests;
