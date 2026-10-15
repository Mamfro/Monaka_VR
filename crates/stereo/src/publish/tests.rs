use super::*;
use monaka_channel::ChannelName;
use monaka_channel::pose::PoseChannel;

fn channel(tag: &str) -> ChannelName {
    ChannelName::new(format!(r"Local\monaka-test-stereo-{tag}-{}", std::process::id())).unwrap()
}

/// What the viewer does with a pair it has read: projects it with the record's poses when every
/// eye has one, else shows it on its flat screen (no record counts as flat).
fn viewer_projects(viewer: &mut PoseChannel, sequence: u64) -> bool {
    viewer.read_pair(sequence).is_some_and(|p| p.eyes.iter().all(|e| e.valid != 0))
}

#[test]
fn a_flat_frame_is_never_projected_with_an_old_record() {
    let name = channel("flat");
    let mut viewer = PoseChannel::open(&name).unwrap();
    let head = HeadSource::new();
    head.open(Some(PoseChannel::open(&name).unwrap()), None);
    let posed = HeadPose { valid: 1, ipd: 0.064, ..HeadPose::default() };
    // A pair of an earlier channel at the sequence the new channel's first pair gets.
    record_first(&head, 1, [posed, posed], |_| {});
    assert!(viewer_projects(&mut viewer, 1));
    // The new channel's first pair is a flat frame: its record replaces the old one.
    let written = record_first(&head, 1, FLAT, |p| p.valid = 1);
    assert_eq!(written, FLAT, "the report leaves eyes without a pose alone");
    assert!(!viewer_projects(&mut viewer, 1), "shown flat");
    // Pairs with poses are projected, with the report applied.
    let written = record_first(&head, 2, [posed, posed], |p| p.ipd = 0.07);
    assert_eq!(written[1].ipd, 0.07);
    assert!(viewer_projects(&mut viewer, 2));
}

#[test]
fn publishes_are_counted() {
    let mut tally = Tally::NEW;
    assert_eq!(tally.count(Ok(true)).unwrap(), Step::Published);
    assert_eq!(tally.count(Ok(false)).unwrap(), Step::Failed);
    assert!(tally.count(Err(monaka_channel::Error::Busy)).is_err());
    assert_eq!((tally.pairs, tally.failures), (1, 1), "an error is the caller's to count");
}
