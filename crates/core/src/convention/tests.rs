use super::*;

#[test]
fn an_id_placement_round_trips_with_its_axes_kept() {
    // id: x forward, y left, z up. Forward is canonical -z, left -x, up +y.
    assert_eq!(Basis::ID_TECH.canonical([1.0, 0.0, 0.0]), [-0.0, 0.0, -1.0]);
    assert_eq!(Basis::ID_TECH.canonical([0.0, 1.0, 0.0]), [-1.0, 0.0, -0.0]);
    let (origin, axis) = ([3.0, -2.0, 1.5], [0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let m = Basis::ID_TECH.from_origin_axis(origin, axis).unwrap();
    // The thing's own x axis (id +y, left) is canonical -x, its first column.
    assert_eq!([m[0], m[4], m[8]], [-1.0, 0.0, -0.0]);
    let (o, a) = Basis::ID_TECH.origin_axis(&m);
    assert!(close(&o, &origin) && close(&a, &axis));
    assert!(Basis::ID_TECH.from_origin_axis([f32::NAN, 0.0, 0.0], axis).is_none());
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

/// A canonical camera turned 90 degrees left (looking down -X) at (1, 2, 3) metres.
const TURNED: Mat34 = [0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 2.0, -1.0, 0.0, 0.0, 3.0];

#[test]
fn dying_light_is_canonical() {
    let c = CameraConvention::DYING_LIGHT;
    assert_eq!(c.to_canonical(&TURNED), Some(TURNED));
    let mut out = [0.0; 12];
    assert!(c.from_canonical(&TURNED, &mut out) && out == TURNED);
}

#[test]
fn unreal_round_trips_and_means_the_same_camera() {
    let c = CameraConvention::UNREAL;
    let mut unreal = [0.0; 16];
    assert!(c.from_canonical(&TURNED, &mut unreal));
    // Canonical forward is world -X; in Unreal's world (x forward, y right, z up) that is -Y.
    assert!(close(&unreal[0..4], &[0.0, -1.0, 0.0, 0.0]), "forward row {:?}", &unreal[0..4]);
    // Canonical up +Y is Unreal +Z; the position is in centimetres.
    assert!(close(&unreal[8..12], &[0.0, 0.0, 1.0, 0.0]));
    assert!(close(&unreal[12..16], &[-300.0, 100.0, 200.0, 1.0]));
    assert!(close(&c.to_canonical(&unreal).unwrap(), &TURNED));
}

#[test]
fn unity_round_trips() {
    let c = CameraConvention::UNITY;
    let mut unity = [0.0; 16];
    assert!(c.from_canonical(&TURNED, &mut unity));
    // Canonical world -Z (forward) is Unity world +Z; the camera looks down canonical -X,
    // which is Unity -X, and Unity's camera space looks down its own -Z: column 2 is +X.
    assert!(close(&unity[8..11], &[1.0, 0.0, 0.0]));
    assert!(close(&c.to_canonical(&unity).unwrap(), &TURNED));
}

#[test]
fn id_tech_round_trips_and_turns_left_with_the_head() {
    let c = CameraConvention { units_per_metre: 50.0, ..CameraConvention::ID_TECH };
    let mut id = [0.0; 16];
    assert!(c.from_canonical(&TURNED, &mut id));
    // Canonical forward world -X is id +Y (left); canonical up +Y is id +Z.
    assert!(close(&id[0..3], &[0.0, 1.0, 0.0]), "forward row {:?}", &id[0..3]);
    assert!(close(&id[8..11], &[0.0, 0.0, 1.0]));
    // (1, 2, 3) m canonical: 1 m right is id -Y, 2 m up id +Z, 3 m back id -X; 50 units per metre.
    assert!(close(&id[12..15], &[-150.0, -50.0, 100.0]), "origin {:?}", &id[12..15]);
    assert!(close(&c.to_canonical(&id).unwrap(), &TURNED));

    // An id camera looking down +X; the head turned 90 degrees left (about +Y in OpenXR).
    let east = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 20.0, 30.0, 1.0];
    let half = std::f32::consts::FRAC_PI_4;
    let turned = crate::camera::apply_head(&c.to_canonical(&east).unwrap(), [0.0, half.sin(), 0.0, half.cos()], [0.0, 0.0, -0.1]);
    let mut out = [0.0; 16];
    assert!(c.from_canonical(&turned, &mut out));
    assert!(close(&out[0..3], &[0.0, 1.0, 0.0]), "forward is now id left: {:?}", &out[0..3]);
    assert!(close(&out[4..7], &[-1.0, 0.0, 0.0]), "left is now id back: {:?}", &out[4..7]);
    // The head position is in the unturned camera's space: 10 cm forward is 5 units along id +X.
    assert!(close(&out[12..15], &[15.0, 20.0, 30.0]), "origin {:?}", &out[12..15]);
}

#[test]
fn cryengine_round_trips_and_means_the_same_camera() {
    let c = CameraConvention::CRYENGINE;
    let mut cry = [0.0; 12];
    assert!(c.from_canonical(&TURNED, &mut cry));
    // Canonical forward world -X is CryEngine -X: column 1 (forward) is (-1, 0, 0).
    assert!(close(&[cry[1], cry[5], cry[9]], &[-1.0, 0.0, 0.0]), "forward column {cry:?}");
    // Canonical up +Y is CryEngine +Z; (1, 2, 3) m is CryEngine (1, -3, 2).
    assert!(close(&[cry[2], cry[6], cry[10]], &[0.0, 0.0, 1.0]));
    assert!(close(&[cry[3], cry[7], cry[11]], &[1.0, -3.0, 2.0]));
    assert!(close(&c.to_canonical(&cry).unwrap(), &TURNED));
}

#[test]
fn left_handed_projection_converts() {
    // DirectXMath XMMatrixPerspectiveFovLH-style, stored for row vectors: z' = z*f/(f-n) - n*f/(f-n), w = z.
    let (n, f) = (0.1f32, 100.0f32);
    let mut lh = [0.0; 16];
    lh[0] = 1.0;
    lh[5] = 1.5;
    lh[10] = f / (f - n);
    lh[11] = 1.0; // w = +z (row vectors: element [2][3])
    lh[14] = -n * f / (f - n);
    let canonical = ProjectionConvention::LEFT_HANDED_ROW_VECTORS.to_canonical(&lh);
    // Right-handed column vectors: w = -z, so element [3][2] is -1.
    assert!((canonical[14] + 1.0).abs() < 1e-6 && (canonical[10] + f / (f - n)).abs() < 1e-5);
    let mapping = crate::depth::DepthMapping::from_projection(&canonical, n).unwrap();
    assert!((mapping.nearness(0.0) - 1.0).abs() < 1e-5 && (mapping.nearness(1.0) - n / f).abs() < 1e-5);
    assert!(close(&ProjectionConvention::LEFT_HANDED_ROW_VECTORS.from_canonical(&canonical), &lh));
}
