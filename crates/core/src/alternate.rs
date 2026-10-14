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
    /// reads while that frame is drawn (`monaka_core::debug_draw`; 0 stays "no frame").
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
mod tests {
    use super::*;

    #[test]
    fn a_present_ring_gives_back_only_its_own_present() {
        let mut ring = PresentRing::new();
        ring.put(3, 'a');
        assert_eq!((ring.get(3), ring.get(4)), (Some('a'), None));
        // The same slot, a ring later: the earlier present is gone, not answered with the later value.
        ring.put(3 + POSE_RING as u64, 'b');
        assert_eq!((ring.get(3), ring.get(3 + POSE_RING as u64)), (None, Some('b')));
        assert_eq!(Schedule { latency: 2 }.chosen_at(10), 8);
    }

    #[test]
    fn eyes_follow_the_written_cameras() {
        let schedule = Schedule::new(2).unwrap();
        assert!(Schedule::new(9).is_none());
        // Cameras written after presents 0,1,2,3 (left,right,left,right) show at 2,3,4,5.
        assert_eq!(schedule.shown_eye(2), None);
        assert_eq!(schedule.shown_eye(3), Some(Schedule::camera_eye(1)));
        assert_eq!(schedule.shown_eye(4), Some(Schedule::camera_eye(2)));
        assert_eq!(schedule.shown_eye(5), Some(1));
        assert!(Schedule::starts_pair(2) && !Schedule::starts_pair(3));
    }

    #[test]
    fn pair_poses_come_from_the_frames_shown() {
        let schedule = Schedule::new(2).unwrap();
        // Present 5 shows the right eye written after present 3; its left eye was written after 2.
        assert_eq!(schedule.pair_slots(5), [2, 3]);
        assert_eq!(schedule.pair_slots(17), [14, 15]);
        assert_eq!(schedule.pair_slots(19), [0, 1]);
    }
}
