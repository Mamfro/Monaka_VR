use super::*;

fn close(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1.0e-9)
}

/// The rows of a 4x4 whose last column is `(0, 0, 0, 1)`.
fn to_rows(m: &Mat4d) -> Mat43d {
    let mut out = [0.0; 12];
    for row in 0..4 {
        out[row * 3..row * 3 + 3].copy_from_slice(&m[row * 4..row * 4 + 3]);
    }
    out
}

/// A camera turned 90 degrees about the up axis, at (1, 2, 3).
const TURNED: Mat43d = [0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 2.0, 3.0];

#[test]
fn rigid_inverse_round_trips() {
    let inv = rigid_inverse(&TURNED).unwrap();
    let product = mul(&from_rows(&TURNED), &from_rows(&inv));
    assert!(close(&product, &identity()), "{product:?}");
    assert!(close(&to_rows(&from_rows(&TURNED)), &TURNED));
    // Not orthonormal: refused.
    let mut skewed = TURNED;
    skewed[0] = 0.5;
    assert!(rigid_inverse(&skewed).is_none());
}

#[test]
fn general_inverse_matches_the_rigid_one_and_inverts_a_projection() {
    let general = inverse(&from_rows(&TURNED)).unwrap();
    assert!(close(&general, &from_rows(&rigid_inverse(&TURNED).unwrap())));
    let (n, f) = (0.1, 1000.0);
    let mut game = [0.0; 16];
    game[0] = 1.2;
    game[5] = 2.1;
    game[10] = f / (f - n);
    game[11] = 1.0;
    game[14] = -n * f / (f - n);
    let back = inverse(&game).unwrap();
    assert!(close(&mul(&game, &back), &identity()));
    assert!(inverse(&[0.0; 16]).is_none());
}

#[test]
fn off_centre_perspective_matches_the_canonical_one() {
    let (n, f) = (0.1f64, 1000.0f64);
    let mut game = [0.0; 16];
    game[0] = 1.0;
    game[5] = 1.5;
    game[10] = f / (f - n);
    game[11] = 1.0;
    game[14] = -n * f / (f - n);
    let tangents = [-0.9f64, 0.7, -0.8, 0.85];
    let p = perspective_lh(tangents, &game).unwrap();
    // The same frustum through the single-precision canonical path.
    let frustum = crate::camera::Frustum { left: tangents[0] as f32, right: tangents[1] as f32, up: tangents[3] as f32, down: tangents[2] as f32 };
    let convention = crate::convention::ProjectionConvention::LEFT_HANDED_ROW_VECTORS;
    let mut canonical = convention.to_canonical(&game.map(|v| v as f32));
    frustum.apply_to_projection(&mut canonical);
    let expected = convention.from_canonical(&canonical);
    for (a, b) in p.iter().zip(expected) {
        assert!((a - b as f64).abs() < 1.0e-5, "{p:?} vs {expected:?}");
    }
    // A frustum with no width is refused.
    assert!(perspective_lh([0.0, 0.0, -1.0, 1.0], &game).is_none());
}
