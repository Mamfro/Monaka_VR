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
    assert_eq!(schedule.shown_eye(3), Some(1));
    assert_eq!(schedule.shown_eye(4), Some(0));
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
