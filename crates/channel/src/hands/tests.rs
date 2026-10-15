use super::*;
use monaka_contract::protocol::RIGHT_HAND;

#[test]
fn hands_travel_and_go_stale() {
    let name = ChannelName::new(format!(r"Local\monaka-test-hands-{}", std::process::id())).unwrap();
    let mut writer = HandChannel::open(&name).unwrap();
    let mut reader = HandChannel::open(&name).unwrap();
    assert!(reader.read(RIGHT_HAND, 1000).is_none());
    let fresh = HandPose { valid: 1, position: [0.2, -0.3, -0.4], received_tick: crate::tick(), ..HandPose::default() };
    assert!(writer.write(&[HandPose::default(), fresh], &[HandPose::default(), fresh]));
    assert_eq!(reader.read(RIGHT_HAND, 1000), Some(fresh));
    assert_eq!(reader.read_grip(RIGHT_HAND, 1000), Some(fresh));
    let grip=HandPose { position:[0.3,-0.4,-0.5],..fresh };
    assert!(writer.write(&[HandPose::default(),fresh], &[HandPose::default(),grip]));
    assert_eq!(reader.read_palms(1000),[None,Some(HandPose { position:grip.position,..fresh })]);
    assert!(reader.read(0, 1000).is_none(), "an invalid hand is not returned");
    let old = HandPose { received_tick: crate::tick().saturating_sub(5000), ..fresh };
    assert!(writer.write(&[HandPose::default(), old], &[HandPose::default(); 2]));
    assert!(reader.read(RIGHT_HAND, 1000).is_none(), "a stale hand is not returned");
    assert_eq!(reader.read_palms(1000),[None,None]);
    assert!(reader.read(2, 1000).is_none());
}
