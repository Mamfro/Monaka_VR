use super::*;

#[test]
fn alternating_eyes_carry_on_and_each_keeps_its_own() {
    let mut h: EyeHistory<(u32, u32), &str> = EyeHistory::new(Rules::CHAINED);
    assert_eq!(h.frame(0, (100, 100), false).reset, Some(Reset::First));
    h.keep(0, "left 1");
    assert_eq!(h.frame(1, (100, 100), false).reset, Some(Reset::First));
    h.keep(1, "right 1");
    let step = h.frame(0, (100, 100), false);
    assert_eq!(step.reset, None);
    assert!(step.after_other_eye);
    assert_eq!(h.previous(0), Some(&"left 1"), "the left eye reads its own, not the right's");
}

#[test]
fn a_repeat_breaks_a_chained_history_but_not_an_eye_s_own() {
    let mut chained: EyeHistory<u32, ()> = EyeHistory::new(Rules::CHAINED);
    let mut own: EyeHistory<u32, ()> = EyeHistory::new(Rules::OWN);
    for h in [&mut chained, &mut own] {
        h.frame(0, 1, false);
        h.frame(1, 1, false);
    }
    assert_eq!(chained.frame(1, 1, false).reset, Some(Reset::Repeat));
    assert_eq!(own.frame(1, 1, false).reset, None);
}

#[test]
fn a_flat_frame_or_a_new_size_starts_over() {
    let mut h: EyeHistory<u32, u8> = EyeHistory::new(Rules::OWN);
    h.frame(0, 1, false);
    h.keep(0, 7);
    h.frame(1, 1, false);
    h.break_chain();
    assert_eq!(h.previous(0), None);
    assert_eq!(h.frame(0, 1, false).reset, Some(Reset::Broken));
    assert_eq!(h.frame(1, 2, false).reset, Some(Reset::Shape));
    assert_eq!(h.frame(0, 1, true).reset, Some(Reset::Requested));
    assert_eq!(h.frame(1, 2, false).reset, None);
    assert_eq!(h.summary(), "6 frames, 5 resets (first frame 2, size or format changed 1, a flat or skipped frame 1, the game's own reset 1)");
}

#[test]
fn a_bank_seeds_then_keeps_each_eye() {
    let mut bank = Bank::<4>::new(2);
    let seed = [1u8, 2, 3, 4];
    let left = bank.get(10, 7, 1, 0, &seed).unwrap();
    assert_eq!(left.as_ptr() as usize % 16, 0, "copies are 16-byte aligned");
    left[0] = 9;
    assert_eq!(bank.get(10, 7, 1, 0, &[0; 4]).unwrap()[0], 9, "the same epoch keeps the copy");
    assert_eq!(bank.get(10, 7, 1, 1, &[5; 4]).unwrap()[0], 5, "the other eye seeds its own");
    assert_eq!(bank.get(10, 7, 2, 0, &seed).unwrap()[0], 1, "a new epoch reseeds");
    assert_eq!(bank.get(10, 7, 2, 1, &[6; 4]).unwrap()[0], 6, "both eyes");
    assert!(bank.store(10, 7, 2, 1, &[8; 4]));
    assert_eq!(bank.get(10, 7, 2, 1, &[0; 4]).unwrap()[0], 8);
    assert!(bank.get(0, 7, 2, 0, &seed).is_none() && bank.get(10, 7, 2, 2, &seed).is_none(), "owner 0 and eye 2 are refused");
}

#[test]
fn a_full_bank_takes_an_older_epoch_s_entry() {
    let mut bank = Bank::<1>::new(2);
    bank.get(1, 0, 1, 0, &[1]);
    bank.get(2, 0, 1, 0, &[2]);
    assert!(bank.get(3, 0, 1, 0, &[3]).is_none(), "full of this epoch's entries");
    assert_eq!(bank.get(3, 0, 2, 0, &[3]).unwrap()[0], 3);
    let moved = bank.get(1, 0, 2, 0, &[4]).unwrap();
    assert_eq!(moved[0], 4, "the other old entry is reused, its copy seeded afresh");
}
