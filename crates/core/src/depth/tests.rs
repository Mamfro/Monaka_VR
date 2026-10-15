use super::*;

fn projection(p10: f32, p11: f32) -> Mat44 {
    let mut p = [0.0; 16];
    p[0] = 1.0;
    p[5] = 1.7;
    p[10] = p10;
    p[11] = p11;
    p[14] = -1.0;
    p
}

#[test]
fn standard_depth_maps_to_nearness() {
    let (n, f) = (0.1f32, 1000.0f32);
    let mapping = DepthMapping::from_projection(&projection(f / (n - f), n * f / (n - f)), n).unwrap();
    assert!((mapping.nearness(0.0) - 1.0).abs() < 1e-5, "near plane");
    assert!((mapping.nearness(1.0) - n / f).abs() < 1e-5, "far plane");
    // A point 2 m away: d = -P10 + P11 / 2.
    let d = -(f / (n - f)) + n * f / (n - f) / 2.0;
    assert!((mapping.nearness(d) - n / 2.0).abs() < 1e-4);
}

#[test]
fn reverse_infinite_depth_is_nearness() {
    let mapping = DepthMapping::from_projection(&projection(0.0, 0.05), 0.05).unwrap();
    assert_eq!(mapping, DepthMapping { a: 1.0, b: 0.0 });
    assert!(DepthMapping::from_projection(&[1.0; 16], 0.05).is_none(), "not a perspective projection");
}

#[test]
fn a_projection_gives_its_planes_in_metres() {
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-4 * b.abs().max(1.0);
    // Reverse-Z infinite, near 0.05 game units of a metre each: far infinite at depth 0.
    let params = depth_params(&projection(0.0, 0.05), 1.0).unwrap();
    assert_eq!(params, DepthParams::new(0.05, f32::INFINITY, true));
    assert_eq!(params.distances_at_0_and_1(), (f32::INFINITY, 0.05));
    // The same in centimetres (100 units to a metre).
    assert_eq!(depth_params(&projection(0.0, 5.0), 100.0), Some(DepthParams::new(0.05, f32::INFINITY, true)));
    // Standard depth, finite: near at 0, far at 1.
    let (n, f) = (0.1f32, 1000.0f32);
    let standard = depth_params(&projection(f / (n - f), n * f / (n - f)), 1.0).unwrap();
    assert!(standard.reversed == 0 && close(standard.near, n) && close(standard.far, f), "{standard:?}");
    // Standard infinite (P10 -1, P11 -near), with a rounding error in P10.
    let infinite = depth_params(&projection(-0.99999994, -n), 1.0).unwrap();
    assert!(infinite.reversed == 0 && close(infinite.near, n) && infinite.far > 1e4, "{infinite:?}");
    // Reversed finite: far at 0, near at 1.
    let reversed = depth_params(&projection(n / (f - n), n * f / (f - n)), 1.0).unwrap();
    assert!(reversed.reversed != 0 && close(reversed.near, n) && close(reversed.far, f), "{reversed:?}");
    assert!(depth_params(&[1.0; 16], 1.0).is_none(), "not a perspective projection");
    assert!(depth_params(&projection(0.0, 0.05), 0.0).is_none(), "no units");
    assert!(depth_params(&projection(0.0, -0.05), 1.0).is_none(), "a plane behind the eye");
}

fn apply(m: &Mat44, v: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|row| (0..4).map(|k| m[row * 4 + k] * v[k]).sum())
}

#[test]
fn reprojection_moves_a_point_between_cameras() {
    let (n, f) = (0.1f32, 1000.0f32);
    let mut p = projection(f / (n - f), n * f / (n - f));
    p[5] = 1.0;
    let left: crate::camera::Mat34 = [1.0, 0.0, 0.0, -0.032, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let right: crate::camera::Mat34 = [1.0, 0.0, 0.0, 0.032, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    // A point straight ahead of the left eye, 2 m away: x_ndc 0, depth from the projection.
    let d = -p[10] + p[11] / 2.0;
    let m = reprojection(&p, &left, &p, &right).unwrap();
    let clip = apply(&m, [0.0, 0.0, d, 1.0]);
    // In the right eye it sits 64 mm to the left at 2 m: x_ndc = -0.064/2 * P00, same depth.
    assert!((clip[0] / clip[3] + 0.032).abs() < 1e-4, "{clip:?}");
    assert!((clip[2] / clip[3] - d).abs() < 1e-4, "same distance, same depth: {clip:?} d {d}");
    let near = DepthMapping::from_projection(&p, n).unwrap().nearness(clip[2] / clip[3]);
    assert!((near - n / 2.0).abs() < 1e-4);
    assert!(invert(&[0.0; 16]).is_none());
    let identity = multiply(&p, &invert(&p).unwrap());
    assert!(identity.iter().enumerate().all(|(i, v)| (v - if i % 5 == 0 { 1.0 } else { 0.0 }).abs() < 1e-4));
}

#[test]
fn disparity_and_comfort() {
    // 64 mm IPD, P00 1, 2000 px wide, near 0.05: a near-plane point moves 640 px; at 0.5 m 64 px.
    assert!((disparity_scale(1.0, 2000, 0.064, 0.05) - 640.0).abs() < 1e-3);
    assert!((max_shift(1.0, 2000, 0.064, 0.5) - 64.0).abs() < 1e-3);
}
