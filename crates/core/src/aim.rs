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

use crate::math::wrap_radians;

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
            last if last.is_finite() => last + wrap_radians(head_yaw - last),
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

    /// The game wrote the targets itself: none of the head's share is in them any more. The share
    /// is dropped without writing anything back (the targets are the game's as they are).
    pub fn forget(&mut self) {
        *self = Self { owner: self.owner, ..Self::default() };
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
mod tests;
