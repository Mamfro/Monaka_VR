use super::*;
use monaka_contract::protocol::NO_SLOT;

#[test]
fn a_slot_whose_copy_failed_is_never_read_and_is_written_next() {
    let mut h = holding([5, 6, 7, 3, 4]);
    // Pair 8 was marked into slot 3 (the oldest), then its submit failed.
    h.slot_sequences[3] = 8;
    unmark(&mut h, 3);
    assert_eq!(reader_take(&h, 0, 8), Some((2, 7)), "the empty slot is not a pair");
    assert_eq!(choose_slot(&h, 0), Some(3), "the empty slot is the first one written");
}

#[test]
fn a_late_commit_never_goes_back() {
    let mut h = holding([5, 6, 7, 3, 4]);
    commit(&mut h, Commit { slot: 3, sequence: 8, tick: 80 });
    assert_eq!((h.latest_slot, h.sequence, h.tick), (3, 8, 80));
    commit(&mut h, Commit { slot: 1, sequence: 6, tick: 60 });
    assert_eq!((h.latest_slot, h.sequence, h.tick), (3, 8, 80), "an older commit landing late changes nothing");
}

/// No pair yet: no slot is the newest or being read.
fn empty_header() -> FenceHeader {
    FenceHeader { latest_slot: NO_SLOT, reader_slot: NO_SLOT, ..FenceHeader::default() }
}

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
