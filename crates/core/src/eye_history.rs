//! Temporal history per eye. An upscaler, an anti-aliasing pass or a motion pass carries state from
//! the previous frame (its accumulated image, the previous view-projection, last frame's motion
//! vectors). With the eyes taking turns, or both drawn in one frame, the game's "previous frame" is
//! the other eye's, and the history of the two eyes blends (shimmer, smearing, a ghost of the other
//! eye's view). Each eye must keep its own, and each must start over when its own chain breaks.
//!
//! [`EyeHistory`] decides, frame by frame, whether an eye's history carries on or starts over, and
//! keeps a value per eye for the producer (a matrix, a resource, a context): what the engine
//! stores stays the producer's. The rules are the same everywhere:
//!
//! - an eye's first frame starts over ([`Reset::First`]);
//! - a frame whose size or format differs from that eye's last one starts over ([`Reset::Shape`]);
//! - a frame shown without stereo (a flat frame, a skipped one) breaks both eyes' chains
//!   ([`EyeHistory::break_chain`], [`Reset::Broken`]);
//! - the game's own reset (a cut, a teleport) passes through ([`Reset::Requested`]);
//! - where the history is chained through the frame before, which must then be the other eye's
//!   (motion vectors over two frames, alternate eyes), the same eye twice running starts over
//!   ([`Rules::chained`], [`Reset::Repeat`]).
//!
//! An engine that keeps its temporal state per object (and only one, the last eye's) gets a copy
//! per object and eye under the same rules from a [`Bank`].

/// Why an eye's history starts over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reset {
    First,
    Shape,
    Broken,
    Requested,
    Repeat,
}

impl Reset {
    pub const ALL: [Reset; 5] = [Reset::First, Reset::Shape, Reset::Broken, Reset::Requested, Reset::Repeat];

    pub const fn label(self) -> &'static str {
        match self {
            Reset::First => "first frame",
            Reset::Shape => "size or format changed",
            Reset::Broken => "a flat or skipped frame",
            Reset::Requested => "the game's own reset",
            Reset::Repeat => "the same eye twice",
        }
    }
}

/// What an eye's history needs to carry on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// The history reads the frame before this one, which must be the other eye's (alternate
    /// eyes with motion vectors chained over two frames): the same eye twice running starts over.
    /// Off where each eye only reads its own previous frame (a previous view-projection per eye).
    pub chained: bool,
}

impl Rules {
    /// Each eye reads only its own previous frame.
    pub const OWN: Self = Self { chained: false };
    /// Each eye's frame is read through the other eye's frame just before it.
    pub const CHAINED: Self = Self { chained: true };
}

/// One frame's decision for its eye.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub eye: usize,
    /// `None`: the eye's history carries on.
    pub reset: Option<Reset>,
    /// The frame before was the other eye's (what a chained history reads through).
    pub after_other_eye: bool,
}

/// Each eye's history: the shape (`S`: size, format, whatever must stay the same for a history to
/// carry on) of its last frame and a value the producer keeps for it (`T`).
#[derive(Clone, Debug)]
pub struct EyeHistory<S: Copy + PartialEq, T> {
    rules: Rules,
    shapes: [Option<S>; 2],
    kept: [Option<T>; 2],
    /// A break waiting for each eye's next frame.
    broken: [bool; 2],
    /// The eye of the frame before, `None` after a break.
    last: Option<usize>,
    resets: [u64; Reset::ALL.len()],
    frames: u64,
}

impl<S: Copy + PartialEq, T> EyeHistory<S, T> {
    pub const fn new(rules: Rules) -> Self {
        Self { rules, shapes: [None, None], kept: [None, None], broken: [false; 2], last: None, resets: [0; Reset::ALL.len()], frames: 0 }
    }

    /// A frame for `eye` (0 left, 1 right) of `shape`; `requested`: the game asked for a reset.
    /// On a reset the eye's kept value goes.
    pub fn frame(&mut self, eye: usize, shape: S, requested: bool) -> Step {
        let eye = eye.min(1);
        let after_other_eye = self.last == Some(1 - eye);
        let reset = if requested {
            Some(Reset::Requested)
        } else if self.shapes[eye].is_none() {
            Some(Reset::First)
        } else if self.shapes[eye] != Some(shape) {
            Some(Reset::Shape)
        } else if self.broken[eye] {
            Some(Reset::Broken)
        } else if self.rules.chained && self.last == Some(eye) {
            Some(Reset::Repeat)
        } else {
            None
        };
        if let Some(reset) = reset {
            self.kept[eye] = None;
            self.resets[reset as usize] += 1;
        }
        self.shapes[eye] = Some(shape);
        self.broken[eye] = false;
        self.last = Some(eye);
        self.frames += 1;
        Step { eye, reset, after_other_eye }
    }

    /// A frame without stereo went by (flat, mono, skipped): both eyes' chains break, and their
    /// kept values go.
    pub fn break_chain(&mut self) {
        self.broken = [true; 2];
        self.kept = [None, None];
        self.last = None;
    }

    /// The value kept for `eye` (from its previous frame), unless its history started over since.
    pub fn previous(&self, eye: usize) -> Option<&T> {
        self.kept[eye.min(1)].as_ref()
    }

    /// [`EyeHistory::previous`], to change in place.
    pub fn previous_mut(&mut self, eye: usize) -> Option<&mut T> {
        self.kept[eye.min(1)].as_mut()
    }

    /// Keeps `value` for `eye`'s next frame.
    pub fn keep(&mut self, eye: usize, value: T) {
        self.kept[eye.min(1)] = Some(value);
    }

    /// Takes the value kept for `eye` out (a resource to reuse).
    pub fn take(&mut self, eye: usize) -> Option<T> {
        self.kept[eye.min(1)].take()
    }

    /// Forgets everything (the producer stops, or the game remade its pass).
    pub fn clear(&mut self) {
        self.shapes = [None, None];
        self.kept = [None, None];
        self.broken = [false; 2];
        self.last = None;
    }

    /// Resets so far, by reason.
    pub fn resets(&self, reason: Reset) -> u64 {
        self.resets[reason as usize]
    }

    /// `"1200 frames, 4 resets (first frame 2, a flat or skipped frame 2)"`.
    pub fn summary(&self) -> String {
        let total: u64 = self.resets.iter().sum();
        let reasons: Vec<String> = Reset::ALL.iter().filter(|r| self.resets[**r as usize] > 0).map(|r| format!("{} {}", r.label(), self.resets[*r as usize])).collect();
        if reasons.is_empty() { format!("{} frames, no resets", self.frames) } else { format!("{} frames, {total} resets ({})", self.frames, reasons.join(", ")) }
    }
}

/// An engine's own temporal state kept per eye, where the engine keeps it per object (a camera
/// history per render tree, a pass's previous camera): a `BYTES` copy per (owner, key) and eye,
/// seeded from the engine's own whenever that eye's history starts over. Each entry's history is an
/// [`EyeHistory`] whose shape is the caller's epoch, so a new epoch (a flat or skipped frame, a
/// reset, a new camera) starts both eyes over. Copies are 16-byte aligned (an engine may read them
/// with aligned loads) and stay where they are for the bank's life, so the engine may be handed
/// one to read and write in place of its own.
pub struct Bank<const BYTES: usize> {
    entries: Vec<Banked<BYTES>>,
}

struct Banked<const BYTES: usize> {
    owner: usize,
    key: u32,
    /// The epoch of the entry's last use.
    epoch: u32,
    history: EyeHistory<u32, crate::Aligned16<[u8; BYTES]>>,
}

impl<const BYTES: usize> Bank<BYTES> {
    /// Room for `capacity` (owner, key) pairs; an entry of an older epoch is reused for a new pair.
    pub fn new(capacity: usize) -> Self {
        Self { entries: (0..capacity).map(|_| Banked { owner: 0, key: 0, epoch: 0, history: EyeHistory::new(Rules::OWN) }).collect() }
    }

    /// `eye`'s (0 left, 1 right) copy for (`owner`, `key`) in `epoch`, seeded from `seed` when its
    /// history starts over. `None` for owner 0, another eye, or a bank full of this epoch's entries.
    pub fn get(&mut self, owner: usize, key: u32, epoch: u32, eye: usize, seed: &[u8; BYTES]) -> Option<&mut [u8; BYTES]> {
        if owner == 0 || eye > 1 {
            return None;
        }
        let index = match self.entries.iter().position(|e| e.owner == owner && e.key == key) {
            Some(i) => i,
            None => {
                let i = self.entries.iter().position(|e| e.owner == 0 || e.epoch != epoch)?;
                let e = &mut self.entries[i];
                (e.owner, e.key) = (owner, key);
                e.history.clear();
                i
            }
        };
        let entry = &mut self.entries[index];
        entry.epoch = epoch;
        entry.history.frame(eye, epoch, false);
        if entry.history.previous(eye).is_none() {
            entry.history.keep(eye, crate::Aligned16(*seed));
        }
        entry.history.previous_mut(eye).map(|copy| &mut copy.0)
    }

    /// Stores `current` as `eye`'s copy; whether there was room for it.
    pub fn store(&mut self, owner: usize, key: u32, epoch: u32, eye: usize, current: &[u8; BYTES]) -> bool {
        match self.get(owner, key, epoch, eye, current) {
            Some(copy) => {
                *copy = *current;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests;
