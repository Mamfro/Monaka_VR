use super::*;
use monaka_channel::d3d::{create_device, create_texture, simple_texture_desc};
use monaka_channel::pose::PoseChannel;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

#[test]
fn flat_frames_keep_the_pairs_pace_and_carry_an_empty_record() {
    let name = ChannelName::new(format!(r"Local\monaka-test-alternate-flat-{}", std::process::id())).unwrap();
    let (device, context, _) = create_device(None).unwrap();
    let image = create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
    let mut viewer = PoseChannel::open(&name).unwrap();
    let head = HeadSource::new();
    head.open(Some(PoseChannel::open(&name).unwrap()), None);
    // An earlier run's record at the sequence this run's first pair gets.
    let posed = HeadPose { valid: 1, ..HeadPose::default() };
    head.write_pair(1, [posed, posed]);

    let schedule = Schedule::new(1).unwrap();
    let mut publisher = AlternatePublisher::new(name, schedule);
    let n = (0..8).find(|&n| schedule.shown_eye(n) == Some(0)).unwrap();
    assert_eq!(publisher.publish_flat(&device, &context, &image, n, &head), Step::Waiting, "a left eye's present sends nothing");
    assert_eq!(publisher.publish_flat(&device, &context, &image, n + 1, &head), Step::Published);
    let record = viewer.read_pair(1).unwrap();
    assert!(record.eyes.iter().all(|e| e.valid == 0), "the viewer shows it flat");
}
