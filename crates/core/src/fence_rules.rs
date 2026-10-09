//! Slot rules of the fence stereo channel, kept separate from the GPU calls so they can be tested.
//!
//! The producer copies each pair into one of [`FENCE_SLOTS`] slots, marks the slot with the pair's
//! sequence before the copy is submitted, and signals the `produced` fence with that sequence. The
//! reader takes the newest pair whose copy has completed (not only the newest pair: a GPU running
//! frames behind keeps the newest unfinished whenever the reader looks); it records which slot it is
//! copying and signals the `consumed` fence when done. Neither side waits. All header reads and
//! writes happen under the header mutex.

use crate::protocol::{FENCE_SLOTS, FenceHeader, NO_SLOT};

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

/// No pair yet: no slot is the newest or being read.
pub fn empty_header() -> FenceHeader {
    FenceHeader { latest_slot: NO_SLOT, reader_slot: NO_SLOT, ..FenceHeader::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header whose slots hold these pairs, the newest being `latest`.
    fn holding(sequences: [u64; FENCE_SLOTS]) -> FenceHeader {
        let latest = (0..FENCE_SLOTS).max_by_key(|&s| sequences[s]).unwrap();
        FenceHeader { slot_sequences: sequences, sequence: sequences[latest], latest_slot: latest as u32, ..empty_header() }
    }

    #[test]
    fn first_pair_goes_to_slot_zero() {
        assert_eq!(choose_slot(&empty_header(), 0), Some(0));
        assert_eq!(reader_take(&empty_header(), 0, 0), None);
    }

    #[test]
    fn the_oldest_pair_is_overwritten_never_the_newest() {
        let h = holding([5, 6, 7, 3, 4]);
        assert_eq!(choose_slot(&h, 0), Some(3));
        let h = holding([5, 9, 7, 8, 6]);
        assert_eq!(choose_slot(&h, 0), Some(0));
    }

    #[test]
    fn skips_the_slot_being_read() {
        let mut h = holding([5, 6, 7, 3, 4]);
        (h.reader_slot, h.reader_claim) = (3, 3);
        assert_eq!(choose_slot(&h, 2), Some(4));
        // Once the reader's copy has completed, its slot is free again.
        assert_eq!(choose_slot(&h, 3), Some(3));
    }

    #[test]
    fn the_reader_takes_the_newest_finished_pair() {
        // Pairs 8 and 9 are still being copied; 7 is the newest finished one.
        let h = holding([5, 9, 7, 8, 6]);
        assert_eq!(reader_take(&h, 4, 7), Some((2, 7)));
        assert_eq!(reader_take(&h, 7, 7), None, "nothing newer than what was shown");
        assert_eq!(reader_take(&h, 7, 9), Some((1, 9)));
    }
}
