//! How an engine lays out cameras and projections, so the shared code works in one canonical form.
//!
//! Canonical (what [`crate::camera`], [`crate::aim`] and [`crate::depth`] assume): OpenXR axes
//! (+X right, +Y up, looking down -Z, right-handed), metres, and a camera as a row-major 3x4
//! camera-to-world matrix whose columns are its right, up and back axes and its position. The
//! Dying Light engines store exactly that, so their conversion is the identity. Other engines
//! differ in handedness (Unreal and Unity are left-handed), up axis (Unreal: Z), units (Unreal:
//! centimetres) and matrix layout; a game crate describes them once with a [`CameraConvention`]
//! and converts at the edge, where it reads and writes the engine's camera.

use crate::camera::{Mat34, Mat44};

/// A signed axis of an engine's coordinate system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Axis {
    fn index(self) -> usize {
        match self {
            Axis::PosX | Axis::NegX => 0,
            Axis::PosY | Axis::NegY => 1,
            Axis::PosZ | Axis::NegZ => 2,
        }
    }

    fn sign(self) -> f32 {
        match self {
            Axis::PosX | Axis::PosY | Axis::PosZ => 1.0,
            _ => -1.0,
        }
    }

    /// The component of `v` along this axis.
    fn pick(self, v: [f32; 3]) -> f32 {
        self.sign() * v[self.index()]
    }

    /// Adds `value` along this axis to `v`.
    fn put(self, v: &mut [f32; 3], value: f32) {
        v[self.index()] += self.sign() * value;
    }
}

/// Which of an engine's axes point right, up and forward (for a camera: in its own space; for
/// the world: for an unrotated camera).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Basis {
    pub right: Axis,
    pub up: Axis,
    pub forward: Axis,
}

impl Basis {
    pub const OPENXR: Self = Self { right: Axis::PosX, up: Axis::PosY, forward: Axis::NegZ };
    /// Unreal: X forward, Y right, Z up (left-handed).
    pub const UNREAL: Self = Self { right: Axis::PosY, up: Axis::PosZ, forward: Axis::PosX };
    /// Unity and DirectX's left-handed default: X right, Y up, Z forward.
    pub const LEFT_HANDED_Y_UP: Self = Self { right: Axis::PosX, up: Axis::PosY, forward: Axis::PosZ };
    /// id Software's engines: X forward, Y left, Z up (right-handed).
    pub const ID_TECH: Self = Self { right: Axis::NegY, up: Axis::PosZ, forward: Axis::PosX };
    /// CryEngine: X right, Y forward, Z up (right-handed).
    pub const CRYENGINE: Self = Self { right: Axis::PosX, up: Axis::PosZ, forward: Axis::PosY };

    /// A vector in this basis expressed canonically (+X right, +Y up, -Z forward).
    pub fn canonical(self, v: [f32; 3]) -> [f32; 3] {
        [self.right.pick(v), self.up.pick(v), -self.forward.pick(v)]
    }

    /// A canonical vector expressed in this basis.
    pub fn express(self, v: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0; 3];
        self.right.put(&mut out, v[0]);
        self.up.put(&mut out, v[1]);
        self.forward.put(&mut out, -v[2]);
        out
    }

    /// A placement in a world of this basis, kept as an `origin` and `axis` rows (the placed
    /// thing's own x, y and z axes: id Tech's and the Void engine's entities, render models and
    /// bones), as a canonical-world frame (columns: those axes, then the origin). Only the world
    /// changes: the axes stay the thing's own (a model's bone axes are its own), and the units
    /// stay the game's. None when not finite.
    pub fn from_origin_axis(self, origin: [f32; 3], axis: [f32; 9]) -> Option<Mat34> {
        let [x, y, z, p] = [[axis[0], axis[1], axis[2]], [axis[3], axis[4], axis[5]], [axis[6], axis[7], axis[8]], origin].map(|v| self.canonical(v));
        let m = [x[0], y[0], z[0], p[0], x[1], y[1], z[1], p[1], x[2], y[2], z[2], p[2]];
        m.iter().all(|v| v.is_finite()).then_some(m)
    }

    /// A canonical-world frame as an origin and axis rows in a world of this basis
    /// ([`Basis::from_origin_axis`]'s inverse).
    pub fn origin_axis(self, m: &Mat34) -> ([f32; 3], [f32; 9]) {
        let column = |c: usize| self.express([m[c], m[4 + c], m[8 + c]]);
        let (x, y, z) = (column(0), column(1), column(2));
        (column(3), [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]])
    }
}

/// Where a camera matrix keeps its axes and position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixLayout {
    /// Row-major 3x4: the camera's axes and position are the columns (Dying Light).
    Columns3x4,
    /// Row-major 4x4 with the axes and position as columns (column vectors stored by row).
    Columns4x4,
    /// 4x4 with the axes and position as rows: row-vector maths (DirectXMath, Unreal's FMatrix)
    /// or column-major storage of column-vector maths (glm, Unity in memory).
    Rows4x4,
}

impl MatrixLayout {
    /// The number of floats a camera matrix takes.
    pub fn len(self) -> usize {
        match self {
            MatrixLayout::Columns3x4 => 12,
            _ => 16,
        }
    }

    pub fn is_empty(self) -> bool {
        false
    }

    /// Column `k` (0-2 axes, 3 position) of the camera-to-world transform.
    fn get(self, m: &[f32], k: usize) -> [f32; 3] {
        match self {
            MatrixLayout::Columns3x4 => [m[k], m[4 + k], m[8 + k]],
            MatrixLayout::Columns4x4 => [m[k], m[4 + k], m[8 + k]],
            MatrixLayout::Rows4x4 => [m[4 * k], m[4 * k + 1], m[4 * k + 2]],
        }
    }

    fn set(self, m: &mut [f32], k: usize, v: [f32; 3]) {
        for (i, value) in v.into_iter().enumerate() {
            match self {
                MatrixLayout::Columns3x4 | MatrixLayout::Columns4x4 => m[4 * i + k] = value,
                MatrixLayout::Rows4x4 => m[4 * k + i] = value,
            }
        }
        if self != MatrixLayout::Columns3x4 {
            // The homogeneous row or column: 0 for the axes, 1 for the position.
            let w = if k == 3 { 1.0 } else { 0.0 };
            match self {
                MatrixLayout::Columns4x4 => m[12 + k] = w,
                _ => m[4 * k + 3] = w,
            }
        }
    }
}

/// How an engine stores a camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraConvention {
    /// The camera's own axes, as stored in the matrix's first three columns (or rows).
    pub camera: Basis,
    /// The world's axes.
    pub world: Basis,
    pub layout: MatrixLayout,
    /// Engine distance units per metre (Unreal: 100).
    pub units_per_metre: f32,
}

impl CameraConvention {
    /// Canonical already: both Dying Light engines.
    pub const DYING_LIGHT: Self = Self { camera: Basis::OPENXR, world: Basis::OPENXR, layout: MatrixLayout::Columns3x4, units_per_metre: 1.0 };
    /// Unreal's camera-to-world transform as an `FMatrix` (rows: forward X, right Y, up Z, origin).
    pub const UNREAL: Self = Self { camera: Basis::UNREAL, world: Basis::UNREAL, layout: MatrixLayout::Rows4x4, units_per_metre: 100.0 };
    /// id Tech (4, 5): an origin and an axis matrix whose rows are forward, left and up, packed
    /// here as rows `[forward, left, up, origin]`. Units differ per game: set `units_per_metre`.
    pub const ID_TECH: Self = Self { camera: Basis::ID_TECH, world: Basis::ID_TECH, layout: MatrixLayout::Rows4x4, units_per_metre: 1.0 };
    /// CryEngine's `CCamera` matrix: a row-major `Matrix34` whose columns are the camera's right,
    /// forward and up axes and its position, in metres.
    pub const CRYENGINE: Self = Self { camera: Basis::CRYENGINE, world: Basis::CRYENGINE, layout: MatrixLayout::Columns3x4, units_per_metre: 1.0 };
    /// Unity's `cameraToWorldMatrix` in memory (column-major).
    pub const UNITY: Self = Self {
        camera: Basis { right: Axis::PosX, up: Axis::PosY, forward: Axis::NegZ },
        world: Basis::LEFT_HANDED_Y_UP,
        layout: MatrixLayout::Rows4x4,
        units_per_metre: 1.0,
    };

    /// The engine's camera-to-world matrix in canonical form.
    pub fn to_canonical(&self, m: &[f32]) -> Option<Mat34> {
        if m.len() < self.layout.len() || !self.units_per_metre.is_finite() || self.units_per_metre <= 0.0 {
            return None;
        }
        let axis = |k| self.world.canonical(self.layout.get(m, k));
        // The engine's camera axes, as world vectors.
        let stored = [axis(0), axis(1), axis(2)];
        let along = |a: Axis| {
            let v = stored[a.index()];
            [a.sign() * v[0], a.sign() * v[1], a.sign() * v[2]]
        };
        let right = along(self.camera.right);
        let up = along(self.camera.up);
        let forward = along(self.camera.forward);
        let position = axis(3).map(|v| v / self.units_per_metre);
        let mut out = [0.0; 12];
        for row in 0..3 {
            out[row * 4] = right[row];
            out[row * 4 + 1] = up[row];
            out[row * 4 + 2] = -forward[row];
            out[row * 4 + 3] = position[row];
        }
        Some(out)
    }

    /// A canonical camera-to-world matrix in the engine's layout (`out` has [`MatrixLayout::len`]
    /// floats; for 4x4 layouts the homogeneous row or column is written too).
    pub fn from_canonical(&self, m: &Mat34, out: &mut [f32]) -> bool {
        if out.len() < self.layout.len() {
            return false;
        }
        let column = |k: usize| [m[k], m[4 + k], m[8 + k]];
        let (right, up, back) = (column(0), column(1), column(2));
        let world = |v: [f32; 3]| self.world.express(v);
        // The engine's stored axis k is whichever of right/up/forward lies along camera axis k.
        let mut stored = [[0.0f32; 3]; 3];
        for (axis, vector) in [(self.camera.right, right), (self.camera.up, up), (self.camera.forward, back.map(|v| -v))] {
            stored[axis.index()] = vector.map(|v| axis.sign() * v);
        }
        for (k, vector) in stored.into_iter().enumerate() {
            self.layout.set(out, k, world(vector));
        }
        self.layout.set(out, 3, world(column(3).map(|v| v * self.units_per_metre)));
        true
    }
}

/// How an engine stores a perspective projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectionConvention {
    /// The matrix is stored transposed (row vectors, or column-major storage).
    pub transposed: bool,
    /// The view-space z of a point in front of the camera has this sign: -1 right-handed
    /// (looking down -z), +1 left-handed.
    pub forward_z: f32,
}

impl ProjectionConvention {
    /// Row-major, column vectors, right-handed: both Dying Light engines.
    pub const RIGHT_HANDED: Self = Self { transposed: false, forward_z: -1.0 };
    /// Row vectors, left-handed: DirectXMath's `XMMatrixPerspectiveLH` family, Unreal.
    pub const LEFT_HANDED_ROW_VECTORS: Self = Self { transposed: true, forward_z: 1.0 };

    /// Index of element (row, column) of the column-vector matrix in storage.
    pub fn at(&self, row: usize, column: usize) -> usize {
        if self.transposed { column * 4 + row } else { row * 4 + column }
    }

    /// The projection's elements in canonical form (row-major, column vectors, right-handed).
    pub fn to_canonical(&self, p: &Mat44) -> Mat44 {
        let mut out = [0.0; 16];
        for row in 0..4 {
            for column in 0..4 {
                // Right-handed view z is the negated left-handed z: flip the z column.
                let flip = if column == 2 && self.forward_z > 0.0 { -1.0 } else { 1.0 };
                out[row * 4 + column] = flip * p[self.at(row, column)];
            }
        }
        out
    }

    pub fn from_canonical(&self, p: &Mat44) -> Mat44 {
        let mut out = [0.0; 16];
        for row in 0..4 {
            for column in 0..4 {
                let flip = if column == 2 && self.forward_z > 0.0 { -1.0 } else { 1.0 };
                out[self.at(row, column)] = flip * p[row * 4 + column];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
