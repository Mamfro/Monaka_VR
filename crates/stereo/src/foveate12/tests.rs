use super::*;

const TILE: u32 = 16;

fn symmetric() -> Frustum {
    Frustum { left: -1.0, right: 1.0, up: 1.0, down: -1.0 }
}

/// A direction `yaw` degrees right and `pitch` degrees up of ahead.
fn towards(yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    [sy * cp, sp, -cy * cp]
}

#[test]
fn rates_are_d3d12s() {
    assert_eq!(rate::R1X1 as i32, D3D12_SHADING_RATE_1X1.0);
    assert_eq!(rate::R2X2 as i32, D3D12_SHADING_RATE_2X2.0);
    assert_eq!(rate::R2X4 as i32, D3D12_SHADING_RATE_2X4.0);
    assert_eq!(rate::R4X4 as i32, D3D12_SHADING_RATE_4X4.0);
}

#[test]
fn the_gaze_lands_on_its_tile() {
    let pixels = (1600, 1600);
    assert_eq!(tiles(pixels, TILE), (100, 100));
    assert_eq!(tiles((1601, 1599), TILE), (101, 100), "a part tile counts");
    // Ahead: the middle of a symmetric frustum.
    assert_eq!(tile_of(&symmetric(), pixels, TILE, AHEAD), Some((50, 50)));
    // 45 degrees right is the right edge (tangent 1): clamped onto the last tile.
    assert_eq!(tile_of(&symmetric(), pixels, TILE, towards(45.0, 0.0)), Some((99, 50)));
    // Up is towards row 0; tangent 0.5 up is a quarter of the way down.
    let up = [0.0, 0.5, -1.0];
    assert_eq!(tile_of(&symmetric(), pixels, TILE, up), Some((50, 25)));
    // Right and down: x tangent 0.5, y tangent -0.5.
    assert_eq!(tile_of(&symmetric(), pixels, TILE, [0.5, -0.5, -1.0]), Some((75, 75)));
    // Behind, or sideways: nowhere on the image.
    assert_eq!(tile_of(&symmetric(), pixels, TILE, [0.0, 0.0, 1.0]), None);
    assert_eq!(tile_of(&symmetric(), pixels, TILE, [1.0, 0.0, 0.0]), None);
    assert_eq!(tile_of(&symmetric(), pixels, TILE, [f32::NAN, 0.0, -1.0]), None);
}

#[test]
fn straight_ahead_on_an_asymmetric_frustum_is_off_centre() {
    // A left eye reaching further out (left) than in: tangents left -1.2, right 0.8; up 1.0,
    // down -0.6. Ahead is 1.2 / 2.0 across and 1.0 / 1.6 down.
    let frustum = Frustum { left: -1.2, right: 0.8, up: 1.0, down: -0.6 };
    let pixels = (2000, 1600);
    let [u, v] = image_point(&frustum, AHEAD).unwrap();
    assert!((u - 0.6).abs() < 1e-6 && (v - 0.625).abs() < 1e-6, "{u} {v}");
    assert_eq!(tile_of(&frustum, pixels, TILE, AHEAD), Some((75, 62)));
    // It agrees with the frustum's own idea of ahead (clip space: x right, y up).
    let [cx, cy] = frustum.straight_ahead();
    assert!(((cx + 1.0) * 0.5 - u).abs() < 1e-6 && ((1.0 - cy) * 0.5 - v).abs() < 1e-6);
    // The image of that tile is the full rate; the far left edge the outer rate.
    let rings = Rings { inner: 15f32.to_radians(), middle: 30f32.to_radians(), outer: rate::R4X4 };
    let (across, down) = tiles(pixels, TILE);
    let pitch = 256;
    let mut image = vec![0xffu8; pitch * down as usize];
    fill(&frustum, pixels, TILE, (75, 62), &rings, &mut image, pitch);
    assert_eq!(image[62 * pitch + 75], rate::R1X1);
    assert_eq!(image[62 * pitch], rate::R4X4);
    assert_eq!(image[62 * pitch + across as usize], 0xff, "the row's padding is left");
}

#[test]
fn the_rings_go_by_the_angle_from_the_gaze() {
    let pixels = (1600, 1600);
    let rings = Rings { inner: 15f32.to_radians(), middle: 30f32.to_radians(), outer: rate::R2X4 };
    let (across, down) = tiles(pixels, TILE);
    let pitch = across.next_multiple_of(256) as usize;
    let mut image = vec![0u8; pitch * down as usize];
    let centre = tile_of(&symmetric(), pixels, TILE, AHEAD).unwrap();
    fill(&symmetric(), pixels, TILE, centre, &rings, &mut image, pitch);
    let rate_towards = |yaw: f32, pitch_degrees: f32| {
        let (x, y) = tile_of(&symmetric(), pixels, TILE, towards(yaw, pitch_degrees)).unwrap();
        image[y as usize * pitch + x as usize]
    };
    assert_eq!(rate_towards(0.0, 0.0), rate::R1X1);
    assert_eq!(rate_towards(10.0, 0.0), rate::R1X1);
    assert_eq!(rate_towards(0.0, -10.0), rate::R1X1);
    assert_eq!(rate_towards(20.0, 0.0), rate::R2X2);
    assert_eq!(rate_towards(0.0, 25.0), rate::R2X2);
    assert_eq!(rate_towards(40.0, 0.0), rate::R2X4);
    assert_eq!(image[0], rate::R2X4, "the corner");
    // Round: as many full-rate tiles across as down.
    let row = &image[centre.1 as usize * pitch..centre.1 as usize * pitch + across as usize];
    let column: Vec<u8> = (0..down as usize).map(|y| image[y * pitch + centre.0 as usize]).collect();
    let full = |line: &[u8]| line.iter().filter(|&&r| r == rate::R1X1).count();
    assert_eq!(full(row), full(&column));

    // The gaze moved 20 degrees right: the sharp area with it.
    let moved = tile_of(&symmetric(), pixels, TILE, towards(20.0, 0.0)).unwrap();
    fill(&symmetric(), pixels, TILE, moved, &rings, &mut image, pitch);
    assert_eq!(image[moved.1 as usize * pitch + moved.0 as usize], rate::R1X1);
    assert_eq!(image[centre.1 as usize * pitch + centre.0 as usize], rate::R2X2, "ahead is 20 degrees off now");
}

#[test]
fn without_a_gaze_the_rings_widen_and_without_coarse_rates_the_edge_is_2x2() {
    let rings = Rings { inner: 15f32.to_radians(), middle: 30f32.to_radians(), outer: rate::R4X4 };
    let fixed = rings.fixed();
    assert!((fixed.inner.to_degrees() - 25.0).abs() < 1e-4 && (fixed.middle.to_degrees() - 40.0).abs() < 1e-4);
    assert_eq!(fixed.outer, rate::R4X4);
    assert_eq!(rings.basic().outer, rate::R2X2);
    // A 20 degree tile: 2x2 with a gaze, full rate without one.
    let cos = 20f32.to_radians().cos();
    assert_eq!((rings.rate(cos), fixed.rate(cos)), (rate::R2X2, rate::R1X1));
}

#[test]
fn options_read_as_rings() {
    let rings = Rings::read(&Options::parse(""));
    assert!((rings.inner.to_degrees() - 15.0).abs() < 1e-4 && (rings.middle.to_degrees() - 30.0).abs() < 1e-4);
    assert_eq!(rings.outer, rate::R2X4);
    let rings = Rings::read(&Options::parse("foveated_inner=20\nfoveated_middle=10\nfoveated_outer=4x4"));
    assert!((rings.middle - rings.inner).abs() < 1e-6, "the middle ring is never inside the sharp area");
    assert_eq!(rings.outer, rate::R4X4);
}

#[test]
fn a_gaze_counts_when_valid_fresh_and_ahead() {
    let mut pose = HeadPose { valid: 1, ..HeadPose::default() };
    assert_eq!(live_gaze(&pose), None, "no gaze");
    pose.gaze.valid = 1;
    pose.gaze.direction = towards(10.0, 5.0);
    assert_eq!(live_gaze(&pose), Some(pose.gaze.direction), "no tick: taken as it is");
    pose.gaze.received_tick = monaka_channel::tick();
    assert!(live_gaze(&pose).is_some());
    pose.gaze.received_tick = monaka_channel::tick().saturating_sub(5_000);
    assert_eq!(live_gaze(&pose), None, "stale");
    pose.gaze.received_tick = 0;
    pose.gaze.direction = [0.0, 0.0, 1.0];
    assert_eq!(live_gaze(&pose), None, "behind");
}

#[test]
fn a_view_takes_its_mip_levels_size() {
    assert_eq!(view_size(std::ptr::null_mut(), None), None);
    assert_eq!(Pass { targets: Targets::Unknown, viewport: Some((8, 8)), bound: None }.size(), Some(((8, 8), false)));
    assert_eq!(Pass { targets: Targets::Sized((4, 4)), viewport: Some((8, 8)), bound: None }.size(), Some(((4, 4), true)));
    assert_eq!(Pass { targets: Targets::Mixed, ..Pass::default() }.size(), None);
}
