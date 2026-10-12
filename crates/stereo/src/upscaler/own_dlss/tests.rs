use super::*;

/// A packet layout of the shape the tests write (Dying Light 2's).
pub(super) const LAYOUT: PacketLayout =
    PacketLayout { bytes: 0x1b0, reset: 0x18, size: [0x24, 0x28], projection: 0x30, position: 0x170, basis: [0x188, 0x17c, 0x194], to_previous: 0xf0, to_current: 0x130 };

/// A packet with an identity view at `position` and a simple projection.
pub(super) fn packet(position: [f32; 3]) -> Vec<u8> {
    let mut packet = vec![0u8; LAYOUT.bytes];
    for (i, v) in [1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 0.1, 0.0].iter().enumerate() {
        packet[0x30 + 4 * i..0x34 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    for (row, v) in [(0x188, [1.0f32, 0.0, 0.0]), (0x17c, [0.0, 1.0, 0.0]), (0x194, [0.0, 0.0, 1.0])] {
        for (j, x) in v.iter().enumerate() {
            packet[row + 4 * j..row + 4 * j + 4].copy_from_slice(&x.to_le_bytes());
        }
    }
    for (j, x) in position.iter().enumerate() {
        packet[0x170 + 4 * j..0x174 + 4 * j].copy_from_slice(&x.to_le_bytes());
    }
    packet
}

#[test]
fn each_eye_seeds_its_own_history_and_keeps_it() {
    let mut m = DlssMatrices::default();
    let mut p = packet([0.0; 3]);
    assert_eq!(m.apply(&LAYOUT, &mut p, 1, 1, 3, false), Some(true));
    assert_eq!(read_u32(&p, 0x18), 1, "first use seeds");
    p[0x18..0x1c].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(m.apply(&LAYOUT, &mut p, 1, 1, 3, false), Some(false));
    assert_eq!(read_u32(&p, 0x18), 0, "same epoch keeps history");
    assert!((read_f32(&p, 0xf0) - 1.0).abs() < 1e-5);
    assert_eq!(m.apply(&LAYOUT, &mut p, 2, 1, 3, false), Some(true), "the other eye seeds its own");
    assert_eq!(m.apply(&LAYOUT, &mut p, 1, 2, 3, false), Some(true), "a new epoch seeds again");
    assert_eq!(m.apply(&LAYOUT, &mut p, 1, 2, 4, false), Some(true), "and a new viewport");
    assert_eq!(m.apply(&LAYOUT, &mut p, 1, 2, 4, true), Some(true), "and a reset asked for");
    assert_eq!(m.apply(&LAYOUT, &mut p, 1, 2, 4, false), Some(false));
    assert_eq!(m.apply(&LAYOUT, &mut p, 3, 2, 4, false), None, "eyes are 1 and 2");
}

#[test]
fn writers_are_bits_by_their_place() {
    let writers = [0x10, 0x20, 0x30];
    assert_eq!(writers.iter().fold(0, |m, &r| m | writer_bit(&writers, r)), all_writers(3));
    assert_eq!(writer_bit(&writers, 0x40), 0);
    assert_eq!(all_writers(7), 127);
}

#[test]
fn a_new_camera_or_a_skipped_frame_starts_a_new_epoch() {
    let cameras: EyeCameras<16, 8> = EyeCameras::new();
    let first = cameras.epoch();
    cameras.begin_scene(10, 1, 1, 0x100);
    let after_left = cameras.epoch();
    assert!(after_left > first, "the first left eye starts over");
    cameras.end_scene(1, 1, all_writers(2), |_, _| 0);
    assert!(cameras.starting_over(), "its writers did not all write");
    cameras.begin_scene(11, 1, 2, 0x100);
    assert_eq!(cameras.epoch(), after_left, "the right eye of the same camera and frame");
    cameras.begin_scene(13, 1, 2, 0x100);
    assert!(cameras.epoch() > after_left, "a skipped frame");
}
