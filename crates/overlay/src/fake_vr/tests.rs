use super::*;

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)
}

#[test]
fn the_hands_turn_with_the_head_and_mirror_each_other() {
    let mut fake = FakeVr::desktop();
    let [left, right] = fake.controllers().map(|c| c.expect("held forward"));
    assert!(close(right.0, [0.2, -0.35, -0.35]) && close(left.0, [-0.2, -0.35, -0.35]), "mirrored: {left:?} {right:?}");
    // A quarter turn to the left: the right hand, forward and right, is now forward and left.
    fake.head[0] = 90.0;
    let right = fake.controllers()[1].expect("held").0;
    assert!(close(right, [-0.35, -0.35, -0.2]), "{right:?}");
    // The tilt does not move the hands (a body does not tilt with the head).
    fake.head = [0.0, 40.0];
    assert!(close(fake.controllers()[1].expect("held").0, [0.2, -0.35, -0.35]));
}

#[test]
fn off_is_no_controllers_and_the_headset_fakes_nothing() {
    let fake = FakeVr { preset: Preset::Off, hands: Preset::Off.hands(), ..FakeVr::desktop() };
    assert_eq!(fake.controllers(), [None, None]);
    assert_eq!(FakeVr::HEADSET.controllers(), [None, None]);
    assert!(Preset::ALL.iter().all(|p| *p == Preset::Off || p.hands().is_some()));
}
