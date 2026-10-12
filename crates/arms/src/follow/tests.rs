use super::arms::*;
use super::checks::*;
use super::*;
use monaka_core::camera::{distance, is_rigid};

#[test]
fn the_grip_turns_the_aim_to_the_blade() {
    assert!(is_rigid(&MELEE_GRIP, 1e-5));
    let along = apply(&MELEE_GRIP, [0.0, 0.0, -1.0]);
    assert!(along.iter().zip(MELEE_BLADE).all(|(a, b)| (a - b).abs() < 1e-6), "{along:?}");
}

#[test]
fn a_sway_on_the_written_bones_is_told_from_a_fresh_pose() {
    let written = vec![[1.0, 0.0, 0.0, 340.0, 0.0, 1.0, 0.0, 60.0, 0.0, 0.0, 1.0, 210.0], [f32::NAN; 12]];
    // Not written: whatever the game has there does not count.
    let mut handed = vec![written[0], [5.0; 12]];
    assert_eq!(written_apart(&written, &handed), (0.0, 0.0));
    handed[0][3] += 0.004;
    handed[0][1] += 0.01;
    let (turn, shift) = written_apart(&written, &handed);
    assert!(turn <= SWAY_TURN && shift <= SWAY_SHIFT, "a sway: {turn} {shift}");
    handed[0][11] -= 0.15;
    assert!(written_apart(&written, &handed).1 > SWAY_SHIFT, "the game's own pose, 15 cm off");
    handed[0][0] = f32::NAN;
    assert_eq!(written_apart(&written, &handed).0, f32::INFINITY, "not finite is never ours");
}

#[test]
fn the_wrist_calibration_is_rigid_and_mirrors() {
    let identity = BoneAxes::DYING_LIGHT_1.wrist;
    let right = wrist_in_palm(RIGHT_HAND, None, &identity).unwrap();
    assert!(is_rigid(&right, 1e-4));
    assert!(distance(&right, &RIGHT_WRIST_IN_PALM) < 0.01, "only rounding is corrected");
    // The right wrist's +X runs forward along the fingers (-Z of the palm).
    assert!(right[8] < -0.9, "{right:?}");
    assert!(wrist_in_palm(LEFT_HAND, None, &identity).is_none(), "no left wrist without the skeleton's mirroring");
    // A skeleton whose left bones flip their z axis.
    let flip = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
    let left = wrist_in_palm(LEFT_HAND, Some(&flip), &identity).unwrap();
    assert!(is_rigid(&left, 1e-4), "{left:?}");
    assert_eq!((left[3], left[7], left[11]), (-right[3], right[7], right[11]));
    // Mirrored: the left wrist's +X still runs forward, its z axis the right one's mirrored and flipped.
    assert!((left[8] - right[8]).abs() < 1e-6 && (left[0] + right[0]).abs() < 1e-6);
    assert!((left[2] - right[2]).abs() < 1e-6, "x of z: negated twice");
}

#[test]
fn a_turned_skeleton_turns_the_wrist_in_its_own_frame() {
    // Dying Light 2's wrist: DL1's half turned about its own x (along the forearm).
    let turn = [1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0];
    let dl1 = wrist_in_palm(RIGHT_HAND, None, &BoneAxes::DYING_LIGHT_1.wrist).unwrap();
    let dl2 = wrist_in_palm(RIGHT_HAND, None, &turn).unwrap();
    assert!(is_rigid(&dl2, 1e-4));
    for row in 0..3 {
        assert_eq!(dl2[row * 4], dl1[row * 4], "x kept");
        assert_eq!((dl2[row * 4 + 1], dl2[row * 4 + 2]), (-dl1[row * 4 + 1], -dl1[row * 4 + 2]), "y and z flipped");
        assert_eq!(dl2[row * 4 + 3], dl1[row * 4 + 3], "the wrist stays where it was");
    }
}

#[test]
fn mirror_flip_reads_the_skeleton() {
    // A right bone turned 30 degrees about y; its left twin mirrored across x with z flipped.
    let (c, s) = (0.866_025_4f32, 0.5f32);
    let right = [c, 0.0, s, 0.0, 1.0, 0.0, -s, 0.0, c];
    let flip = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
    let mut left = [0.0f32; 9];
    for row in 0..3 {
        for col in 0..3 {
            let v: f32 = (0..3).map(|k| right[row * 3 + k] * flip[k * 3 + col]).sum();
            left[row * 3 + col] = if row == 0 { -v } else { v };
        }
    }
    assert_eq!(mirror_flip(&right, &left), Some(flip));
}

#[test]
fn a_blend_far_from_the_origin_keeps_the_bones_near() {
    // Half way from staying put to a quarter turn about up, 460 m out: a bone a metre from the
    // pivot lands a metre from it, not metres away.
    let pivot = [460.0, 50.0, 190.0];
    let turn = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0];
    let about_pivot = compose(&compose(&translation(pivot), &turn), &translation([-pivot[0], -pivot[1], -pivot[2]]));
    let half = blend(&translation([0.0; 3]), &about_pivot, 0.5, pivot);
    let bone = apply(&half, [461.0, 50.0, 190.0]);
    assert!((monaka_core::math::distance(bone, pivot) - 1.0).abs() < 1e-3, "{bone:?}");
    assert!(monaka_core::math::distance(apply(&half, pivot), pivot) < 1e-3);
}

#[test]
fn only_plausible_poses_and_placements_pass() {
    let level = [0.0, 0.0, 0.0, 1.0];
    assert!(plausible_pose(level, [0.3, 1.2, -0.4]));
    assert!(!plausible_pose([0.0; 4], [0.3, 1.2, -0.4]), "no rotation");
    assert!(!plausible_pose(level, [f32::NAN, 0.0, 0.0]));
    assert!(!plausible_pose(level, [0.0, 1e20, 0.0]), "far out");
    let camera = [1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0, -5.0];
    let bone = [0.0, -1.0, 0.0, 10.3, 1.0, 0.0, 0.0, 1.5, 0.0, 0.0, 1.0, -5.4];
    assert!(plausible_placement(&bone, &bone, &camera));
    let far = [0.0, -1.0, 0.0, 10.3, 1.0, 0.0, 0.0, 1.5, 0.0, 0.0, 1.0, 400.0];
    assert!(!plausible_placement(&far, &bone, &camera));
    let flat = [0.0, -1.0, 0.0, 10.3, 0.0, 0.0, 0.0, 1.5, 0.0, 0.0, 1.0, -5.4];
    assert!(!plausible_placement(&flat, &bone, &camera), "a column squashed to nothing");
}
