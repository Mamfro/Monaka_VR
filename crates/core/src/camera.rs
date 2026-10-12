//! Camera matrices shared by the producers.
//!
//! Everything here is in the canonical form ([`crate::convention`]): a row-major 3x4
//! camera-to-world matrix (columns right, up, back, position; metres) in OpenXR axes (+X right,
//! +Y up, looking down -Z), so a head pose composes directly onto it; the view matrix is the
//! rigid inverse. Both Dying Light engines store cameras this way; other engines convert first.

use crate::math::{Quat, Vec3};
use crate::protocol::HeadPose;

/// How the head's position moves the view (the `head_position` option).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadPosition {
    /// Rotation only: the eyes stay at the game camera.
    Off,
    /// Body movement (leaning) only: the eye movement of turning the head about the neck is taken
    /// out, so a weapon's sights stay lined up with the dominant eye at any angle.
    Lean,
    /// Everything the headset reports.
    Full,
}

impl HeadPosition {
    /// Where the eyes' centre sits against the tracking origin for `pose` (metres).
    pub fn position(self, pose: &HeadPose) -> [f32; 3] {
        match self {
            HeadPosition::Off => [0.0; 3],
            HeadPosition::Lean => crate::math::lean_only(pose.orientation, pose.position),
            HeadPosition::Full => pose.position,
        }
    }
}


/// Row-major 3x4 camera-to-world matrix: columns right, up, back, position.
pub type Mat34 = [f32; 12];
/// Row-major 4x4 projection.
pub type Mat44 = [f32; 16];

/// Row-major 3x3 rotation of a unit quaternion (xyzw).
pub fn rotation(q: Quat) -> [f32; 9] {
    let [x, y, z, w] = q;
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y - z * w),
        2.0 * (x * z + y * w),
        2.0 * (x * y + z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z - x * w),
        2.0 * (x * z - y * w),
        2.0 * (y * z + x * w),
        1.0 - 2.0 * (x * x + y * y),
    ]
}

pub fn all_finite(values: &[f32]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// The camera turned and moved by a pose given in the camera's own space: `camera * [R|p]`.
pub fn apply_head(camera: &Mat34, orientation: Quat, position: Vec3) -> Mat34 {
    let r = rotation(orientation);
    let mut out = [0.0; 12];
    for row in 0..3 {
        let axes = [camera[row * 4], camera[row * 4 + 1], camera[row * 4 + 2]];
        for col in 0..3 {
            out[row * 4 + col] = axes[0] * r[col] + axes[1] * r[3 + col] + axes[2] * r[6 + col];
        }
        out[row * 4 + 3] = camera[row * 4 + 3] + axes[0] * position[0] + axes[1] * position[1] + axes[2] * position[2];
    }
    out
}

/// World-to-camera matrix of a rigid camera-to-world matrix.
pub fn rigid_inverse(m: &Mat34) -> Mat34 {
    let mut view = [0.0; 12];
    for row in 0..3 {
        for col in 0..3 {
            view[row * 4 + col] = m[col * 4 + row];
        }
        view[row * 4 + 3] = -(m[row] * m[3] + m[4 + row] * m[7] + m[8 + row] * m[11]);
    }
    view
}

/// Whether `view` and `camera` invert each other within `tolerance`.
pub fn mutually_inverse(view: &Mat34, camera: &Mat34, tolerance: f32) -> bool {
    for row in 0..3 {
        for col in 0..4 {
            let mut value = if col == 3 { view[row * 4 + 3] } else { 0.0 };
            for axis in 0..3 {
                value += view[row * 4 + axis] * camera[axis * 4 + col];
            }
            let expected = if col == 3 { 0.0 } else if row == col { 1.0 } else { 0.0 };
            if !value.is_finite() || (value - expected).abs() > tolerance {
                return false;
            }
        }
    }
    true
}

/// Whether `camera` is finite with orthonormal axes (within `tolerance`): a camera, not a
/// half-written matrix or something else read from game memory.
pub fn is_rigid(camera: &Mat34, tolerance: f32) -> bool {
    all_finite(camera)
        && (0..3).all(|a| {
            (0..3).all(|b| {
                let dot: f32 = (0..3).map(|row| camera[row * 4 + a] * camera[row * 4 + b]).sum();
                (dot - if a == b { 1.0 } else { 0.0 }).abs() <= tolerance
            })
        })
}

/// The camera moved `distance` metres along its own right axis.
pub fn offset_right(camera: &Mat34, distance: f32) -> Mat34 {
    let mut out = *camera;
    for row in 0..3 {
        out[row * 4 + 3] += camera[row * 4] * distance;
    }
    out
}

/// Two parallel eye cameras (left, right) `separation` metres apart around a centre camera.
/// Refuses non-finite input and a view that is not the camera's inverse, so a half-written game
/// camera never becomes an eye.
pub fn parallel_eyes(view: &Mat34, camera: &Mat34, separation: f32) -> Option<[Mat34; 2]> {
    if !separation.is_finite() || separation <= 0.0 || !all_finite(view) || !all_finite(camera) || !mutually_inverse(view, camera, 0.001) {
        return None;
    }
    let eyes = [offset_right(camera, -0.5 * separation), offset_right(camera, 0.5 * separation)];
    eyes.iter().all(|eye| all_finite(eye)).then_some(eyes)
}

/// A head's yaw (radians, positive turning left about +Y) and pitch (radians, positive up). The
/// yaw comes from the head's right axis, which stays usable when looking straight up or down.
pub fn head_yaw_pitch(q: Quat) -> (f32, f32) {
    let [x, y, z, w] = q;
    let yaw = (-2.0 * (x * z - y * w)).atan2(1.0 - 2.0 * (y * y + z * z));
    let pitch = (2.0 * (w * x - y * z)).clamp(-1.0, 1.0).asin();
    (yaw, pitch)
}

/// Where something points (its forward, -Z of `q`) as yaw (radians, positive turning left about
/// +Y) and pitch (radians, positive up): the same angles as [`head_yaw_pitch`] for a level head,
/// but a hand's wrist roll does not change them.
pub fn pointing_yaw_pitch(q: Quat) -> (f32, f32) {
    let [x, y, z, w] = q;
    let forward = [-2.0 * (x * z + w * y), -2.0 * (y * z - w * x), -(1.0 - 2.0 * (x * x + y * y))];
    (((-forward[0]).atan2(-forward[2])), forward[1].clamp(-1.0, 1.0).asin())
}

/// A direction as yaw (radians, positive turning left about +Y) and pitch (radians, positive up),
/// the angles [`pointing_yaw_pitch`] gives for an orientation pointing along it.
pub fn direction_yaw_pitch(v: Vec3) -> Option<(f32, f32)> {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (length.is_finite() && length > 1e-6).then(|| ((-v[0]).atan2(-v[2]), (v[1] / length).clamp(-1.0, 1.0).asin()))
}

/// The camera with its pitch and roll removed (world up is +Y), same position: the view turns
/// with the game but only the head looks up and down. The right axis gives the yaw at any pitch.
pub fn level(camera: &Mat34) -> Option<Mat34> {
    let (rx, rz) = (camera[0], camera[8]);
    let n = (rx * rx + rz * rz).sqrt();
    if !n.is_finite() || n < 1e-4 {
        return None;
    }
    let (rx, rz) = (rx / n, rz / n);
    Some([rx, 0.0, -rz, camera[3], 0.0, 1.0, 0.0, camera[7], rz, 0.0, rx, camera[11]])
}

/// The camera turned about world +Y by `angle` radians (positive left), same position.
pub fn turn(camera: &Mat34, angle: f32) -> Mat34 {
    let (c, s) = (angle.cos(), angle.sin());
    let mut out = *camera;
    for col in 0..3 {
        let (x, z) = (camera[col], camera[8 + col]);
        out[col] = c * x + s * z;
        out[8 + col] = -s * x + c * z;
    }
    out
}

/// Where the headset's tracking space sits in the world for a game camera that carries `baked_yaw`
/// radians of the head's turn (head aim fed to the game): the camera levelled, that yaw turned back
/// out. The eyes are this with the head pose applied; any tracked pose lands in the world through it.
pub fn tracking_origin(camera: &Mat34, baked_yaw: f32) -> Option<Mat34> {
    Some(turn(&level(camera)?, -baked_yaw))
}

/// A camera-to-world matrix (columns right, up, back, position) from its back axis and a rough up,
/// made orthonormal: what `FromForwardUpPos`-style setters are given (their "forward" is the back
/// axis). `None` for degenerate axes.
pub fn from_back_up(back: Vec3, up: Vec3, position: Vec3) -> Option<Mat34> {
    let unit = |v: Vec3| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        (l > 1e-6 && l.is_finite()).then(|| v.map(|c| c / l))
    };
    let cross = |a: Vec3, b: Vec3| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let z = unit(back)?;
    let x = unit(cross(up, z))?;
    let y = cross(z, x);
    Some([x[0], y[0], z[0], position[0], x[1], y[1], z[1], position[1], x[2], y[2], z[2], position[2]])
}

/// Head aim on a game camera: its yaw and pitch kept, the head's yaw, pitch and roll added on top
/// (yaw about world up, pitch about the turned right axis, then roll), same position. For a
/// camera the game rebuilds from its own angles every frame, so nothing accumulates. `None` for
/// a camera looking straight up or down (no yaw to keep).
pub fn add_head_aim(camera: &Mat34, orientation: Quat) -> Option<Mat34> {
    swap_head_aim(camera, 0.0, 0.0, orientation)
}

/// [`add_head_aim`] for a game camera that already carries `baked_yaw` and `baked_pitch` radians
/// of the head's turn (head aim fed to the game as input): that share is taken out first, so the
/// view follows the current head exactly while the game's aim follows it a frame or two later.
pub fn swap_head_aim(camera: &Mat34, baked_yaw: f32, baked_pitch: f32, orientation: Quat) -> Option<Mat34> {
    swap_head(camera, baked_yaw, baked_pitch, orientation, [0.0; 3])
}

/// [`swap_head_aim`] with the head's position as well: `position` (metres) taken in the frame
/// turned by the yaw only, as [`apply_head`] takes a head's position in the frame it turns.
pub fn swap_head(camera: &Mat34, baked_yaw: f32, baked_pitch: f32, orientation: Quat, position: Vec3) -> Option<Mat34> {
    let pitch = (-camera[6]).clamp(-1.0, 1.0).asin() - baked_pitch;
    let (head_yaw, head_pitch) = pointing_yaw_pitch(orientation);
    let r = rotation(orientation);
    let roll = r[3].atan2(r[4]);
    let yawed = turn(&level(camera)?, head_yaw - baked_yaw);
    let (a, c1) = ((pitch + head_pitch) * 0.5).sin_cos();
    let (b, c2) = (roll * 0.5).sin_cos();
    // Pitch about the yawed +X, then roll about the pitched -Z axis.
    let q = [c2 * a, -a * b, c1 * b, c1 * c2];
    Some(apply_head(&yawed, q, position))
}

/// How a game camera is taken before the head pose goes on it, for [`view_centre`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Base {
    /// As the game set it: the head turns and moves it in its own frame, the game's pitch kept.
    Game,
    /// Levelled, so only the head looks up and down, with `baked_yaw` radians of the head's turn the
    /// game carries (head aim fed to it as input) turned back out: [`tracking_origin`].
    Level { baked_yaw: f32 },
    /// The game's own yaw and pitch kept, the head's share it carries (`baked_yaw`, `baked_pitch`
    /// radians) swapped for the head's current turn: [`swap_head`].
    Swap { baked_yaw: f32, baked_pitch: f32 },
}

/// The view centre (between the eyes) for the game camera `game` taken as `base`, the head turned
/// by `orientation` and at `position` (metres, as the head model places it:
/// [`HeadPosition::position`]).
pub fn view_centre(game: &Mat34, base: Base, orientation: Quat, position: Vec3) -> Option<Mat34> {
    match base {
        Base::Game => Some(apply_head(game, orientation, position)),
        Base::Level { baked_yaw } => Some(apply_head(&tracking_origin(game, baked_yaw)?, orientation, position)),
        Base::Swap { baked_yaw, baked_pitch } => swap_head(game, baked_yaw, baked_pitch, orientation, position),
    }
}

/// Eye `eye`'s camera (0 left, 1 right) of the view centre `centre`, the eyes `separation` metres
/// apart along its right axis.
pub fn eye_camera(centre: &Mat34, eye: usize, separation: f32) -> Mat34 {
    let side = if eye == 0 { -0.5 } else { 0.5 };
    offset_right(centre, side * separation)
}

/// Where the headset's tracking space sits in the world for a view drawn at `drawn` (a view centre
/// or an eye, camera-to-world) from the tracked head `orientation` and `position` (as tracked,
/// before a head model or an eye shift moved the view): the drawn view with the real head taken
/// back out. A controller's pose put through it lands where the drawn eyes see it, so the hands
/// cannot drift from the view whatever moved it.
pub fn tracking_space(drawn: &Mat34, orientation: Quat, position: Vec3) -> Mat34 {
    compose(drawn, &rigid_inverse(&apply_head(&IDENTITY_MATRIX, orientation, position)))
}

/// The identity camera: at the origin, looking down -Z.
pub const IDENTITY_MATRIX: Mat34 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// Whether `camera` is finite and its position within `reach` (metres) of `reference`'s: a camera
/// worth writing for a game that looks things up near its camera (one far out or not finite can
/// crash it).
pub fn within_reach(camera: &Mat34, reference: &Mat34, reach: f32) -> bool {
    let apart = [camera[3] - reference[3], camera[7] - reference[7], camera[11] - reference[11]];
    all_finite(camera) && apart.iter().map(|v| v * v).sum::<f32>().sqrt() < reach
}

/// Where the forward axis of camera `aim` lands in the image of camera `view`, in normalised
/// device coordinates (x right, y up), given the view's projection scales P00 and P11 of a
/// centred projection. `None` when it points away from the view.
pub fn aim_in_view(aim: &Mat34, view: &Mat34, scale: [f32; 2]) -> Option<[f32; 2]> {
    let forward = [-aim[2], -aim[6], -aim[10]];
    let mut d = [0.0f32; 3];
    for (axis, out) in d.iter_mut().enumerate() {
        for (k, f) in forward.iter().enumerate() {
            *out += view[k * 4 + axis] * f;
        }
    }
    if d[2] >= -0.05 {
        return None;
    }
    let ndc = [scale[0] * d[0] / -d[2], scale[1] * d[1] / -d[2]];
    all_finite(&ndc).then_some(ndc)
}

/// The affine product `a * b` (apply `b`, then `a`); neither needs to be rigid.
pub fn compose(a: &Mat34, b: &Mat34) -> Mat34 {
    let mut out = [0.0; 12];
    for row in 0..3 {
        for col in 0..4 {
            let mut sum = (0..3).map(|k| a[row * 4 + k] * b[k * 4 + col]).sum::<f32>();
            if col == 3 {
                sum += a[row * 4 + 3];
            }
            out[row * 4 + col] = sum;
        }
    }
    out
}

/// A pure translation.
pub fn translation(offset: Vec3) -> Mat34 {
    [1.0, 0.0, 0.0, offset[0], 0.0, 1.0, 0.0, offset[1], 0.0, 0.0, 1.0, offset[2]]
}

/// Largest absolute difference between two matrices.
pub fn distance(a: &Mat34, b: &Mat34) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// Tangents of a field of view's edges (left and down negative for a view around straight ahead).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    pub left: f32,
    pub right: f32,
    pub up: f32,
    pub down: f32,
}

impl Frustum {
    /// From OpenXR angles (left, right, up, down; radians). `None` when degenerate.
    pub fn from_fov(fov: [f32; 4]) -> Option<Self> {
        let frustum = Self { left: fov[0].tan(), right: fov[1].tan(), up: fov[2].tan(), down: fov[3].tan() };
        frustum.usable().then_some(frustum)
    }

    /// A centred frustum of the given half-tangents (`[horizontal, vertical]`). `None` when degenerate.
    pub fn centred(half: [f32; 2]) -> Option<Self> {
        let frustum = Self { left: -half[0], right: half[0], up: half[1], down: -half[1] };
        frustum.usable().then_some(frustum)
    }

    /// From near-plane extents (left, right, bottom, top) at distance `near`.
    pub fn from_extents(extents: [f32; 4], near: f32) -> Option<Self> {
        if !(near.is_finite() && near > 0.0) {
            return None;
        }
        let frustum = Self { left: extents[0] / near, right: extents[1] / near, up: extents[3] / near, down: extents[2] / near };
        frustum.usable().then_some(frustum)
    }

    fn usable(&self) -> bool {
        all_finite(&[self.left, self.right, self.up, self.down]) && self.right - self.left > 0.1 && self.up - self.down > 0.1
    }

    /// OpenXR angles (left, right, up, down; radians).
    pub fn fov(&self) -> [f32; 4] {
        [self.left.atan(), self.right.atan(), self.up.atan(), self.down.atan()]
    }

    /// The smallest frustum containing both eyes' fields (OpenXR angles, left eye first): what an
    /// engine culls with when it culls once for the pair.
    pub fn both_eyes(fov: &[[f32; 4]; 2]) -> Option<Self> {
        Some(Self::from_fov(fov[0])?.union(Self::from_fov(fov[1])?))
    }

    /// The smallest frustum containing both.
    pub fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            right: self.right.max(other.right),
            up: self.up.max(other.up),
            down: self.down.min(other.down),
        }
    }

    /// Writes the x/y terms of a canonical (row-major, right-handed) perspective projection; the
    /// depth terms stay as the game set them. Other layouts convert through
    /// [`crate::convention::ProjectionConvention`].
    pub fn apply_to_projection(&self, projection: &mut Mat44) {
        let (w, h) = (self.right - self.left, self.up - self.down);
        projection[0] = 2.0 / w;
        projection[2] = (self.right + self.left) / w;
        projection[5] = 2.0 / h;
        projection[6] = (self.up + self.down) / h;
    }

    /// Near-plane extents (left, right, bottom, top) at distance `near`.
    pub fn extents(&self, near: f32) -> [f32; 4] {
        [near * self.left, near * self.right, near * self.down, near * self.up]
    }

    /// Where straight ahead lands in this frustum's clip space (x right, y up).
    pub fn straight_ahead(&self) -> [f32; 2] {
        [-(self.right + self.left) / (self.right - self.left), -(self.up + self.down) / (self.up - self.down)]
    }

    /// The largest edge tangent horizontally and vertically.
    pub fn half_extent(&self) -> [f32; 2] {
        [self.left.abs().max(self.right.abs()), self.up.abs().max(self.down.abs())]
    }

    /// The smallest centred frustum containing this one. Some screen-space effects assume a
    /// centred projection (DL2: an off-centre one darkened screen-aligned blocks of the sun
    /// shadow); render this and report it as the eye's field of view, and the viewer shows the
    /// right part.
    pub fn symmetric(&self) -> Self {
        let [h, v] = self.half_extent();
        Self { left: -h, right: h, up: v, down: -v }
    }

    /// A render size `height` pixels tall with this frustum's aspect, width rounded to 8.
    pub fn render_size(&self, height: u32) -> (u32, u32) {
        let aspect = (self.right - self.left) / (self.up - self.down);
        let width = ((height as f32 * aspect / 8.0).round() as u32).max(1) * 8;
        (width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::FRAC_PI_6;
    use crate::math::{IDENTITY, yaw_rotation};

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
    fn view_centres_take_the_game_camera_three_ways() {
        let yaw = 30f32.to_radians();
        let game = from_back_up([yaw.sin() * 0.9, 0.43, yaw.cos() * 0.9], [0.0, 1.0, 0.0], [5.0, 1.0, 2.0]).unwrap();
        let head = half_turn(0.2, -0.1);
        let position = [0.03, -0.02, 0.05];
        assert_eq!(view_centre(&game, Base::Game, head, position), Some(apply_head(&game, head, position)));
        let levelled = view_centre(&game, Base::Level { baked_yaw: 0.1 }, head, position).unwrap();
        assert_eq!(levelled, apply_head(&tracking_origin(&game, 0.1).unwrap(), head, position));
        let swapped = view_centre(&game, Base::Swap { baked_yaw: 0.1, baked_pitch: 0.05 }, head, position).unwrap();
        let unmoved = swap_head_aim(&game, 0.1, 0.05, head).unwrap();
        // Turned as without a position, moved by it in the yawed (level) frame.
        let yawed = turn(&level(&game).unwrap(), pointing_yaw_pitch(head).0 - 0.1);
        let moved = apply_head(&yawed, IDENTITY, position);
        for row in 0..3 {
            assert_eq!(swapped[row * 4..row * 4 + 3], unmoved[row * 4..row * 4 + 3]);
            assert!(close(swapped[row * 4 + 3] - unmoved[row * 4 + 3], moved[row * 4 + 3] - yawed[row * 4 + 3]));
        }
        // The eyes straddle the centre along its right axis.
        let [left, right] = [0, 1].map(|eye| eye_camera(&levelled, eye, 0.064));
        let apart = [right[3] - left[3], right[7] - left[7], right[11] - left[11]];
        assert!(close(apart.iter().map(|v| v * v).sum::<f32>().sqrt(), 0.064) && left[0] == levelled[0]);
        assert!(close(apart[0] / 0.064, levelled[0]) && close(apart[2] / 0.064, levelled[8]), "along the right axis");
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
}