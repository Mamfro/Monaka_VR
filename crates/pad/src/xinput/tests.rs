use super::*;

#[test]
fn layouts_are_xinputs() {
    assert_eq!(size_of::<XInputState>(), 16);
    assert_eq!(size_of::<XInputCapabilities>(), 20);
    assert_eq!(std::mem::offset_of!(XInputCapabilities, vibration), 16);
}

#[test]
fn packets_pass_the_real_pads_own_and_the_floor() {
    assert_eq!(next_packet(0, 0, 0), 1);
    assert_eq!(next_packet(5, 0, 0), 6, "no real pad: counts on");
    assert_eq!(next_packet(1, 461, 0), 462, "a real pad already seen at 461: above it");
    assert_eq!(next_packet(462, 461, 0), 463);
    assert_eq!(next_packet(0, 0, 900_000), 900_000, "a pad switched off since: above the floor");
    assert_eq!(next_packet(900_000, 0, 900_000), 900_001);
    assert_eq!(next_packet(u32::MAX, 0, 0), 1, "wraps, never 0");
    assert!(packet_floor() < 1 << 31);
}
