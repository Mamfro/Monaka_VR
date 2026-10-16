use super::*;
use core::mem::{align_of, offset_of, size_of};

// Sizes and offsets, pinned so a layout change is always deliberate: within a major a record
// only grows at its end ("Changing a record"), so these offsets never move, and only a size may
// rise, with its minor.
#[test]
fn eye_gaze_layout() {
    assert_eq!(size_of::<EyeGaze>(), 48);
    assert_eq!(align_of::<EyeGaze>(), 8);
    assert_eq!(offset_of!(EyeGaze, sample_time), 24);
    assert_eq!(offset_of!(EyeGaze, received_tick), 32);
    assert_eq!(offset_of!(EyeGaze, valid), 40);
}

#[test]
fn head_pose_layout() {
    assert_eq!(size_of::<HeadPose>(), 120);
    assert_eq!(offset_of!(HeadPose, ipd), 28);
    assert_eq!(offset_of!(HeadPose, fov), 32);
    assert_eq!(offset_of!(HeadPose, valid), 64);
    assert_eq!(offset_of!(HeadPose, gaze), 72);
    assert_eq!(size_of::<RenderedPair>(), 248);
    assert_eq!(offset_of!(RenderedPair, eyes), 8);
}

#[test]
fn stamp_layout() {
    assert_eq!(size_of::<Stamp>(), 16);
    assert_eq!(offset_of!(Stamp, major), 4);
    assert_eq!(offset_of!(Stamp, minor), 6);
    assert_eq!(offset_of!(Stamp, size), 8);
}

#[test]
fn pose_block_layout() {
    assert_eq!(size_of::<Seq<HeadPose>>(), 128);
    assert_eq!(offset_of!(Seq<HeadPose>, value), 8);
    assert_eq!(size_of::<PoseBlock>(), 4264);
    assert_eq!(offset_of!(PoseBlock, head), 16);
    assert_eq!(offset_of!(PoseBlock, pairs), 144);
    // Appended in minor 1, where the record of minor 0 ended.
    assert_eq!(offset_of!(PoseBlock, display), 4240);
}

#[test]
fn finger_block_layout() {
    assert_eq!(size_of::<FingerPose>(), 96);
    assert_eq!(offset_of!(FingerPose, fingers), 16);
    assert_eq!(size_of::<FingerBlock>(), 216);
    assert_eq!(offset_of!(FingerBlock, hands), 16);
    assert!(FingerBlock::fresh().is_compatible() && !FingerBlock::fresh().is_blank());
}

#[test]
fn reticle_block_layout() {
    assert_eq!(size_of::<ReticleAim>(), 32);
    assert_eq!(offset_of!(ReticleAim, written_tick), 24);
    assert_eq!(size_of::<ReticleBlock>(), 96);
    assert_eq!(offset_of!(ReticleBlock, aims), 16);
}

#[test]
fn hand_block_layout() {
    assert_eq!(size_of::<HandPose>(), 40);
    assert_eq!(offset_of!(HandPose, received_tick), 32);
    assert_eq!(size_of::<HandBlock>(), 216);
    assert_eq!(offset_of!(HandBlock, poses), 16);
    assert_eq!(offset_of!(HandPoses, grips), 80);
    assert_eq!(offset_of!(HandBlock, pad), 184);
    assert_eq!(size_of::<PadState>(), 24);
    assert!(HandBlock::fresh().is_compatible() && !HandBlock::fresh().is_blank());
}

#[test]
fn only_the_same_record_at_another_major_is_a_mismatch() {
    let stamp = |magic, major, minor| Stamp { magic, major, minor, size: 0, _reserved: 0 };
    assert_eq!(
        Mismatch::of(stamp(FENCE_MAGIC, 2, 0), "stereo channel", FENCE_MAGIC, FENCE_MAJOR),
        Some(Mismatch { what: "stereo channel", found: 2, expected: FENCE_MAJOR })
    );
    assert_eq!(Mismatch::of(stamp(FENCE_MAGIC, FENCE_MAJOR, 0), "stereo channel", FENCE_MAGIC, FENCE_MAJOR), None, "the same major");
    assert_eq!(Mismatch::of(stamp(FENCE_MAGIC, FENCE_MAJOR, 7), "stereo channel", FENCE_MAGIC, FENCE_MAJOR), None, "a newer minor is read");
    assert_eq!(Mismatch::of(stamp(POSE_MAGIC, 9, 0), "stereo channel", FENCE_MAGIC, FENCE_MAJOR), None, "another kind of record");
    assert_eq!(Mismatch::of(stamp(0, 0, 0), "stereo channel", FENCE_MAGIC, FENCE_MAJOR), None, "no record yet");
}

#[test]
fn a_record_of_any_minor_is_read() {
    let mut newer = HandBlock::fresh();
    newer.stamp.minor += 3;
    newer.stamp.size += 64;
    assert!(newer.is_compatible());
    let mut other_major = HandBlock::fresh();
    other_major.stamp.major += 1;
    assert!(!other_major.is_compatible());
}

/// Before contract 4 a stamp was `magic` and a 32-bit `version` at the same offset as `major` and
/// `minor`: a side from then reads a record now as `major | minor << 16`, and this side reads
/// that side's version as a major with minor 0. Each major is above the version its record had
/// then, so neither side takes the other's record for its own.
#[test]
fn majors_continue_the_old_versions() {
    let before_4 = [
        (FENCE_MAJOR, 3),
        (POSE_MAJOR, 2),
        (HANDS_MAJOR, 3),
        (FINGERS_MAJOR, 1),
        (RETICLES_MAJOR, 1),
        (crate::debug_draw::DEBUG_MAJOR, 8),
        (crate::page::PAGE_MAJOR, 1),
        (crate::console::CONSOLE_MAJOR, 2),
        (crate::panels::PANELS_MAJOR, 1),
    ];
    for (major, version) in before_4 {
        assert!(major as u32 > version, "major {major} must be above the old version {version}");
    }
}

/// The stereo header only hands pairs over; what comes with a pair is a block beside it.
#[test]
fn fence_header_layout() {
    assert_eq!(size_of::<FenceHeader>(), 248);
    assert_eq!(offset_of!(FenceHeader, width), 16);
    assert_eq!(offset_of!(FenceHeader, adapter), 32);
    assert_eq!(offset_of!(FenceHeader, textures), 40);
    assert_eq!(offset_of!(FenceHeader, produced), 120);
    assert_eq!(offset_of!(FenceHeader, consumed), 128);
    assert_eq!(offset_of!(FenceHeader, sequence), 136);
    assert_eq!(offset_of!(FenceHeader, tick), 144);
    assert_eq!(offset_of!(FenceHeader, latest_slot), 152);
    assert_eq!(offset_of!(FenceHeader, reader_slot), 156);
    assert_eq!(offset_of!(FenceHeader, reader_claim), 160);
    assert_eq!(offset_of!(FenceHeader, slot_sequences), 168);
    assert_eq!(offset_of!(FenceHeader, slot_ticks), 208);
}

#[test]
fn depth_block_layout() {
    assert_eq!(size_of::<DepthBlock>(), 392);
    assert_eq!(offset_of!(DepthBlock, generation), 16);
    assert_eq!(offset_of!(DepthBlock, textures), 32);
    assert_eq!(offset_of!(DepthBlock, slots), 112);
    assert!(DepthBlock::fresh().is_compatible() && !DepthBlock::fresh().is_blank());
    assert_eq!(size_of::<DepthSlot>(), 56);
    assert_eq!(offset_of!(DepthSlot, params), 8);
    assert_eq!(offset_of!(DepthSlot, rects), 24);
    assert_eq!(size_of::<DepthParams>(), 16);
    assert_eq!(offset_of!(DepthParams, reversed), 8);
}

#[test]
fn depth_params_give_the_distances_at_0_and_1() {
    let standard = DepthParams::new(0.1, 1000.0, false);
    assert!(standard.is_valid());
    assert_eq!(standard.distances_at_0_and_1(), (0.1, 1000.0));
    // Reversed depth is 1 at the near plane: the far plane is at 0, even when infinite.
    let reversed = DepthParams::new(0.05, f32::INFINITY, true);
    assert!(reversed.is_valid());
    assert_eq!(reversed.distances_at_0_and_1(), (f32::INFINITY, 0.05));
    assert!(!DepthParams::new(0.0, 100.0, false).is_valid(), "no near plane");
    assert!(!DepthParams::new(10.0, 1.0, false).is_valid(), "far before near");
    assert!(!DepthParams::new(f32::NAN, 100.0, false).is_valid());
    assert!(!DepthParams::default().is_valid(), "zeroed shared memory is no depth");
}

#[test]
fn depth_slot_formats() {
    use dxgi_format::*;
    for format in [D32_FLOAT, R32_TYPELESS, R32_FLOAT, D32_FLOAT_S8X24_UINT, R32G8X24_TYPELESS, R32_FLOAT_X8X24_TYPELESS] {
        assert_eq!(depth_slot_format(format), Some(R32_TYPELESS), "{format}");
    }
    for format in [D16_UNORM, R16_TYPELESS, R16_UNORM] {
        assert_eq!(depth_slot_format(format), Some(R16_TYPELESS), "{format}");
    }
    assert_eq!(depth_slot_format(D24_UNORM_S8_UINT), None);
    assert_eq!(depth_slot_format(R8G8B8A8_UNORM), None);
}

#[test]
fn channel_names() {
    let name = ChannelName::new(r"Local\monaka-1").unwrap();
    assert_eq!(name.header_lock(), r"Local\monaka-1-lock");
    assert_eq!(name.reader_lease(), r"Local\monaka-1-reader");
    assert_eq!(name.block(Block::Pose), (r"Local\monaka-1-pose".into(), r"Local\monaka-1-pose-lock".into()));
    assert_eq!(name.block(Block::ViewConsole), (r"Local\monaka-1-view-console".into(), r"Local\monaka-1-view-console-lock".into()));
    assert_eq!(name.block(Block::Depth), (r"Local\monaka-1-depth".into(), r"Local\monaka-1-depth-lock".into()));
    assert!(ChannelName::new("").is_none());
    assert!(ChannelName::new("x".repeat(201)).is_none());
}

#[test]
fn pose_ring_wraps_by_sequence() {
    assert_eq!(PoseBlock::pair_slot(17), 1);
    assert_eq!(PoseBlock::pair_slot(16), 0);
    assert!(PoseBlock::fresh().is_compatible());
}

#[test]
fn fence_formats() {
    assert!(fence_format_ok(dxgi_format::R10G10B10A2_UNORM));
    assert!(!fence_format_ok(dxgi_format::R8G8B8A8_UNORM_SRGB));
}

#[test]
fn an_image_travels_in_its_family_s_fence_format() {
    use dxgi_format::*;
    assert_eq!(fence_format_of(R8G8B8A8_TYPELESS), Some(R8G8B8A8_UNORM));
    assert_eq!(fence_format_of(R8G8B8A8_UNORM_SRGB), Some(R8G8B8A8_UNORM));
    assert_eq!(fence_format_of(B8G8R8A8_UNORM_SRGB), Some(B8G8R8A8_UNORM));
    assert_eq!(fence_format_of(R10G10B10A2_TYPELESS), Some(R10G10B10A2_UNORM));
    assert_eq!(fence_format_of(R32_FLOAT), None);
    for format in 0..200 {
        assert!(fence_format_of(format).is_none_or(fence_format_ok), "format {format}");
    }
}
