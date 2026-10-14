//! One eye as drawn: where it was and where it looked (tracking space), and its field of view.

use monaka_core::math::{Quat, Vec3, rotate};

#[cfg(test)]
mod tests;

/// An eye as its image was rendered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeView {
    pub orientation: Quat,
    pub position: Vec3,
    /// Angles (radians) of the image's left, right, up and down edges from straight ahead, as
    /// OpenXR gives a field of view (left and down negative).
    pub fov: [f32; 4],
}

impl EyeView {
    /// The rows of the tracking-to-eye transform (3x4): the inverse of the eye's pose.
    pub fn view_rows(&self) -> [[f32; 4]; 3] {
        let [x, y, z, w] = self.orientation;
        let inverse = [-x, -y, -z, w];
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|axis| rotate(inverse, axis));
        // Column `k` of the inverse rotation is where tracking axis `k` points in eye space.
        let moved = rotate(inverse, self.position);
        [0, 1, 2].map(|row| [axes[0][row], axes[1][row], axes[2][row], -moved[row]])
    }

    /// The tangents of the field of view's edges (left, right, up, down).
    pub fn tangents(&self) -> [f32; 4] {
        self.fov.map(f32::tan)
    }

    /// Where tracking-space `point` lands in an image `width` by `height` pixels (from its top
    /// left), or `None` behind the eye.
    pub fn pixel(&self, point: Vec3, width: f32, height: f32) -> Option<[f32; 2]> {
        let rows = self.view_rows();
        let v = rows.map(|r| r[0] * point[0] + r[1] * point[1] + r[2] * point[2] + r[3]);
        if v[2] >= -1e-3 {
            return None;
        }
        let [left, right, up, down] = self.tangents();
        let (tx, ty) = (v[0] / -v[2], v[1] / -v[2]);
        Some([(tx - left) / (right - left) * width, (up - ty) / (up - down) * height])
    }
}
