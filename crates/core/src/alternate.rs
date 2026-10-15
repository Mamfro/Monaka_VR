//! Alternate-eye rendering: every presented frame is a full render for one eye, the eyes taking
//! turns, and each left/right pair of frames is published as one stereo pair.
//!
//! Presents are counted from attachment (`n`). The camera written after present `n` is for eye
//! `n & 1` and shows `latency` presents later. A pair is published at the right-eye present, with
//! the head poses its two frames were rendered from, kept in a small ring by present number.

/// Size of the pose ring; must be larger than `latency + 1`.
pub const POSE_RING: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// Presents between writing a camera and seeing it on screen (Dying Light 1: 2).
    pub latency: u64,
}

impl Schedule {
    pub const MAX_LATENCY: u64 = 8;

    pub fn new(latency: u64) -> Option<Self> {
        (latency <= Self::MAX_LATENCY).then_some(Self { latency })
    }

    /// `latency=<presents>` from the options, in range, else `default` (the game's own).
    pub fn from_options(options: &crate::options::Options, default: u64) -> Self {
        options.value::<u64>("latency").and_then(Self::new).unwrap_or(Self { latency: default })
    }

    /// The eye whose camera is written after present `n`: left on even presents.
    pub fn camera_eye(n: u64) -> usize {
        (n & 1) as usize
    }

    /// Whether the head pose for a pair is sampled after present `n` (its left-eye frame).
    pub fn starts_pair(n: u64) -> bool {
        n & 1 == 0
    }

    /// The eye present `n` shows, by parity alone (also before the first written camera shows).
    pub fn eye_at(&self, n: u64) -> usize {
        (n.wrapping_sub(self.latency) & 1) as usize
    }

    /// The eye present `n` shows, once the first written camera has reached the screen.
    pub fn shown_eye(&self, n: u64) -> Option<usize> {
        (n > self.latency).then(|| self.eye_at(n))
    }

    /// Pose ring slot written after present `n`.
    pub fn ring_slot(n: u64) -> usize {
        (n % POSE_RING as u64) as usize
    }

    /// The present after which the camera of the frame shown at present `n` was chosen (the key
    /// its cameras were recorded under).
    pub fn chosen_at(&self, n: u64) -> u64 {
        n.wrapping_sub(self.latency)
    }

    /// The debug frame keys (left, right) of the pair completed by right-eye present `n`: each
    /// frame's [`Schedule::chosen_at`] plus one, which is what the driver's drawing present number
    /// reads while that frame is drawn (`monaka_contract::debug_draw`; 0 stays "no frame").
    pub fn pair_keys(&self, n: u64) -> [u64; 2] {
        [self.chosen_at(n.wrapping_sub(1)).wrapping_add(1), self.chosen_at(n).wrapping_add(1)]
    }

    /// Ring slots (left, right) of the poses behind the pair completed by right-eye present `n`.
    pub fn pair_slots(&self, n: u64) -> [usize; 2] {
        [Self::ring_slot(n.wrapping_sub(1 + self.latency)), Self::ring_slot(n.wrapping_sub(self.latency))]
    }
}

/// Values kept by present number (a frame's cameras, its jitter) in a ring of [`POSE_RING`]: each
/// is given back only for the present it was put under, so a slot reused by a later present does
/// not pass for an earlier one.
#[derive(Clone, Copy, Debug)]
pub struct PresentRing<T: Copy> {
    slots: [Option<(u64, T)>; POSE_RING],
}

impl<T: Copy> PresentRing<T> {
    pub const fn new() -> Self {
        Self { slots: [None; POSE_RING] }
    }

    pub fn put(&mut self, n: u64, value: T) {
        self.slots[Schedule::ring_slot(n)] = Some((n, value));
    }

    pub fn get(&self, n: u64) -> Option<T> {
        self.slots[Schedule::ring_slot(n)].filter(|(at, _)| *at == n).map(|(_, value)| value)
    }
}

impl<T: Copy> Default for PresentRing<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
