use super::*;

#[test]
fn updates_take_choices_in_turn_through_the_wobble() {
    let mut s = Sequencer::NEW;
    assert_eq!(s.next(1), (1, Turn::InStep));
    assert_eq!(s.next(2), (2, Turn::InStep));
    // Early: a second update before the present making choice 3.
    assert_eq!(s.next(2), (3, Turn::Early));
    // Late: two presents before this update; it still takes the next choice, not the newest.
    assert_eq!(s.next(4), (4, Turn::InStep));
    assert_eq!(s.next(6), (5, Turn::Late));
    assert_eq!(s.next(6), (6, Turn::InStep));
}

#[test]
fn a_lasting_offset_is_followed() {
    let mut s = Sequencer::NEW;
    assert_eq!(s.next(1).0, 1);
    // A present had no camera update: from here every update is one late.
    let mut made = 2;
    let mut last = (0, Turn::InStep);
    for _ in 0..Sequencer::RESYNC_AFTER {
        made += 1;
        last = s.next(made);
    }
    assert_eq!(last, (made, Turn::Resynced));
    assert_eq!(s.next(made + 1), (made + 1, Turn::InStep));
}
