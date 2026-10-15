use super::*;

#[test]
fn head_source_prefers_the_synthetic_head() {
    let name = test_channel("source");
    let source = HeadSource::new();
    assert!(source.current().is_none() && !source.connected());
    let mut viewer = PoseChannel::open(&name).unwrap();
    let real = HeadPose { valid: 1, ipd: 0.064, ..HeadPose::default() };
    assert!(viewer.write_head(&real));
    source.open(Some(PoseChannel::open(&name).unwrap()), None);
    assert_eq!(source.current(), Some(real));
    let fake = HeadPose { ipd: 0.07, ..real };
    source.open(Some(PoseChannel::open(&name).unwrap()), Some(Box::new(move || fake)));
    assert_eq!(source.current(), Some(fake), "the synthetic head wins");
    assert!(source.write_pair(5, [fake, fake]), "records still go to the viewer");
    assert_eq!(viewer.read_pair(5).map(|p| p.eyes[0].ipd), Some(0.07));
    source.close();
    assert!(source.current().is_none() && !source.write_pair(6, [fake, fake]));
}

fn test_channel(tag: &str) -> ChannelName {
    ChannelName::new(format!(r"Local\monaka-test-{tag}-{}", std::process::id())).unwrap()
}

#[test]
fn head_and_pairs_travel_between_sides() {
    let name = test_channel("pose");
    let mut writer = PoseChannel::open(&name).unwrap();
    let mut reader = PoseChannel::open(&name).unwrap();
    assert!(reader.read_head().is_none());

    let head = HeadPose { valid: 1, ipd: 0.064, position: [0.0, 1.6, 0.0], ..HeadPose::default() };
    assert!(writer.write_head(&head));
    assert_eq!(reader.read_head(), Some(head));

    let pair = RenderedPair { sequence: 17, eyes: [head, head] };
    assert!(writer.write_pair(&pair));
    assert_eq!(reader.read_pair(17), Some(pair));
    assert!(reader.read_pair(1).is_none());
}
