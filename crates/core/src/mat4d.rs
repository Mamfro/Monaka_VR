//! Double-precision 4x4 matrices for row vectors (`v' = v * M`), as engines built on DirectXMath
//! keep them: Northlight's `m::View` stores its transforms this way in doubles, so the eye camera
//! and projection are written in the same form without a round trip through floats.
//!
//! A rigid transform is kept as four rows of three (`Mat43d`): the three axes, then the position.

pub type Mat4d = [f64; 16];
pub type Mat43d = [f64; 12];

pub fn identity() -> Mat4d {
    let mut m = [0.0; 16];
    for k in 0..4 {
        m[k * 5] = 1.0;
    }
    m
}

/// `a * b` for row vectors: `v * a * b` applies `a` first.
pub fn mul(a: &Mat4d, b: &Mat4d) -> Mat4d {
    let mut out = [0.0; 16];
    for row in 0..4 {
        for column in 0..4 {
            out[row * 4 + column] = (0..4).map(|k| a[row * 4 + k] * b[k * 4 + column]).sum();
        }
    }
    out
}

/// The 4x4 of a rigid transform: the rows with a trailing 0, the position with a trailing 1.
pub fn from_rows(m: &Mat43d) -> Mat4d {
    let mut out = [0.0; 16];
    for row in 0..4 {
        out[row * 4..row * 4 + 3].copy_from_slice(&m[row * 3..row * 3 + 3]);
    }
    out[15] = 1.0;
    out
}

/// The inverse of a rigid transform (orthonormal axes): the axes transposed, the position taken
/// back through them. `None` when the axes are not orthonormal.
pub fn rigid_inverse(m: &Mat43d) -> Option<Mat43d> {
    let axis = |k: usize| [m[k * 3], m[k * 3 + 1], m[k * 3 + 2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let axes = [axis(0), axis(1), axis(2)];
    for (i, a) in axes.iter().enumerate() {
        for (j, b) in axes.iter().enumerate() {
            let expected = if i == j { 1.0 } else { 0.0 };
            if (dot(*a, *b) - expected).abs() > 1.0e-6 {
                return None;
            }
        }
    }
    let position = axis(3);
    let mut out = [0.0; 12];
    for row in 0..3 {
        for column in 0..3 {
            out[row * 3 + column] = m[column * 3 + row];
        }
    }
    for column in 0..3 {
        out[9 + column] = -dot(position, axes[column]);
    }
    Some(out)
}

/// The general inverse, by cofactors. `None` when singular.
pub fn inverse(m: &Mat4d) -> Option<Mat4d> {
    let mut inv = [0.0; 16];
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if !det.is_finite() || det.abs() < 1.0e-300 {
        return None;
    }
    for v in &mut inv {
        *v /= det;
    }
    Some(inv)
}

/// A left-handed perspective for row vectors (DirectXMath's `XMMatrixPerspectiveOffCenterLH`
/// family) with the frustum's edge tangents (left, right, bottom, top at unit distance), and the
/// depth terms (`[2][2]`, `[3][2]`) taken from `depth_from`, the game's own projection, so near,
/// far and any reversed depth stay as the game set them. `None` when the frustum is degenerate.
pub fn perspective_lh(tangents: [f64; 4], depth_from: &Mat4d) -> Option<Mat4d> {
    let [l, r, b, t] = tangents;
    let (w, h) = (r - l, t - b);
    if !(w.is_finite() && h.is_finite()) || w < 1.0e-3 || h < 1.0e-3 {
        return None;
    }
    let mut p = [0.0; 16];
    p[0] = 2.0 / w;
    p[5] = 2.0 / h;
    p[8] = -(l + r) / w;
    p[9] = -(t + b) / h;
    p[10] = depth_from[10];
    p[11] = 1.0;
    p[14] = depth_from[14];
    Some(p)
}

/// Whether every element is finite.
pub fn all_finite(m: &[f64]) -> bool {
    m.iter().all(|v| v.is_finite())
}

#[cfg(test)]
mod tests;
