//! Melee swings from a tracked controller: the tip of what the hand holds (a point ahead of the
//! palm, along where it points) sweeping fast enough starts a swing, which a producer turns into
//! the game's attack. A swing is over once the tip has slowed down again; only then can the next
//! one start, so one physical swing is one attack. A blade's swing must also be a cut: the tip
//! moving along the edge, not the flat (a slap) or the point (a stab).

use crate::math::{Quat, Vec3, rotate};

/// What makes a swing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwingRules {
    /// How far ahead of the palm the tip is (metres): a weapon's length, about nothing for a fist.
    pub reach: f32,
    /// Which way the weapon runs from the palm, in the controller's frame (x right, y up, z back;
    /// unit): ahead, `-z`, unless the game holds it otherwise (Billie's sword points up and ahead
    /// from her fist). The tip is `reach` along it.
    pub blade: Vec3,
    /// Tip speed that starts a swing (m/s).
    pub start_speed: f32,
    /// Tip speed below which a swing is over (m/s).
    pub settle_speed: f32,
    /// The least time from one swing's start to the next's (seconds).
    pub min_gap: f64,
    /// A tip moving up steeper than this (the up share of its direction) is winding up, not
    /// striking: it starts no swing, and the strike it turns into still can. Counted, a wind-up
    /// would fire the attack on the weapon's way up, leaving the chop after it too soon to count.
    pub max_rise: f32,
    /// A sharp weapon's swings must cut: the tip moving along the edge by at least this share of
    /// its motion. The flat faces the controller's sides (a sword in a hammer grip: the blade in
    /// the plane of thumb and little finger), the edges lie across [`SwingRules::blade`] in that
    /// plane, so a chop, or a slash with the wrist turned, cuts; the flat swung sideways or a
    /// thrust along the blade does not. `None`: a blunt weapon or a fist, any way counts.
    pub cut: Option<f32>,
}

impl SwingRules {
    /// A one-handed weapon: a club's or a machete's sweet spot half a metre out.
    /// The start speed sits between idle hand moves (under 2 m/s) and seated chops (2 m/s and up).
    pub const WEAPON: Self = Self { reach: 0.5, blade: [0.0, 0.0, -1.0], start_speed: 2.2, settle_speed: 0.9, min_gap: 0.3, max_rise: 0.5, cut: None };
    /// A sword or a knife: a weapon's swing that also cuts, the tip moving along the edge for at
    /// least about 45 degrees' worth of its direction (cos 45 = 0.7).
    pub const BLADE: Self = Self { cut: Some(0.7), ..Self::WEAPON };
    /// Bare hands: the punching hand's knuckles.
    pub const FIST: Self = Self { reach: 0.08, blade: [0.0, 0.0, -1.0], start_speed: 1.8, settle_speed: 0.7, min_gap: 0.25, max_rise: 0.6, cut: None };

    /// These rules with the weapon running `blade` (unit, controller frame) from the palm.
    pub fn pointing(self, blade: Vec3) -> Self {
        Self { blade, ..self }
    }

    /// Where the edges face, in the controller's frame: across the blade, in the plane of the
    /// controller's up and back axes (`y` for a blade ahead); `y` if the blade lies sideways.
    fn edge(&self) -> Vec3 {
        let [_, y, z] = self.blade;
        // blade x (1, 0, 0) = (0, z, -y)
        let l = (y * y + z * z).sqrt();
        if l < 1e-3 { [0.0, 1.0, 0.0] } else { [0.0, z / l, -y / l] }
    }

    /// These rules with the speeds scaled by `factor`.
    pub fn faster(self, factor: f32) -> Self {
        Self { start_speed: self.start_speed * factor, settle_speed: self.settle_speed * factor, ..self }
    }
}

/// A swing's start: how fast the tip went and where to (a unit vector, tracking space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Started {
    pub speed: f32,
    pub direction: Vec3,
}

/// What one sample did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    pub started: Option<Started>,
    /// When a motion that got past half the start speed comes to rest: its peak tip speed and
    /// whether it was a swing (for tuning the thresholds from a play log).
    pub settled: Option<(f32, bool)>,
    /// That motion went past the start speed without cutting (a blade's flat or point).
    pub uncut: bool,
}

/// Speeds are measured over at least this long (seconds): the samples' own spacing (a frame) is
/// too short for tracking noise to average out.
const WINDOW: f64 = 0.03;
/// A gap this long between samples (seconds) starts the measuring over.
const STALE: f64 = 0.15;
/// A tip that moves this far between two samples (metres) was a tracking jump, not a swing.
const JUMP: f32 = 0.5;
/// Samples kept, enough for [`WINDOW`] at any frame rate a headset runs.
const KEPT: usize = 16;

/// One hand's swing detector, fed every new controller sample.
#[derive(Clone, Debug, Default)]
pub struct Swing {
    /// (seconds, tip), oldest first.
    samples: Vec<(f64, Vec3)>,
    /// Consecutive samples above the start speed.
    above: u32,
    /// Inside a swing (started, not yet settled).
    swinging: bool,
    last_start: Option<f64>,
    /// The current motion's peak speed, while it is past half the start speed.
    peak: f32,
    /// The current motion went past the start speed without cutting.
    uncut: bool,
}

impl Swing {
    pub const fn new() -> Self {
        Self { samples: Vec::new(), above: 0, swinging: false, last_start: None, peak: 0.0, uncut: false }
    }

    /// Whether a swing is under way: started, its tip not yet slowed below the settle speed.
    pub fn swinging(&self) -> bool {
        self.swinging
    }

    /// Forgets the motion so far (the controller went away, or nothing is held that swings).
    pub fn reset(&mut self) {
        self.samples.clear();
        self.above = 0;
        self.swinging = false;
        self.peak = 0.0;
        self.uncut = false;
    }

    /// A new controller sample at `time` (seconds, any steady clock): its palm and where it points.
    pub fn feed(&mut self, time: f64, palm: Vec3, pointer: Quat, rules: &SwingRules) -> Step {
        let out = rotate(pointer, rules.blade);
        let tip = [0, 1, 2].map(|k| palm[k] + out[k] * rules.reach);
        if let Some(&(last_time, last_tip)) = self.samples.last() {
            if time <= last_time {
                return Step::default();
            }
            if time - last_time > STALE || crate::math::distance(tip, last_tip) > JUMP || !tip.iter().all(|v| v.is_finite()) {
                self.reset();
            }
        }
        if !tip.iter().all(|v| v.is_finite()) {
            return Step::default();
        }
        if self.samples.len() == KEPT {
            self.samples.remove(0);
        }
        self.samples.push((time, tip));
        // The newest sample against the latest one at least a window older.
        let Some(&(then, from)) = self.samples.iter().rev().find(|(t, _)| time - t >= WINDOW) else { return Step::default() };
        let elapsed = (time - then) as f32;
        let moved = [tip[0] - from[0], tip[1] - from[1], tip[2] - from[2]];
        let speed = (moved[0] * moved[0] + moved[1] * moved[1] + moved[2] * moved[2]).sqrt() / elapsed;
        let mut step = Step::default();
        if speed >= rules.start_speed * 0.5 {
            self.peak = self.peak.max(speed);
        }
        let rising = moved[1] / (speed * elapsed) > rules.max_rise;
        // Along the edge, either way.
        let edge = rotate(pointer, rules.edge());
        let cutting = rules.cut.is_none_or(|share| (moved[0] * edge[0] + moved[1] * edge[1] + moved[2] * edge[2]).abs() >= share * speed * elapsed);
        if speed >= rules.start_speed && !rising && !cutting {
            self.uncut = true;
        }
        self.above = if speed >= rules.start_speed && !rising && cutting { self.above + 1 } else { 0 };
        let rested = self.last_start.is_none_or(|start| time - start >= rules.min_gap);
        if !self.swinging && self.above >= 2 && rested {
            self.swinging = true;
            self.last_start = Some(time);
            let length = speed * elapsed;
            step.started = Some(Started { speed, direction: moved.map(|v| v / length) });
        }
        if speed < rules.settle_speed {
            if self.peak > 0.0 {
                step.settled = Some((self.peak, self.swinging));
                step.uncut = self.uncut && !self.swinging;
            }
            self.swinging = false;
            self.peak = 0.0;
            self.uncut = false;
        }
        step
    }
}

#[cfg(test)]
mod tests;
