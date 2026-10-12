//! Head aim: the character's own look angles follow the head, so it looks, aims and attacks where
//! the head points, while mouse or stick turning still works.
//!
//! Re-pitching the camera the game builds moves the view and crosshair but not the arms or
//! attacks. Those read the character's look angles, which are steered each
//! update toward look *targets* that input adds into (DL2: `PlayerDI_PH` +0xb98 yaw, +0xb9c pitch,
//! degrees, yaw turning right). Writing the targets sticks; writing the current angles does not.
//!
//! Each update [`HeadAim::apply`] sets the targets from the head and remembers how much of them
//! is the head's, so the game's own share (mouse, stick, scripted turns) keeps accumulating and is
//! handed back by [`HeadAim::release`]. The game camera then carries the head's yaw from the
//! previous update; the view must turn that share back out ([`BakedYaw`]) before applying the
//! current head pose, or a head turn counts twice.

use core::f32::consts::{PI, TAU};

/// How a game stores look angles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Convention {
    /// Game yaw units per radian of head yaw (positive head yaw turns left): DL2 is `-57.29578`.
    pub yaw_per_radian: f32,
    /// Game pitch units per radian of head pitch (positive up): DL2 is `57.29578`.
    pub pitch_per_radian: f32,
    /// Largest pitch magnitude to write, in game units.
    pub pitch_limit: f32,
}

impl Convention {
    pub const DEGREES_YAW_RIGHT: Self = Self { yaw_per_radian: -57.29578, pitch_per_radian: 57.29578, pitch_limit: 85.0 };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PitchMode {
    /// The head alone pitches the character; vertical look input should be neutralised.
    HeadOnly,
    /// The head's pitch is added to the game's own (mouse/stick pitch keeps working).
    HeadPlusOwn,
}

/// Look targets in game units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub yaw: f32,
    pub pitch: f32,
}

/// The head's share of the look targets of one character.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeadAim {
    owner: usize,
    yaw_share: f32,
    pitch_share: f32,
}

impl HeadAim {
    pub const NONE: Self = Self { owner: 0, yaw_share: 0.0, pitch_share: 0.0 };

    /// The targets to write for this update, and the game's own angles (without the head).
    /// `owner` identifies the character object; a new one starts with no head share. `None`
    /// when the stored values are implausible (nothing should be written).
    pub fn apply(&mut self, owner: usize, stored: Look, head_yaw: f32, head_pitch: f32, mode: PitchMode, c: &Convention) -> Option<(Look, Look)> {
        if owner != self.owner {
            *self = Self { owner, ..Self::default() };
        }
        let own = Look { yaw: stored.yaw - self.yaw_share, pitch: stored.pitch - self.pitch_share };
        let plausible = own.yaw.is_finite() && own.pitch.is_finite() && own.pitch.abs() <= c.pitch_limit * 1.1 + 1.0;
        if !plausible || !head_yaw.is_finite() || !head_pitch.is_finite() {
            *self = Self { owner, ..Self::default() };
            return None;
        }
        // The head's yaw comes within a half turn; past it, the same look one turn off. Kept
        // nearest the yaw already written, so a player standing turned round in the room does not
        // send the targets a full turn off: the game would turn the character all the way round
        // to them.
        let head_yaw = match self.yaw_share / c.yaw_per_radian {
            last if last.is_finite() => last + (head_yaw - last + PI).rem_euclid(TAU) - PI,
            _ => head_yaw,
        };
        let head = head_pitch * c.pitch_per_radian;
        let pitch = match mode {
            PitchMode::HeadOnly => head,
            PitchMode::HeadPlusOwn => own.pitch + head,
        }
        .clamp(-c.pitch_limit, c.pitch_limit);
        self.pitch_share = pitch - own.pitch;
        self.yaw_share = head_yaw * c.yaw_per_radian;
        Some((Look { yaw: own.yaw + self.yaw_share, pitch }, own))
    }

    /// The game wrote the pitch target itself since the last update (a cutscene steering the look):
    /// none of the head's pitch is in it. Counted in anyway, the game's own pitch would drift a
    /// share further every update until it looked implausible and the whole share was dropped, the
    /// yaw's too, drawing frames without the head's turn.
    pub fn pitch_replaced(&mut self) {
        self.pitch_share = 0.0;
    }

    /// The game's own targets to write back when head aim stops, if a share is held on `owner`.
    pub fn release(&mut self, owner: usize, stored: Look) -> Option<Look> {
        let held = owner == self.owner && (self.yaw_share != 0.0 || self.pitch_share != 0.0);
        let own = Look { yaw: stored.yaw - self.yaw_share, pitch: stored.pitch - self.pitch_share };
        *self = Self { owner, ..Self::default() };
        held.then_some(own)
    }

    /// The head yaw (radians, positive left) currently held in the targets.
    pub fn yaw_share_radians(&self, c: &Convention) -> f32 {
        if c.yaw_per_radian == 0.0 { 0.0 } else { self.yaw_share / c.yaw_per_radian }
    }

    pub fn holds_share(&self) -> bool {
        self.yaw_share != 0.0 || self.pitch_share != 0.0
    }

    /// The character the share belongs to (0 before the first one).
    pub fn owner(&self) -> usize {
        self.owner
    }
}

/// The head yaw baked into the game camera of each game frame, by frame counter. Recorded when
/// the game builds its camera, looked up when the view is built from that camera.
#[derive(Clone, Copy, Debug, Default)]
pub struct BakedYaw {
    entries: [(u64, f32); 8],
    next: usize,
}

impl BakedYaw {
    /// No frame matches an empty slot (frame `u64::MAX` is never recorded).
    pub const EMPTY: Self = Self { entries: [(u64::MAX, 0.0); 8], next: 0 };

    pub fn record(&mut self, frame: u64, yaw: f32) {
        self.entries[self.next] = (frame, yaw);
        self.next = (self.next + 1) % self.entries.len();
    }

    /// The yaw recorded for `frame`, else the latest one (`found` false).
    pub fn lookup(&self, frame: u64) -> (f32, bool) {
        match self.entries.iter().find(|(f, _)| *f == frame) {
            Some(&(_, yaw)) => (yaw, true),
            None => (self.entries[(self.next + self.entries.len() - 1) % self.entries.len()].1, false),
        }
    }
}

/// No stick pitch, kept level: while only the head looks up and down (the stick's up and down
/// taken from the game), nothing else would ever bring back a tilt in the game's own pitch (a seat's
/// limits, a cutscene, a climb), so the game's own pitch may only go back toward level. Pitch here
/// is in the game's units with either sign convention, as long as one call uses one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levelling {
    /// Within this of level the game's pitch counts as level.
    pub dead_band: f32,
    /// The most [`Levelling::step`] changes it in one go.
    pub step: f32,
}

impl Levelling {
    /// The change that brings the game's own pitch `own` back toward level, at most
    /// [`Levelling::step`]; none within the dead band. For a game whose own pitch the producer
    /// writes.
    pub fn step(&self, own: f32) -> f32 {
        if own.abs() < self.dead_band { 0.0 } else { (-own).clamp(-self.step, self.step) }
    }

    /// Whether the game's own look input, changing its pitch `own` the positive way (`positive`) or
    /// the negative, takes it back toward level from beyond the dead band. For a game whose look
    /// input the producer can only pass or hold back.
    pub fn toward_level(&self, own: f32, positive: bool) -> bool {
        if positive { own < -self.dead_band } else { own > self.dead_band }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::FRAC_PI_6;

    #[test]
    fn levelling_steps_and_gates_toward_level() {
        let rule = Levelling { dead_band: 0.05, step: 0.25 };
        assert_eq!((rule.step(15.0), rule.step(-15.0), rule.step(0.2), rule.step(0.01)), (-0.25, 0.25, -0.2, 0.0));
        assert!(rule.toward_level(-3.0, true) && !rule.toward_level(-3.0, false));
        assert!(rule.toward_level(3.0, false) && !rule.toward_level(3.0, true));
        assert!(!rule.toward_level(0.04, false) && !rule.toward_level(-0.04, true), "level enough: nothing passes");
    }

    const DL2: Convention = Convention::DEGREES_YAW_RIGHT;

    #[test]
    fn a_pitch_the_game_keeps_writing_keeps_the_yaw_share() {
        // A conversation writes pitch -27 every update over the head's -16; the head looks 120
        // degrees left. Each update the targets carry the yaw written, the pitch the game's.
        let head_yaw = 120f32.to_radians();
        let mut aim = HeadAim::default();
        let (mut write, _) = aim.apply(1, Look { yaw: -80.0, pitch: -27.0 }, head_yaw, -16f32.to_radians(), PitchMode::HeadOnly, &DL2).unwrap();
        for update in 0..30 {
            let stored = Look { yaw: write.yaw, pitch: -27.0 };
            if (stored.pitch - write.pitch).abs() > 0.5 {
                aim.pitch_replaced();
            }
            write = aim.apply(1, stored, head_yaw, -16f32.to_radians(), PitchMode::HeadOnly, &DL2).map(|(w, _)| w).unwrap_or_else(|| panic!("update {update}: share dropped"));
            assert!((aim.yaw_share_radians(&DL2) - head_yaw).abs() < 1e-4, "update {update}");
        }
    }

    #[test]
    fn head_only_pitch_and_added_yaw() {
        let mut aim = HeadAim::default();
        // Mouse at yaw 100, pitch -2.2; head 30 degrees left and 20 down.
        let (write, own) = aim.apply(1, Look { yaw: 100.0, pitch: -2.2 }, FRAC_PI_6, -0.3491, PitchMode::HeadOnly, &DL2).unwrap();
        assert!((write.pitch + 20.0).abs() < 0.01 && (write.yaw - 70.0).abs() < 0.01, "{write:?}");
        assert_eq!(own, Look { yaw: 100.0, pitch: -2.2 });
        // Next update the game added 5 degrees of mouse yaw to what we wrote: it is kept.
        let (write, own) = aim.apply(1, Look { yaw: write.yaw + 5.0, pitch: write.pitch }, FRAC_PI_6, -0.3491, PitchMode::HeadOnly, &DL2).unwrap();
        assert!((own.yaw - 105.0).abs() < 0.01 && (write.yaw - 75.0).abs() < 0.01);
        assert!((aim.yaw_share_radians(&DL2) - FRAC_PI_6).abs() < 1e-4);
        let back = aim.release(1, write).unwrap();
        assert!((back.yaw - 105.0).abs() < 0.01 && (back.pitch + 2.2).abs() < 0.01);
        assert!(aim.release(1, write).is_none(), "released once");
    }

    #[test]
    fn a_head_turning_past_the_half_turn_keeps_the_yaw_near() {
        let mut aim = HeadAim::default();
        let (first, _) = aim.apply(1, Look { yaw: 0.0, pitch: 0.0 }, 179f32.to_radians(), 0.0, PitchMode::HeadOnly, &DL2).unwrap();
        // The head goes on 2 degrees: its yaw reads -179.
        let (next, _) = aim.apply(1, first, (-179f32).to_radians(), 0.0, PitchMode::HeadOnly, &DL2).unwrap();
        assert!((next.yaw - first.yaw + 2.0).abs() < 0.01, "{first:?} then {next:?}");
        // And back.
        let (back, own) = aim.apply(1, next, 179f32.to_radians(), 0.0, PitchMode::HeadOnly, &DL2).unwrap();
        assert!((back.yaw - first.yaw).abs() < 0.01 && own.yaw.abs() < 0.01, "{back:?} {own:?}");
    }

    #[test]
    fn added_pitch_is_clamped_and_new_owners_start_clean() {
        let mut aim = HeadAim::default();
        let (write, _) = aim.apply(1, Look { yaw: 0.0, pitch: 70.0 }, 0.0, 0.5, PitchMode::HeadPlusOwn, &DL2).unwrap();
        assert_eq!(write.pitch, 85.0);
        let (_, own) = aim.apply(2, Look { yaw: 10.0, pitch: 85.0 }, 0.0, 0.0, PitchMode::HeadPlusOwn, &DL2).unwrap();
        assert_eq!(own, Look { yaw: 10.0, pitch: 85.0 }, "another character's values are taken as its own");
        assert!(aim.apply(2, Look { yaw: f32::NAN, pitch: 0.0 }, 0.0, 0.0, PitchMode::HeadOnly, &DL2).is_none());
        assert!(!aim.holds_share());
    }

    #[test]
    fn baked_yaw_by_frame() {
        let mut baked = BakedYaw::default();
        for frame in 1..=10 {
            baked.record(frame, frame as f32);
        }
        assert_eq!(baked.lookup(7), (7.0, true));
        assert_eq!(baked.lookup(1), (10.0, false), "evicted frames fall back to the latest");
    }
}
