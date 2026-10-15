use super::*;
use core::f32::consts::FRAC_PI_6;
use crate::math::{IDENTITY, yaw_rotation};

#[test]
fn row_vector_rows_become_columns_and_the_nearest_axis_is_signed() {
    // A row-vector transform translating by (1, 2, 3).
    let b = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 1.0];
    assert_eq!(from_rows_4x4(&b), [1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0, 3.0]);
    // A frame turned a quarter about y: its z axis points along world x.
    let frame = [0.0, 0.0, 1.0, 5.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0];
    assert_eq!(nearest_axis(&frame, [0.9, 0.1, 0.0]), Some([0.0, 0.0, 1.0]));
    assert_eq!(nearest_axis(&frame, [0.0, 0.0, 2.0]), Some([-1.0, 0.0, 0.0]));
}

#[test]
fn an_eye_view_is_the_eye_beside_the_view_centre_with_its_frustum() {
    let shared = Shared::from_options(&crate::options::Options::parse("eye_separation=0.064"), &monaka_contract::manifest::MONAKA);
    let game = [1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0, -5.0];
    // No head pose: the game's camera is the view centre, its own field of view the frustum.
    let [left, right] = [0, 1].map(|eye| EyeView::of(&game, Base::Game, &HeadPose::default(), eye, &shared, true, [1.5, 1.0]).unwrap());
    assert_eq!(left.centre, game);
    assert!((left.camera[3] - 9.968).abs() < 1e-5 && (right.camera[3] - 10.032).abs() < 1e-5, "{left:?} {right:?}");
    assert!((full_angles_of(&left.frustum)[0] - 1.5).abs() < 1e-5 && (full_angles_of(&left.frustum)[1] - 1.0).abs() < 1e-5);
    // Not a camera, or not a field of view: nothing to draw.
    let mut skewed = game;
    skewed[1] = 0.5;
    assert!(EyeView::of(&skewed, Base::Game, &HeadPose::default(), 0, &shared, true, [1.5, 1.0]).is_none());
    assert!(EyeView::of(&game, Base::Game, &HeadPose::default(), 0, &shared, true, [0.0, 1.0]).is_none());
    assert!(!plausible_fov([3.1, 1.0]) && plausible_fov([0.1, 3.0]));
    assert_eq!(Base::game_or_swap(None), Base::Game);
    assert_eq!(Base::game_or_swap(Some([0.5, 0.25])), Base::Swap { baked_yaw: 0.5, baked_pitch: 0.25 });
}

fn full_angles_of(f: &Frustum) -> [f32; 2] {
    [f.right.atan() - f.left.atan(), f.up.atan() - f.down.atan()]
}

#[test]
fn a_scaled_frame_inverts() {
    let m: Mat34 = [0.0, -1.0, 0.0, 5.0, 2.0, 0.0, 0.0, 6.0, 0.0, 0.0, -1.0, 7.0];
    let back = compose(&inverse(&m).unwrap(), &m);
    assert!(distance(&back, &IDENTITY_MATRIX) < 1e-6, "{back:?}");
}

#[test]
fn a_scaled_frame_takes_a_vector_back_and_squares_up() {
    // Turned 90 degrees about z, x scaled by 2, z mirrored.
    let m: Mat34 = [0.0, -1.0, 0.0, 5.0, 2.0, 0.0, 0.0, 6.0, 0.0, 0.0, -1.0, 7.0];
    let v = [0.3, -0.2, 0.9];
    let back = unapply_3x3(&m, apply_3x3(&m, v)).unwrap();
    assert!((0..3).all(|k| (back[k] - v[k]).abs() < 1e-6), "{back:?}");
    assert!(unapply_3x3(&[0.0; 12], v).is_none(), "singular");
    let [x, y, z] = rigid_axes(&m);
    assert_eq!((x, y), ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]));
    assert!((dot(z, z) - 1.0).abs() < 1e-6 && dot(x, z).abs() < 1e-6 && dot(y, z).abs() < 1e-6);
}

#[test]
fn head_aim_adds_the_heads_turn_and_keeps_the_games_pitch() {
    // A game camera looking 10 degrees down, at (1, 2, 3).
    let down = 10f32.to_radians();
    let (s, c) = (down.sin(), down.cos());
    // Columns right, up, back: forward (0, -s, -c), so back (0, s, c) and up (0, c, -s).
    let camera: Mat34 = [1.0, 0.0, 0.0, 1.0, 0.0, c, s, 2.0, 0.0, -s, c, 3.0];
    // The head turned 30 left and 20 up.
    let (y, p) = (30f32.to_radians(), 20f32.to_radians());
    let (sy, cy) = (y * 0.5).sin_cos();
    let (sp, cp) = (p * 0.5).sin_cos();
    let q = [cy * sp, sy * cp, -sy * sp, cy * cp];
    let aimed = add_head_aim(&camera, q).unwrap();
    let forward = [-aimed[2], -aimed[6], -aimed[10]];
    let (yaw, pitch) = direction_yaw_pitch(forward).unwrap();
    assert!((yaw - y).abs() < 1e-4, "yaw {}", yaw.to_degrees());
    assert!((pitch - 10f32.to_radians()).abs() < 1e-4, "pitch {} = -10 + 20", pitch.to_degrees());
    assert_eq!([aimed[3], aimed[7], aimed[11]], [1.0, 2.0, 3.0]);
    assert!(is_rigid(&aimed, 1e-4));
    // No head turn: the game's camera.
    let same = add_head_aim(&camera, [0.0, 0.0, 0.0, 1.0]).unwrap();
    assert!(distance(&same, &camera) < 1e-5);
    // The game already turned by the head's whole share: the view is the game's camera.
    let baked = add_head_aim(&camera, q).unwrap();
    let swapped = swap_head_aim(&baked, y, p, q).unwrap();
    assert!(distance(&swapped, &baked) < 1e-4, "{swapped:?} vs {baked:?}");
    // Half of it baked: the other half is added.
    let half = swap_head_aim(&add_head_aim(&camera, half_turn(y * 0.5, p * 0.5)).unwrap(), y * 0.5, p * 0.5, q).unwrap();
    assert!(distance(&half, &baked) < 1e-4);
}

/// A head turned `yaw` left, then `pitch` up (radians).
fn half_turn(yaw: f32, pitch: f32) -> [f32; 4] {
    let (sy, cy) = (yaw * 0.5).sin_cos();
    let (sp, cp) = (pitch * 0.5).sin_cos();
    [cy * sp, sy * cp, -sy * sp, cy * cp]
}

#[test]
fn compose_moves_a_frame_into_another() {
    let camera = apply_head(&translation([1.0, 2.0, 3.0]), yaw_rotation(FRAC_PI_6), [0.0; 3]);
    let local = translation([0.0, 0.0, -2.0]);
    // Two metres ahead of a camera turned 30 degrees left.
    let world = compose(&camera, &local);
    let expected = [1.0 - 2.0 * FRAC_PI_6.sin(), 2.0, 3.0 - 2.0 * FRAC_PI_6.cos()];
    assert!((0..3).all(|i| (world[i * 4 + 3] - expected[i]).abs() < 1e-5), "{world:?}");
    // Moving that frame back out of the camera gives the local one again.
    assert!(distance(&compose(&rigid_inverse(&camera), &world), &local) < 1e-5);
}

#[test]
fn pointing_ignores_roll_and_matches_the_head() {
    // Yaw 30 degrees left, then pitch 20 degrees up (about the turned right axis), then roll.
    let (yaw, pitch, roll) = (FRAC_PI_6, 20f32.to_radians(), 50f32.to_radians());
    let about = |axis: [f32; 3], angle: f32| {
        let s = (angle * 0.5).sin();
        [axis[0] * s, axis[1] * s, axis[2] * s, (angle * 0.5).cos()]
    };
    let mul = |a: Quat, b: Quat| {
        let [ax, ay, az, aw] = a;
        let [bx, by, bz, bw] = b;
        [aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx, aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz]
    };
    let level = mul(about([0.0, 1.0, 0.0], yaw), about([1.0, 0.0, 0.0], pitch));
    let rolled = mul(level, about([0.0, 0.0, -1.0], roll));
    for q in [level, rolled] {
        let (y, p) = pointing_yaw_pitch(q);
        assert!((y - yaw).abs() < 1e-5 && (p - pitch).abs() < 1e-5, "{y} {p}");
    }
    let (hy, hp) = head_yaw_pitch(level);
    assert!((hy - yaw).abs() < 1e-5 && (hp - pitch).abs() < 1e-5);
    assert_eq!(pointing_yaw_pitch(IDENTITY), (0.0, 0.0));
    let forward = crate::math::forward_axis(level);
    let (dy, dp) = direction_yaw_pitch(forward.map(|c| c * 3.0)).unwrap();
    assert!((dy - yaw).abs() < 1e-5 && (dp - pitch).abs() < 1e-5);
    assert!(direction_yaw_pitch([0.0; 3]).is_none());
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

const CAMERA: Mat34 = [1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0, -3.0];

#[test]
fn head_composes_in_camera_space() {
    // Head 10 cm up, turned 90 degrees left: the camera's right axis becomes world -Z.
    let turned = apply_head(&CAMERA, yaw_rotation(core::f32::consts::FRAC_PI_2), [0.0, 0.1, 0.0]);
    assert!(close(turned[0], 0.0) && close(turned[8], -1.0) && close(turned[2], 1.0));
    assert!(close(turned[3], 10.0) && close(turned[7], 2.1) && close(turned[11], -3.0));
    assert_eq!(apply_head(&CAMERA, IDENTITY, [0.0; 3]), CAMERA);
}

#[test]
fn rigid_inverse_inverts() {
    let turned = apply_head(&CAMERA, yaw_rotation(0.7), [0.2, 0.1, -0.3]);
    assert!(mutually_inverse(&rigid_inverse(&turned), &turned, 1e-5));
    assert!(!mutually_inverse(&rigid_inverse(&CAMERA), &turned, 1e-3));
}

#[test]
fn eyes_straddle_the_centre() {
    let eyes = parallel_eyes(&rigid_inverse(&CAMERA), &CAMERA, 0.064).unwrap();
    assert!(close(eyes[0][3], 9.968) && close(eyes[1][3], 10.032) && close(eyes[1][11], -3.0));
    assert!(parallel_eyes(&CAMERA, &CAMERA, 0.064).is_none(), "a view that is not the inverse is refused");
    assert!(parallel_eyes(&rigid_inverse(&CAMERA), &CAMERA, 0.0).is_none());
    assert!(close(distance(&eyes[0], &eyes[1]), 0.064));
}

#[test]
fn frustum_projection_and_extents() {
    let fov = [-0.8, 0.7, 0.75, -0.75];
    let frustum = Frustum::from_fov(fov).unwrap();
    let (l, r) = (0.8f32.tan(), 0.7f32.tan());
    let mut projection = [0.0; 16];
    projection[10] = -1.0001;
    frustum.apply_to_projection(&mut projection);
    assert!(close(projection[0], 2.0 / (r + l)) && close(projection[2], (r - l) / (r + l)));
    assert!(close(projection[6], 0.0) && close(projection[10], -1.0001));
    let extents = frustum.extents(0.1);
    assert!(close(extents[0], -0.1 * l) && close(extents[3], 0.1 * 0.75f32.tan()));
    let back = Frustum::from_extents(extents, 0.1).unwrap().fov();
    assert!(back.iter().zip(fov).all(|(a, b)| close(*a, b)));
    assert!(Frustum::from_fov([0.0; 4]).is_none());
}

#[test]
fn head_yaw_and_pitch() {
    let (yaw, pitch) = head_yaw_pitch(yaw_rotation(core::f32::consts::FRAC_PI_2));
    assert!(close(yaw, core::f32::consts::FRAC_PI_2) && close(pitch, 0.0));
    // Yaw 30 left, then pitch 80 down about the turned x axis: the yaw survives the steep pitch.
    let (hy, hp) = (FRAC_PI_6 * 0.5, -1.3963f32 * 0.5);
    let q = [hy.cos() * hp.sin(), hy.sin() * hp.cos(), -hy.sin() * hp.sin(), hy.cos() * hp.cos()];
    let (yaw, pitch) = head_yaw_pitch(q);
    assert!(close(yaw, FRAC_PI_6) && close(pitch, -1.3963));
}

#[test]
fn levelling_turning_and_aim() {
    let (c, s) = (FRAC_PI_6.cos(), FRAC_PI_6.sin());
    let pitched: Mat34 = [1.0, 0.0, 0.0, 4.0, 0.0, c, s, 5.0, 0.0, -s, c, 6.0]; // 30 degrees down
    let flat = level(&pitched).unwrap();
    assert!(close(flat[0], 1.0) && close(flat[5], 1.0) && close(flat[10], 1.0) && close(flat[6], 0.0) && close(flat[3], 4.0));
    let ndc = aim_in_view(&pitched, &flat, [1.2, 1.5]).unwrap();
    assert!(close(ndc[0], 0.0) && (ndc[1] + FRAC_PI_6.tan() * 1.5).abs() < 1e-3, "aim 30 degrees below centre");
    let down: Mat34 = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0];
    assert!(level(&down).is_some_and(|m| close(m[0], 1.0) && close(m[10], 1.0)), "straight down keeps its yaw");
    assert!(aim_in_view(&down, &level(&down).unwrap(), [1.0, 1.0]).is_none());
    let turned = turn(&CAMERA, core::f32::consts::FRAC_PI_2);
    assert!(close(turned[0], 0.0) && close(turned[8], -1.0) && close(turned[2], 1.0) && close(turned[3], 10.0));
}

#[test]
fn symmetric_frustum_and_render_size() {
    let eye = Frustum::from_fov([-1.03, 0.87, 0.87, -1.04]).unwrap();
    let centred = eye.symmetric();
    assert!(close(centred.right, 1.03f32.tan()) && close(centred.left, -1.03f32.tan()) && close(centred.up, 1.04f32.tan()));
    assert_eq!(centred.straight_ahead(), [0.0, 0.0]);
    let (w, h) = Frustum::from_fov([-0.785, 0.785, 0.785, -0.785]).unwrap().render_size(2000);
    assert_eq!((w, h), (2000, 2000));
    assert_eq!(eye.render_size(1000).0 % 8, 0);
}

#[test]
fn frustum_union_and_straight_ahead() {
    let left = Frustum::from_fov([-1.03, 0.87, 0.87, -1.04]).unwrap();
    let right = Frustum::from_fov([-0.87, 1.03, 0.87, -1.04]).unwrap();
    let both = left.union(right);
    assert!(close(both.left, (-1.03f32).tan()) && close(both.right, 1.03f32.tan()));
    // The left eye sees further left, so straight ahead sits right of its image centre.
    assert!(left.straight_ahead()[0] > 0.0 && right.straight_ahead()[0] < 0.0);
    assert!(close(Frustum::from_fov([-0.9, 0.9, 0.9, -0.9]).unwrap().straight_ahead()[0], 0.0));
}
#[test]
fn a_camera_from_its_back_and_up_is_rigid_and_keeps_them() {
    let m = from_back_up([0.0, 0.5, 1.0], [0.0, 1.0, 0.0], [1.0, 2.0, 3.0]).unwrap();
    assert!(is_rigid(&m, 1e-5));
    assert_eq!((m[3], m[7], m[11]), (1.0, 2.0, 3.0));
    let back = [m[2], m[6], m[10]];
    assert!((back[1] - 0.5 / 1.25f32.sqrt()).abs() < 1e-5 && back[0].abs() < 1e-6);
    assert!(m[1].abs() < 1e-6, "right stays level: {m:?}");
    assert!(from_back_up([0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0; 3]).is_none(), "up along back");
}

#[test]
fn a_frames_quaternion_gives_its_rotation_back() {
    for q in [[0.0, 0.0, 0.0, 1.0], half_turn(0.7, -0.3), half_turn(3.0, 1.2), [1.0, 0.0, 0.0, 0.0], [0.0, 0.70710677, 0.0, -0.70710677]] {
        let r = rotation(q);
        let m = [r[0], r[1], r[2], 1.0, r[3], r[4], r[5], 2.0, r[6], r[7], r[8], 3.0];
        let back = rotation(quaternion(&m));
        assert!(r.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-5), "{q:?}");
        assert!(quaternion(&m)[3] >= 0.0);
    }
    assert_eq!(position(&translation([1.0, 2.0, 3.0])), [1.0, 2.0, 3.0]);
}

#[test]
fn view_centres_take_the_game_camera_three_ways() {
    let yaw = 30f32.to_radians();
    let game = from_back_up([yaw.sin() * 0.9, 0.43, yaw.cos() * 0.9], [0.0, 1.0, 0.0], [5.0, 1.0, 2.0]).unwrap();
    let head = half_turn(0.2, -0.1);
    let position = [0.03, -0.02, 0.05];
    // The game's camera: with the head still and ahead, the game camera itself.
    assert!(distance(&view_centre(&game, Base::Game, IDENTITY, [0.0; 3]).unwrap(), &game) < 1e-6);
    // Levelled: the head still and ahead looks level, along the game's yaw (30 degrees) with the
    // baked 0.1 turned out, from where the game camera stands.
    let still = view_centre(&game, Base::Level { baked_yaw: 0.1 }, IDENTITY, [0.0; 3]).unwrap();
    assert!(still[6].abs() < 1e-5, "level: {still:?}");
    assert!((still[2].atan2(still[10]) - (yaw - 0.1)).abs() < 1e-4, "{still:?}");
    assert_eq!((still[3], still[7], still[11]), (5.0, 1.0, 2.0));
    let levelled = view_centre(&game, Base::Level { baked_yaw: 0.1 }, head, position).unwrap();
    // Swapped: the head's position moves the view without turning it.
    let swap = Base::Swap { baked_yaw: 0.1, baked_pitch: 0.05 };
    let (swapped, unmoved) = (view_centre(&game, swap, head, position).unwrap(), view_centre(&game, swap, head, [0.0; 3]).unwrap());
    for row in 0..3 {
        assert_eq!(swapped[row * 4..row * 4 + 3], unmoved[row * 4..row * 4 + 3]);
    }
    // A lean to the right with the head turned a quarter left still goes to the body's right.
    let level_game = level(&game).unwrap();
    let quarter_left = [0.0, std::f32::consts::FRAC_PI_4.sin(), 0.0, std::f32::consts::FRAC_PI_4.cos()];
    let leaned = swap_head(&level_game, 0.0, 0.0, quarter_left, [0.1, 0.0, 0.0]).unwrap();
    let still = swap_head(&level_game, 0.0, 0.0, quarter_left, [0.0; 3]).unwrap();
    let shift = [0, 1, 2].map(|k| leaned[k * 4 + 3] - still[k * 4 + 3]);
    let body_right = [level_game[0], level_game[4], level_game[8]];
    assert!((0..3).all(|k| close(shift[k], 0.1 * body_right[k])), "{shift:?} against {body_right:?}");
    // The eyes straddle the centre along its right axis.
    let [left, right] = [0, 1].map(|eye| eye_camera(&levelled, eye, 0.064));
    let apart = [right[3] - left[3], right[7] - left[7], right[11] - left[11]];
    assert!(close(apart.iter().map(|v| v * v).sum::<f32>().sqrt(), 0.064) && left[0] == levelled[0]);
    assert!(close(apart[0] / 0.064, levelled[0]) && close(apart[2] / 0.064, levelled[8]), "along the right axis");
}

#[test]
fn a_bases_origin_is_where_the_heads_position_moves_the_view() {
    let yaw = 30f32.to_radians();
    let game = from_back_up([yaw.sin() * 0.9, 0.43, yaw.cos() * 0.9], [0.0, 1.0, 0.0], [5.0, 1.0, 2.0]).unwrap();
    let position = [0.03, -0.02, 0.05];
    for base in [Base::Game, Base::Level { baked_yaw: 0.1 }, Base::Swap { baked_yaw: 0.1, baked_pitch: 0.05 }] {
        let origin = base.origin(&game).unwrap();
        let moved = apply_head(&origin, IDENTITY, position);
        let centre = view_centre(&game, base, half_turn(0.2, -0.1), position).unwrap();
        assert!((0..3).all(|k| close(centre[k * 4 + 3], moved[k * 4 + 3])), "{base:?}");
    }
    assert_eq!(Base::Game.origin(&game), Some(game));
}

#[test]
fn the_drawn_origin_is_the_drawn_eyes_tracking_space() {
    let game = apply_head(&translation([4.0, 1.0, -2.0]), yaw_rotation(0.6), [0.0; 3]);
    let pose = HeadPose { valid: 1, orientation: half_turn(0.7, -0.4), position: [0.1, 1.6, -0.2], ..HeadPose::default() };
    let origin = tracking_origin(&game, 0.2).unwrap();
    for model in [HeadPosition::Off, HeadPosition::Lean, HeadPosition::Full] {
        let drawn = view_centre(&game, Base::Level { baked_yaw: 0.2 }, pose.orientation, model.position(&pose)).unwrap();
        let space = tracking_space(&drawn, pose.orientation, pose.position);
        assert!(distance(&model.drawn_origin(&origin, &pose), &space) < 1e-5, "{model:?}");
    }
    assert_eq!(HeadPosition::Full.drawn_origin(&origin, &pose), origin, "the full head is drawn where it is tracked");
    assert_eq!(HeadPosition::Lean.drawn_origin(&origin, &HeadPose::default()), origin);
}

#[test]
fn the_hands_land_where_the_drawn_eyes_see_them() {
    let game = apply_head(&translation([4.0, 1.0, -2.0]), yaw_rotation(0.6), [0.0; 3]);
    let head = half_turn(0.4, 0.2);
    let tracked = [0.1, 1.6, -0.2];
    // Drawn with the head 3 cm off where it was tracked (a head model's shift).
    let drawn = view_centre(&game, Base::Level { baked_yaw: 0.0 }, head, [0.13, 1.57, -0.2]).unwrap();
    let space = tracking_space(&drawn, head, tracked);
    // The tracked head itself lands on the drawn view.
    assert!(distance(&apply_head(&space, head, tracked), &drawn) < 1e-5);
    // A hand 40 cm ahead of the head moves by the same 3 cm.
    let hand = [0.1, 1.5, -0.6];
    let moved = apply_head(&space, IDENTITY, hand);
    let unshifted = apply_head(&tracking_origin(&game, 0.0).unwrap(), IDENTITY, hand);
    let shift = [moved[3] - unshifted[3], moved[7] - unshifted[7], moved[11] - unshifted[11]];
    let expected = apply_head(&tracking_origin(&game, 0.0).unwrap(), IDENTITY, [0.03, -0.03, 0.0]);
    let along = [expected[3] - game[3], expected[7] - game[7], expected[11] - game[11]];
    assert!((0..3).all(|i| (shift[i] - along[i]).abs() < 1e-5), "{shift:?} {along:?}");
}

#[test]
fn reach_and_both_eyes() {
    let near = translation([1.0, 2.0, 3.0]);
    assert!(within_reach(&translation([1.0, 2.0, 7.9]), &near, 5.0));
    assert!(!within_reach(&translation([1.0, 2.0, 8.1]), &near, 5.0));
    assert!(!within_reach(&[f32::NAN; 12], &near, 5.0));
    let both = Frustum::both_eyes(&[[-1.03, 0.87, 0.87, -1.04], [-0.87, 1.03, 0.9, -1.0]]).unwrap();
    assert!(close(both.left, (-1.03f32).tan()) && close(both.right, 1.03f32.tan()) && close(both.up, 0.9f32.tan()) && close(both.down, (-1.04f32).tan()));
    assert!(Frustum::both_eyes(&[[0.0; 4], [-0.87, 1.03, 0.9, -1.0]]).is_none());
}

#[test]
fn the_tracking_origin_turns_the_baked_yaw_out() {
    // A camera turned 30 degrees left and pitched: its origin looks straight down -Z, level.
    let yaw = 30f32.to_radians();
    let pitched = from_back_up([yaw.sin() * 0.9, 0.43, yaw.cos() * 0.9], [0.0, 1.0, 0.0], [5.0, 1.0, 2.0]).unwrap();
    let origin = tracking_origin(&pitched, yaw).unwrap();
    assert!((origin[10] - 1.0).abs() < 1e-4 && origin[6].abs() < 1e-6, "{origin:?}");
    assert_eq!((origin[3], origin[7], origin[11]), (5.0, 1.0, 2.0));
}

#[test]
fn synthetic_pose_and_ipd() {
    let fov = [[-0.9, 0.8, 0.8, -0.9]; 2];
    let pose = monaka_contract::protocol::HeadPose::synthetic(30.0, 0.0, fov, 0.064);
    assert!((crate::math::yaw(pose.orientation) - 30f32.to_radians()).abs() < 1e-5);
    let (yaw, pitch) = crate::camera::head_yaw_pitch(monaka_contract::protocol::HeadPose::synthetic(30.0, 20.0, fov, 0.064).orientation);
    assert!((yaw - 30f32.to_radians()).abs() < 1e-5 && (pitch - 20f32.to_radians()).abs() < 1e-5);
    assert_eq!(pose.ipd_or(0.07), 0.064);
    assert_eq!(monaka_contract::protocol::HeadPose { ipd: 0.2, ..pose }.ipd_or(0.07), 0.07);
    assert_eq!(monaka_contract::protocol::HeadPose { valid: 0, ..pose }.ipd_or(0.07), 0.07);
}

#[test]
fn three_by_three_rotations_compose_turn_vectors_and_split_from_frames() {
    let a = rotation(yaw_rotation(0.4));
    let b = rotation(yaw_rotation(-0.1));
    let both = rotation(crate::math::multiply(yaw_rotation(0.4), yaw_rotation(-0.1)));
    let close = |x: &[f32], y: &[f32]| x.iter().zip(y).all(|(p, q)| (p - q).abs() < 1e-5);
    assert!(close(&mul3(&a, &b), &both), "the product is the rotations in turn");
    assert!(close(&mul3(&a, &transpose3(&a)), &rotation(IDENTITY)), "a rotation's transpose undoes it");
    let v = [0.3, -1.0, 2.0];
    assert!(close(&apply3(&a, v), &crate::math::rotate(yaw_rotation(0.4), v)));
    let frame = from_rotation(&a, [1.0, 2.0, 3.0]);
    assert_eq!(rotation_part(&frame), a);
    assert_eq!(position(&frame), [1.0, 2.0, 3.0]);
}
