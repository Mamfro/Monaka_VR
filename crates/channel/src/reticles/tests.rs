use super::*;

#[test]
fn a_reticle_travels_and_goes_stale() {
    let name = ChannelName::new(format!(r"Local\monaka-test-reticles-{}", std::process::id())).unwrap();
    let mut producer = ReticleChannel::open(&name).unwrap();
    let mut viewer = ReticleChannel::open(&name).unwrap();
    assert_eq!(viewer.read(0, 1000), None);
    assert!(producer.write(0, Some([0.0, 0.3, -0.95])));
    assert_eq!(viewer.read(0, 1000), Some([0.0, 0.3, -0.95]));
    assert_eq!(viewer.read(1, 1000), None);
    assert!(producer.write(0, None));
    assert_eq!(viewer.read(0, 1000), None);
}
