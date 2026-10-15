//! Eyes taking turns on the same-frame loop ([`Turns`]): one eye per frame, the left eye's frame
//! starting a pair and the right eye's ending it, both drawn from the head the left eye was.

use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// Which eye the next frame draws, and the head its pair is drawn from.
pub struct Turns {
    /// The eye the next frame draws (0 left).
    next: AtomicUsize,
    /// The head the pair's left eye was drawn from, kept for its right eye.
    head: Mutex<Option<HeadPose>>,
}

impl Turns {
    pub const fn new() -> Self {
        Self { next: AtomicUsize::new(0), head: Mutex::new(None) }
    }

    /// The eye the next frame draws (0 left) and the head it is drawn from: for a left eye `current`
    /// (kept for its right eye when there is one), for a right eye the head its left eye was.
    pub fn frame(&self, current: Option<HeadPose>) -> (usize, Option<HeadPose>) {
        let eye = self.next.load(Relaxed);
        let mut kept = self.head.locked();
        if eye == 1 {
            return (eye, *kept);
        }
        if current.is_some() {
            *kept = current;
        }
        (eye, current)
    }

    /// Eye `eye`'s frame went out: the other eye is next.
    pub fn drawn(&self, eye: usize) {
        self.next.store(if eye == 0 { 1 } else { 0 }, Relaxed);
    }

    /// The next frame starts a pair again (stereo paused).
    pub fn start_over(&self) {
        self.next.store(0, Relaxed);
    }
}

impl Default for Turns {
    fn default() -> Self {
        Self::new()
    }
}
