//! Slot rules of the fence stereo channel, kept separate from the GPU calls so they can be tested.
//!
//! The producer copies each pair into one of [`FENCE_SLOTS`] slots, marks the slot with the pair's
//! sequence before the copy is submitted, and signals the `produced` fence with that sequence. The
//! reader takes the newest pair whose copy has completed (not only the newest pair: a GPU running
//! frames behind keeps the newest unfinished whenever the reader looks); it records which slot it is
//! copying and signals the `consumed` fence when done. Neither side waits. All header reads and
//! writes happen under the header mutex.

use monaka_contract::protocol::{FENCE_SLOTS, FenceHeader};

/// A slot the producer must not overwrite: the newest pair, or the slot the reader is still
/// copying from.
pub fn slot_busy(header: &FenceHeader, slot: u32, consumed_completed: u64) -> bool {
    if header.sequence != 0 && slot == header.latest_slot {
        return true;
    }
    slot == header.reader_slot && consumed_completed < header.reader_claim
}

/// The slot the producer should write next: the one holding the oldest pair (finished pairs newer
/// than it stay readable while newer ones are still being copied); `None` skips the pair.
pub fn choose_slot(header: &FenceHeader, consumed_completed: u64) -> Option<u32> {
    (0..FENCE_SLOTS as u32).filter(|&slot| !slot_busy(header, slot, consumed_completed)).min_by_key(|&slot| header.slot_sequences[slot as usize])
}

/// The slot and pair the reader should copy: the newest pair newer than `seen` whose copy has
/// completed on the GPU (`produced_completed`).
pub fn reader_take(header: &FenceHeader, seen: u64, produced_completed: u64) -> Option<(u32, u64)> {
    (0..FENCE_SLOTS as u32)
        .map(|slot| (slot, header.slot_sequences[slot as usize]))
        .filter(|&(_, sequence)| sequence != 0 && sequence > seen && sequence <= produced_completed)
        .max_by_key(|&(_, sequence)| sequence)
}

/// A submitted pair's entry in the header: which slot holds the newest pair, and its sequence and
/// tick. The reader takes pairs by their slot marks alone, so a commit may land a publish late (the
/// producer never waits for the header lock on the game's thread); until it lands, the previous
/// newest pair stays protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Commit {
    pub slot: u32,
    pub sequence: u64,
    pub tick: u64,
}

/// Writes `commit` into the header, unless a newer pair is committed already.
pub fn commit(header: &mut FenceHeader, commit: Commit) {
    if commit.sequence > header.sequence {
        (header.latest_slot, header.sequence, header.tick) = (commit.slot, commit.sequence, commit.tick);
    }
}

/// Empties `slot` after its copy failed to be submitted: the reader never takes it (its mark no
/// longer names a pair, whatever the slot holds now) and the producer writes it next.
pub fn unmark(header: &mut FenceHeader, slot: u32) {
    header.slot_sequences[slot as usize] = 0;
    header.slot_ticks[slot as usize] = 0;
}

#[cfg(test)]
mod tests;
