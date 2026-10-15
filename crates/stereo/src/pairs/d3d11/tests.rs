use super::*;
use monaka_channel::d3d::{create_device, create_texture, simple_texture_desc};
use monaka_channel::pose::PoseChannel;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

#[test]
fn a_flat_frame_carries_an_empty_record() {
    let name = ChannelName::new(format!(r"Local\monaka-test-pairs11-flat-{}", std::process::id())).unwrap();
    let (device, context, _) = create_device(None).unwrap();
    let image = create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
    let mut viewer = PoseChannel::open(&name).unwrap();
    let head = HeadSource::new();
    head.open(Some(PoseChannel::open(&name).unwrap()), None);
    // An earlier run's record at the sequence this run's first pair gets.
    let posed = HeadPose { valid: 1, ..HeadPose::default() };
    head.write_pair(1, [posed, posed]);

    let mut publisher = PairPublisher11::new(name);
    assert_eq!(publisher.publish_flat(&device, &context, &image, &head, 5, |_, _| true), Step::Published);
    let record = viewer.read_pair(1).unwrap();
    assert!(record.eyes.iter().all(|e| e.valid == 0), "the viewer shows it flat");
}
