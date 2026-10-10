//! Depth-based stereo: both eyes made from one centre render and its depth buffer. Each pixel
//! moves sideways by its disparity, which for parallel eyes is proportional to nearness
//! (`near / distance`, 1 at the near plane, 0 at infinity). For any perspective projection the
//! depth value is affine in nearness, so a shader needs only `nearness = a * depth + b`.

use crate::camera::Mat44;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthMapping {
    pub a: f32,
    pub b: f32,
}

impl DepthMapping {
    /// From a row-major right-handed projection (clip w = -z) and its near distance: depth
    /// `d = -P[10] + P[11] / distance`, so `nearness = near * (d + P[10]) / P[11]`. Covers standard
    /// depth (0 at near, 1 at far) and reverse-Z infinite (d = near / distance).
    pub fn from_projection(p: &Mat44, near: f32) -> Option<Self> {
        let perspective = (p[14] + 1.0).abs() < 1e-3 && p[15].abs() < 1e-3;
        if !perspective || !near.is_finite() || near <= 0.0 || !p[11].is_finite() || p[11].abs() < 1e-9 || !p[10].is_finite() {
            return None;
        }
        Some(Self { a: near / p[11], b: near * p[10] / p[11] })
    }

    pub fn nearness(&self, depth: f32) -> f32 {
        (self.a * depth + self.b).clamp(0.0, 1.0)
    }
}

/// Pixels a point at the near plane moves in each eye: eyes `ipd/2` either side of a camera with
/// projection scale `p00`, image `width` pixels wide. A pixel moves `scale * nearness`.
pub fn disparity_scale(p00: f32, width: u32, ipd: f32, near: f32) -> f32 {
    0.5 * ipd * p00 * width as f32 * 0.5 / near
}

/// The largest shift in pixels: anything nearer than `comfort` metres shifts as if it were at
/// `comfort` (DL2: 0.5 m; nearer, the arms doubled).
pub fn max_shift(p00: f32, width: u32, ipd: f32, comfort: f32) -> f32 {
    0.5 * ipd * p00 * width as f32 * 0.5 / comfort
}

/// Row-major 4x4 product `a * b`.
pub fn multiply(a: &Mat44, b: &Mat44) -> Mat44 {
    let mut out = [0.0; 16];
    for row in 0..4 {
        for col in 0..4 {
            out[row * 4 + col] = (0..4).map(|k| a[row * 4 + k] * b[k * 4 + col]).sum();
        }
    }
    out
}

/// Inverse of a row-major 4x4 matrix, `None` if singular.
pub fn invert(m: &Mat44) -> Option<Mat44> {
    // Gauss-Jordan elimination with partial pivoting.
    let mut a = *m;
    let mut out = [0.0f32; 16];
    for i in 0..4 {
        out[i * 4 + i] = 1.0;
    }
    for col in 0..4 {
        let pivot = (col..4).max_by(|&x, &y| a[x * 4 + col].abs().total_cmp(&a[y * 4 + col].abs()))?;
        if a[pivot * 4 + col].abs() < 1e-12 {
            return None;
        }
        for k in 0..4 {
            a.swap(col * 4 + k, pivot * 4 + k);
            out.swap(col * 4 + k, pivot * 4 + k);
        }
        let scale = 1.0 / a[col * 4 + col];
        for k in 0..4 {
            a[col * 4 + k] *= scale;
            out[col * 4 + k] *= scale;
        }
        for row in (0..4).filter(|&r| r != col) {
            let factor = a[row * 4 + col];
            for k in 0..4 {
                a[row * 4 + k] -= factor * a[col * 4 + k];
                out[row * 4 + k] -= factor * out[col * 4 + k];
            }
        }
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// A camera-to-world 3x4 matrix as 4x4.
fn affine(m: &crate::camera::Mat34) -> Mat44 {
    let mut out = [0.0; 16];
    out[..12].copy_from_slice(m);
    out[15] = 1.0;
    out
}

/// The matrix taking a source pixel's normalised device coordinates and depth `(x, y, d, 1)`
/// to the target camera's clip space: target projection * target view * source camera-to-world *
/// inverse source projection. Divide by w for the target's normalised coordinates and depth (w is
/// only a homogeneous scale; the target's [`DepthMapping`] turns the divided depth into nearness).
/// Canonical cameras and projections.
pub fn reprojection(source_projection: &Mat44, source_camera: &crate::camera::Mat34, target_projection: &Mat44, target_camera: &crate::camera::Mat34) -> Option<Mat44> {
    let unproject = invert(source_projection)?;
    let target_view = invert(&affine(target_camera))?;
    Some(multiply(&multiply(&multiply(target_projection, &target_view), &affine(source_camera)), &unproject))
}

#[cfg(test)]
mod tests {
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
}
