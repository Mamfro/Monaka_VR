use super::*;

#[test]
fn fingers_travel_and_go_stale() {
    let name = ChannelName::new(format!(r"Local\monaka-test-fingers-{}", std::process::id())).unwrap();
    let mut writer = FingerChannel::open(&name).unwrap();
    let mut reader = FingerChannel::open(&name).unwrap();
    assert!(reader.read(1, 1000).is_none());
    let mut fresh = FingerPose { valid: 1, received_tick: crate::tick(), ..FingerPose::default() };
    fresh.fingers[1] = [0.5, 0.4, 0.3, 0.1];
    assert!(writer.write(&[FingerPose::default(), fresh]));
    assert_eq!(reader.read(1, 1000), Some(fresh));
    assert!(reader.read(0, 1000).is_none(), "an untracked hand is not returned");
    let old = FingerPose { received_tick: crate::tick().saturating_sub(5000), ..fresh };
    assert!(writer.write(&[FingerPose::default(), old]));
    assert!(reader.read(1, 1000).is_none(), "stale fingers are not returned");
}
