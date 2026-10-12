//! Camera matrices shared by the producers.
//!
//! Everything here is in the canonical form ([`crate::convention`]): a row-major 3x4
//! camera-to-world matrix (columns right, up, back, position; metres) in OpenXR axes (+X right,
//! +Y up, looking down -Z), so a head pose composes directly onto it; the view matrix is the
//! rigid inverse. Both Dying Light engines store cameras this way; other engines convert first.

use crate::math::{Quat, Vec3, cross, dot, unit};
use crate::options::Shared;
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

/// A frame's position.
pub fn position(m: &Mat34) -> Vec3 {
    [m[3], m[7], m[11]]
}

/// The unit quaternion (xyzw) of a frame's rotation: [`rotation`]'s inverse (Shepperd's method,
/// from the largest diagonal term, stable at any angle).
pub fn quaternion(m: &Mat34) -> Quat {
    let at = |i: usize, j: usize| m[i * 4 + j];
    let trace = at(0, 0) + at(1, 1) + at(2, 2);
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(at(2, 1) - at(1, 2)) / s, (at(0, 2) - at(2, 0)) / s, (at(1, 0) - at(0, 1)) / s, s / 4.0]
    } else if at(0, 0) > at(1, 1) && at(0, 0) > at(2, 2) {
        let s = (1.0 + at(0, 0) - at(1, 1) - at(2, 2)).sqrt() * 2.0;
        [s / 4.0, (at(0, 1) + at(1, 0)) / s, (at(0, 2) + at(2, 0)) / s, (at(2, 1) - at(1, 2)) / s]
    } else if at(1, 1) > at(2, 2) {
        let s = (1.0 + at(1, 1) - at(0, 0) - at(2, 2)).sqrt() * 2.0;
        [(at(0, 1) + at(1, 0)) / s, s / 4.0, (at(1, 2) + at(2, 1)) / s, (at(0, 2) - at(2, 0)) / s]
    } else {
        let s = (1.0 + at(2, 2) - at(0, 0) - at(1, 1)).sqrt() * 2.0;
        [(at(0, 2) + at(2, 0)) / s, (at(1, 2) + at(2, 1)) / s, s / 4.0, (at(1, 0) - at(0, 1)) / s]
    };
    // One of the two signs: w not negative.
    if q[3] < 0.0 { q.map(|v| -v) } else { q }
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

/// [`swap_head_aim`] with the head's position as well: `position` (metres, in the headset's
/// tracking space) taken in the body's frame ([`tracking_origin`]), not turned by the head: a lean
/// to the right goes to the body's right whichever way the head looks.
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
    let mut centre = apply_head(&yawed, q, [0.0; 3]);
    let moved = apply_head(&tracking_origin(camera, baked_yaw)?, crate::math::IDENTITY, position);
    for row in 0..3 {
        centre[row * 4 + 3] = moved[row * 4 + 3];
    }
    Some(centre)
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

impl Base {
    /// The game's camera as it set it, or with `baked` (yaw, pitch; radians) of the head's turn in
    /// it (head aim fed to the game as input) swapped for the head's current turn.
    pub fn game_or_swap(baked: Option<[f32; 2]>) -> Base {
        baked.map_or(Base::Game, |[baked_yaw, baked_pitch]| Base::Swap { baked_yaw, baked_pitch })
    }

    /// Where the headset's tracking space sits for the game camera `game` taken this way: the frame
    /// the head's position moves the view in ([`view_centre`]). The camera itself for
    /// [`Base::Game`]; levelled with the baked yaw turned back out ([`tracking_origin`]) otherwise.
    pub fn origin(self, game: &Mat34) -> Option<Mat34> {
        match self {
            Base::Game => Some(*game),
            Base::Level { baked_yaw } | Base::Swap { baked_yaw, .. } => tracking_origin(game, baked_yaw),
        }
    }
}

impl HeadPosition {
    /// The tracking origin `origin` moved by how far this head model draws the head from where
    /// `pose` tracks it: a controller's pose put through it lands where the eyes drawn from `origin`
    /// see it ([`tracking_space`]). `origin` itself without a head pose.
    pub fn drawn_origin(self, origin: &Mat34, pose: &HeadPose) -> Mat34 {
        if pose.valid == 0 {
            return *origin;
        }
        let drawn = self.position(pose);
        apply_head(origin, crate::math::IDENTITY, [0, 1, 2].map(|k| drawn[k] - pose.position[k]))
    }
}

/// One eye's view of a game camera: the eye's camera, the view centre between the eyes, and the
/// frustum the eye is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeView {
    pub camera: Mat34,
    pub centre: Mat34,
    pub frustum: Frustum,
}

impl EyeView {
    /// Eye `eye` of the game camera `game` (canonical) for the head `pose`, the camera taken as
    /// `base`: the view centre turned and moved by the head as the head model places it
    /// ([`view_centre`]; the game's camera itself without a head pose), the eye half the eye
    /// distance along its right axis ([`eye_camera`]), and its frustum (`centred`: the smallest
    /// centred one around the headset's, for engines that take only a field of view; the game's
    /// full angles `game_fov` without a headset). None when the game's camera or field of view is
    /// not a plausible one ([`plausible_camera`], [`plausible_fov`]).
    pub fn of(game: &Mat34, base: Base, pose: &HeadPose, eye: usize, shared: &Shared, centred: bool, game_fov: [f32; 2]) -> Option<Self> {
        if !plausible_camera(game) || !plausible_fov(game_fov) {
            return None;
        }
        let centre = match pose.valid != 0 {
            true => view_centre(game, base, pose.orientation, shared.head_position.position(pose))?,
            false => *game,
        };
        let camera = eye_camera(&centre, eye, shared.separation_for(pose));
        let frustum = shared.rendered_frustum(pose, eye, centred, Some(game_fov))?;
        Some(Self { camera, centre, frustum })
    }
}

/// Whether a game camera (canonical) is one to draw from: finite, orthonormal axes, and a position
/// within ten million units of the origin.
pub fn plausible_camera(m: &Mat34) -> bool {
    let axis = |k: usize| [m[k], m[4 + k], m[8 + k]];
    let (r, u, b) = (axis(0), axis(1), axis(2));
    all_finite(m)
        && [m[3], m[7], m[11]].iter().all(|v| v.abs() < 1.0e7)
        && [r, u, b].iter().all(|a| (dot(*a, *a) - 1.0).abs() < 1.0e-3)
        && dot(r, u).abs() < 1.0e-3
        && dot(r, b).abs() < 1.0e-3
        && dot(u, b).abs() < 1.0e-3
}

/// Whether full field-of-view angles (radians) are a game's own: between 5 and 175 degrees.
pub fn plausible_fov(angles: [f32; 2]) -> bool {
    angles.iter().all(|v| v.is_finite() && *v > 0.09 && *v < 3.05)
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

/// A 4x4 whose rows are the axes and then the position (row-vector maths, as a game's bind poses
/// may be kept) as a 3x4.
pub fn from_rows_4x4(m: &Mat44) -> Mat34 {
    [m[0], m[4], m[8], m[12], m[1], m[5], m[9], m[13], m[2], m[6], m[10], m[14]]
}

/// Which of `frame`'s own axes (x, y or z, either way) points nearest `direction`, as a signed
/// unit axis of the frame (a held weapon's barrel, from where the game points it).
pub fn nearest_axis(frame: &Mat34, direction: Vec3) -> Option<Vec3> {
    let (k, along) = (0..3)
        .map(|k| {
            let axis = [frame[k], frame[4 + k], frame[8 + k]];
            (k, dot(axis, direction) / dot(axis, axis).sqrt())
        })
        .filter(|(_, c)| c.is_finite())
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
    let mut axis = [0.0; 3];
    axis[k] = along.signum();
    Some(axis)
}

/// A pure translation.
pub fn translation(offset: Vec3) -> Mat34 {
    [1.0, 0.0, 0.0, offset[0], 0.0, 1.0, 0.0, offset[1], 0.0, 0.0, 1.0, offset[2]]
}

/// `v` through `m`'s 3x3 part (a direction or an offset, no translation).
pub fn apply_3x3(m: &Mat34, v: Vec3) -> Vec3 {
    [0, 1, 2].map(|r| m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2])
}

/// `v` taken back through `m`'s 3x3 part, which may scale or mirror (a model's bones can):
/// [`apply_3x3`] of the result gives `v` again. None when the part is singular.
pub fn unapply_3x3(m: &Mat34, v: Vec3) -> Option<Vec3> {
    let rows = [0, 1, 2].map(|r| [m[r * 4], m[r * 4 + 1], m[r * 4 + 2]]);
    // The inverse's columns are the rows' pairwise crosses over the determinant.
    let columns = [cross(rows[1], rows[2]), cross(rows[2], rows[0]), cross(rows[0], rows[1])];
    let det = dot(rows[0], columns[0]);
    (det.is_finite() && det.abs() > 1e-9).then(|| [0, 1, 2].map(|i| (columns[0][i] * v[0] + columns[1][i] * v[1] + columns[2][i] * v[2]) / det))
}

/// The inverse of an affine `m` whose 3x3 part may scale or mirror (a game's posed bones can); none
/// when it is singular.
pub fn inverse(m: &Mat34) -> Option<Mat34> {
    let columns = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|axis| unapply_3x3(m, axis));
    let [Some(x), Some(y), Some(z)] = columns else { return None };
    let t = unapply_3x3(m, position(m))?;
    Some([x[0], y[0], z[0], -t[0], x[1], y[1], z[1], -t[1], x[2], y[2], z[2], -t[2]])
}

/// `m`'s axes (its columns) made a rotation, keeping its x axis and the plane of x and y: a game
/// matrix that carries a scale or a squash, as a rigid frame.
pub fn rigid_axes(m: &Mat34) -> [Vec3; 3] {
    let unit = |v: Vec3| unit(v).unwrap_or(v);
    let x = unit([m[0], m[4], m[8]]);
    let z = unit(cross(x, [m[1], m[5], m[9]]));
    [x, cross(z, x), z]
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
        let shared = Shared::from_options(&crate::options::Options::parse("eye_separation=0.064"), &crate::manifest::MONAKA);
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
        assert_eq!(view_centre(&game, Base::Game, head, position), Some(apply_head(&game, head, position)));
        let levelled = view_centre(&game, Base::Level { baked_yaw: 0.1 }, head, position).unwrap();
        assert_eq!(levelled, apply_head(&tracking_origin(&game, 0.1).unwrap(), head, position));
        let swapped = view_centre(&game, Base::Swap { baked_yaw: 0.1, baked_pitch: 0.05 }, head, position).unwrap();
        let unmoved = swap_head_aim(&game, 0.1, 0.05, head).unwrap();
        // Turned as without a position, moved by it in the body's frame (the head's turn left out).
        let body = tracking_origin(&game, 0.1).unwrap();
        let moved = apply_head(&body, IDENTITY, position);
        for row in 0..3 {
            assert_eq!(swapped[row * 4..row * 4 + 3], unmoved[row * 4..row * 4 + 3]);
            assert!(close(swapped[row * 4 + 3] - unmoved[row * 4 + 3], moved[row * 4 + 3] - body[row * 4 + 3]));
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
        assert_eq!(Base::Level { baked_yaw: 0.1 }.origin(&game), tracking_origin(&game, 0.1));
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
}