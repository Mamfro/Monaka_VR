//! A record's [`Seq`] parts in shared memory, written and read without a lock: one writer each,
//! any number of readers. A reader that meets a write in progress tries again a few times, then
//! takes it as a miss (the writer's next write comes within a frame).

use crate::shared::{SharedBlock, SharedRecord};
use monaka_contract::protocol::Seq;
use std::sync::atomic::{AtomicU64, Ordering, fence};

/// How often a reader tries before calling it a miss (a write takes well under a microsecond).
const TRIES: u32 = 64;

/// The part of `block`'s record that `part` picks, written without a lock. Only one side (and
/// one thread there) writes a given part.
pub fn write<R: SharedRecord, T: Copy>(block: &SharedBlock<R>, part: impl FnOnce(*mut R) -> *mut Seq<T>, value: &T) {
    let at = part(block.record_for_writing());
    // SAFETY: `at` is in the block's mapping, which lives as long as the block, and a `Seq`'s
    // counter is an aligned u64 (repr(C)); the value's bytes are plain data.
    unsafe {
        let seq = AtomicU64::from_ptr(&raw mut (*at).seq);
        // Even: no write in progress. A writer that died mid-write left it odd; this one goes on
        // from there.
        let next = (seq.load(Ordering::Relaxed) + 2) & !1;
        seq.store(next - 1, Ordering::Relaxed);
        fence(Ordering::Release);
        std::ptr::write_volatile(&raw mut (*at).value, *value);
        seq.store(next, Ordering::Release);
    }
}

/// The part of `block`'s record that `part` picks, as one whole write left it; `None` while a
/// write stays in progress.
pub fn read<R: SharedRecord, T: Copy>(block: &SharedBlock<R>, part: impl FnOnce(*const R) -> *const Seq<T>) -> Option<T> {
    let at = part(block.record());
    // SAFETY: as in `write`; any bit pattern is a valid value (SharedRecord), so a copy torn by a
    // write is only ever dropped, never used.
    unsafe {
        let seq = AtomicU64::from_ptr((&raw const (*at).seq).cast_mut());
        for _ in 0..TRIES {
            let before = seq.load(Ordering::Acquire);
            if before & 1 == 0 {
                let value = std::ptr::read_volatile(&raw const (*at).value);
                fence(Ordering::Acquire);
                if seq.load(Ordering::Relaxed) == before {
                    return Some(value);
                }
            }
            std::hint::spin_loop();
        }
    }
    None
}

#[cfg(test)]
mod tests;
