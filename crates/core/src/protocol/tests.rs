use super::*;
use core::mem::{align_of, offset_of, size_of};

// Sizes and offsets under MSVC x64, pinned so a layout change is always deliberate.
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
fn pose_block_layout() {
    assert_eq!(size_of::<PoseBlock>(), 4096);
    assert_eq!(offset_of!(PoseBlock, head), 8);
    assert_eq!(offset_of!(PoseBlock, pairs), 128);
}

#[test]
fn synthetic_pose_and_ipd() {
    let fov = [[-0.9, 0.8, 0.8, -0.9]; 2];
    let pose = HeadPose::synthetic(30.0, 0.0, fov, 0.064);
    assert!((crate::math::yaw(pose.orientation) - 30f32.to_radians()).abs() < 1e-5);
    let (yaw, pitch) = crate::camera::head_yaw_pitch(HeadPose::synthetic(30.0, 20.0, fov, 0.064).orientation);
    assert!((yaw - 30f32.to_radians()).abs() < 1e-5 && (pitch - 20f32.to_radians()).abs() < 1e-5);
    assert_eq!(pose.ipd_or(0.07), 0.064);
    assert_eq!(HeadPose { ipd: 0.2, ..pose }.ipd_or(0.07), 0.07);
    assert_eq!(HeadPose { valid: 0, ..pose }.ipd_or(0.07), 0.07);
}

#[test]
fn finger_block_layout() {
    assert_eq!(size_of::<FingerPose>(), 96);
    assert_eq!(offset_of!(FingerPose, fingers), 16);
    assert_eq!(size_of::<FingerBlock>(), 200);
    assert_eq!(offset_of!(FingerBlock, hands), 8);
    assert!(FingerBlock::fresh().is_compatible() && !FingerBlock::fresh().is_blank());
}

#[test]
fn hand_block_layout() {
    assert_eq!(size_of::<HandPose>(), 40);
    assert_eq!(offset_of!(HandPose, received_tick), 32);
    assert_eq!(size_of::<HandBlock>(), 192);
    assert_eq!(offset_of!(HandBlock, hands), 8);
    assert_eq!(offset_of!(HandBlock, grips), 88);
    assert_eq!(offset_of!(HandBlock, pad), 168);
    assert_eq!(size_of::<PadState>(), 24);
    assert!(HandBlock::fresh().is_compatible() && !HandBlock::fresh().is_blank());
}

#[test]
fn only_the_same_record_at_another_version_is_a_mismatch() {
    let stamp = |magic, version| Stamp { magic, version };
    assert_eq!(
        Mismatch::of(stamp(FENCE_MAGIC, 2), "stereo channel", FENCE_MAGIC, FENCE_VERSION),
        Some(Mismatch { what: "stereo channel", found: 2, expected: FENCE_VERSION })
    );
    assert_eq!(Mismatch::of(stamp(FENCE_MAGIC, FENCE_VERSION), "stereo channel", FENCE_MAGIC, FENCE_VERSION), None, "the same version");
    assert_eq!(Mismatch::of(stamp(crate::protocol::POSE_MAGIC, 9), "stereo channel", FENCE_MAGIC, FENCE_VERSION), None, "another kind of record");
    assert_eq!(Mismatch::of(stamp(0, 0), "stereo channel", FENCE_MAGIC, FENCE_VERSION), None, "no record yet");
    assert_eq!(size_of::<Stamp>(), 8);
}

/// Version 3's layout: version 2's 240 bytes, then the depth part (older channels are refused
/// by the version check).
#[test]
fn fence_header_layout() {
    assert_eq!(size_of::<FenceHeader>(), 616);
    assert_eq!(offset_of!(FenceHeader, adapter), 24);
    assert_eq!(offset_of!(FenceHeader, textures), 32);
    assert_eq!(offset_of!(FenceHeader, produced), 112);
    assert_eq!(offset_of!(FenceHeader, consumed), 120);
    assert_eq!(offset_of!(FenceHeader, sequence), 128);
    assert_eq!(offset_of!(FenceHeader, tick), 136);
    assert_eq!(offset_of!(FenceHeader, latest_slot), 144);
    assert_eq!(offset_of!(FenceHeader, reader_slot), 148);
    assert_eq!(offset_of!(FenceHeader, reader_claim), 152);
    assert_eq!(offset_of!(FenceHeader, slot_sequences), 160);
    assert_eq!(offset_of!(FenceHeader, slot_ticks), 200);
    assert_eq!(offset_of!(FenceHeader, depth), 240);
    assert_eq!(size_of::<FenceDepth>(), 376);
    assert_eq!(offset_of!(FenceDepth, textures), 16);
    assert_eq!(offset_of!(FenceDepth, slots), 96);
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
