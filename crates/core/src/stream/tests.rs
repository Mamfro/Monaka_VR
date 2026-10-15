use super::*;
use monaka_contract::debug_draw::{Category, RED};

fn frame(sequence: u64) -> Frame {
    let line = Primitive::segment(Category::Skeleton, [0.1, 1.2, -0.3], [0.2, 1.1, -0.4], RED);
    Frame {
        sequence,
        tick: 1000 + sequence,
        eyes: [Eye { orientation: [0.0, 0.0, 0.0, 1.0], position: [-0.03, 1.6, 0.0], fov: [-0.9, 0.8, 0.85, -0.95] }; 2],
        head: Pose { orientation: [0.0, 0.1, 0.0, 0.99], position: [0.0, 1.6, 0.0], tracked: true },
        controllers: [Pose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.2, 1.2, -0.3], tracked: true }, Pose::default(), Pose::default(), Pose::default()],
        primitives: [vec![line], vec![line, Primitive::point(Category::Probe, [1.0, 2.0, 3.0], RED)]],
    }
}

#[test]
fn frames_read_back_as_written() {
    let mut file = header();
    file.extend(frame(7).encode());
    file.extend(frame(8).encode());
    assert_eq!(read(&file).unwrap(), vec![frame(7), frame(8)]);
}

#[test]
fn a_frame_cut_short_is_left_out_and_strangers_are_refused() {
    let mut file = header();
    file.extend(frame(7).encode());
    let next = frame(8).encode();
    file.extend(&next[..next.len() - 3]);
    assert_eq!(read(&file).unwrap().len(), 1);
    assert!(read(b"not a stream at all").is_err());
}

#[test]
fn the_report_finds_the_shaking_bone() {
    let frames: Vec<Frame> = (0..20u64)
        .map(|i| {
            let wobble = if i % 2 == 0 { 0.004 } else { -0.004 };
            let still = Primitive::segment(Category::Skeleton, [0.0; 3], [0.0, 1.0, 0.0], RED);
            let shaking = Primitive::segment(Category::Skeleton, [0.0; 3], [0.1 + wobble, 1.0, 0.0], RED);
            Frame { sequence: i, tick: i * 11, primitives: [vec![still, shaking], Vec::new()], ..frame(i) }
        })
        .collect();
    let report = report(&frames);
    assert!(report.contains("20 frames"), "{report}");
    let worst = report.lines().find(|l| l.contains("bone ")).unwrap();
    assert!(worst.contains("bone 1") && worst.contains("16.00 mm"), "{report}");
}
