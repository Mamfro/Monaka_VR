//! Head and eye geometry in OpenXR conventions: +X right, +Y up, looking down -Z, quaternions
//! stored xyzw.

use monaka_contract::protocol::{EyeGaze, HeadPose};

pub type Quat = [f32; 4];
pub type Vec3 = [f32; 3];

pub const IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

/// Heading of an orientation around +Y in radians; 0 looks down -Z, positive turns left.
pub fn yaw(q: Quat) -> f32 {
    let [x, y, z, w] = q;
    let forward_x = -2.0 * (x * z + y * w);
    let forward_z = -(1.0 - 2.0 * (x * x + y * y));
    (-forward_x).atan2(-forward_z)
}

/// Pure rotation around +Y.
pub fn yaw_rotation(yaw: f32) -> Quat {
    let half = yaw * 0.5;
    [0.0, half.sin(), 0.0, half.cos()]
}

/// The orientation's +X axis.
pub fn right_axis(q: Quat) -> Vec3 {
    let [x, y, z, w] = q;
    [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)]
}

/// The orientation's forward (-Z) axis.
pub fn forward_axis(q: Quat) -> Vec3 {
    let [x, y, z, w] = q;
    [-2.0 * (x * z + w * y), -2.0 * (y * z - w * x), -(1.0 - 2.0 * (x * x + y * y))]
}

/// A point `distance` metres from `from` along where `pointer` faces, oriented like it (a quad
/// there faces back along the ray). For a hand-aim reticle: from the head, where the game's
/// aim ray starts, along the controller's direction.
pub fn along(from: Vec3, pointer: Quat, distance: f32) -> Vec3 {
    let f = forward_axis(pointer);
    [from[0] + f[0] * distance, from[1] + f[1] * distance, from[2] + f[2] * distance]
}

/// How far ahead a hand-held weapon's line and the game's aim ray meet (metres). The game shoots
/// from its camera, the player's head; the weapon is held at the hand, a forearm away. Aiming the
/// camera at this point on the weapon's line puts the shots, the weapon's sights and the
/// reticle together there, and close to each other beyond.
pub const WEAPON_CONVERGENCE: f32 = 10.0;

/// The point [`WEAPON_CONVERGENCE`] metres along a weapon held at `palm` pointing like `pointer`.
pub fn converged(palm: Vec3, pointer: Quat) -> Vec3 {
    along(palm, pointer, WEAPON_CONVERGENCE)
}

/// Where a hand's aim reticle shows, for a controller at `palm` pointing like `aim`: with
/// `from_palm`, [`WEAPON_CONVERGENCE`] metres along `said` (a direction in the palm's frame the
/// producer gives, a bow held at an angle) or along the aim itself; else [`WEAPON_CONVERGENCE`]
/// metres along the aim from `head`, where the game's aim ray starts.
pub fn reticle_point(head: Vec3, aim: Quat, palm: Vec3, from_palm: bool, said: Option<Vec3>) -> Vec3 {
    match (from_palm, said) {
        (true, Some(direction)) => {
            let d = rotate(aim, direction);
            let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
            [0, 1, 2].map(|k| palm[k] + d[k] / length * WEAPON_CONVERGENCE)
        }
        (true, None) => converged(palm, aim),
        (false, _) => along(head, aim, WEAPON_CONVERGENCE),
    }
}

/// `v` turned by `q`.
pub fn rotate(q: Quat, v: Vec3) -> Vec3 {
    let [x, y, z, w] = q;
    // v + 2w(q x v) + 2 q x (q x v)
    let t = [2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0])];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

/// Where the eyes sit relative to the neck they turn about (the usual neck model: 7.5 cm up,
/// 8 cm forward).
pub const NECK_TO_EYES: Vec3 = [0.0, 0.075, -0.0805];

/// The head's position without the movement its own turning causes: where the eyes would be if
/// only the neck (the body) moved. Nodding and turning leave it in place; leaning still moves it.
/// For views tied to a game camera that pivots at the eyes (weapon sights stay lined up).
pub fn lean_only(orientation: Quat, position: Vec3) -> Vec3 {
    let turned = rotate(orientation, NECK_TO_EYES);
    [0, 1, 2].map(|k| position[k] - (turned[k] - NECK_TO_EYES[k]))
}

pub fn midpoint(a: Vec3, b: Vec3) -> Vec3 {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5]
}

pub fn distance(a: Vec3, b: Vec3) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
}

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// How far along the segment from `a` to `b` point `p` sits (its nearest point on the line: 0 at
/// `a`, 1 at `b`, beyond either outside them); none for a segment shorter than a millimetre.
pub fn segment_share(p: Vec3, a: Vec3, b: Vec3) -> Option<f32> {
    let d = sub(b, a);
    let length2 = dot(d, d);
    (length2 > 1e-6).then(|| dot(sub(p, a), d) / length2).filter(|t| t.is_finite())
}

/// `a` made one long; none when it is too short to have a direction (1e-6) or not finite.
pub fn unit(a: Vec3) -> Option<Vec3> {
    let l = length(a);
    (l.is_finite() && l > 1e-6).then(|| scale(a, 1.0 / l))
}

/// `v` turned by the rotation that takes unit `from` to unit `to` (about the axis square to both);
/// `v` as it is where they point the same way or exactly opposite.
pub fn turned(v: Vec3, from: Vec3, to: Vec3) -> Vec3 {
    let axis = cross(from, to);
    let (sin, cos) = (length(axis), dot(from, to));
    let Some(k) = unit(axis).filter(|_| sin > 1e-6) else { return v };
    let kv = cross(k, v);
    let kd = dot(k, v) * (1.0 - cos);
    [0, 1, 2].map(|i| v[i] * cos + kv[i] * sin + k[i] * kd)
}

/// The inverse turn of a unit quaternion.
pub fn conjugate(q: Quat) -> Quat {
    [-q[0], -q[1], -q[2], q[3]]
}

/// `a` then `b` in the frame `a` gives (Hamilton product `a * b`).
pub fn multiply(a: Quat, b: Quat) -> Quat {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx, aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz]
}

/// The dot product of two quaternions: the cosine of half the angle between them.
pub fn quat_dot(a: Quat, b: Quat) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// `q` scaled to unit length (the identity for a zero quaternion).
pub fn quat_unit(q: Quat) -> Quat {
    let length = quat_dot(q, q).sqrt();
    if length > 0.0 { q.map(|v| v / length) } else { IDENTITY }
}

/// The angle (radians) turning unit quaternion `a` into `b`.
pub fn angle_between(a: Quat, b: Quat) -> f32 {
    2.0 * quat_dot(a, b).abs().min(1.0).acos()
}

/// `q`, or its negation (the same rotation) on `reference`'s side, so blending takes the short way.
pub fn same_side(reference: Quat, q: Quat) -> Quat {
    if quat_dot(reference, q) < 0.0 { q.map(|v| -v) } else { q }
}

/// The rotation taking x, y and z to the orthonormal axes `x`, `y`, `z` (right-handed).
pub fn from_axes(x: Vec3, y: Vec3, z: Vec3) -> Quat {
    // Rows of the matrix whose columns are the axes.
    let m = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
    let trace = m[0][0] + m[1][1] + m[2][2];
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, 0.25 * s]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [(m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [(m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s, (m[1][0] - m[0][1]) / s]
    }
}

/// A head pose from the two located eye views: orientation from the left view, position halfway
/// between the eyes, IPD as the distance between them.
pub fn head_from_views(orientation: Quat, eye_positions: [Vec3; 2], fov: [[f32; 4]; 2]) -> HeadPose {
    HeadPose {
        orientation,
        position: midpoint(eye_positions[0], eye_positions[1]),
        ipd: distance(eye_positions[0], eye_positions[1]),
        fov,
        valid: 1,
        gaze: EyeGaze::default(),
    }
}

/// Where one eye of a rendered head pose sits: half the IPD along the head's right axis
/// (eye 0 left, eye 1 right).
pub fn eye_position(pose: &HeadPose, eye: usize) -> Vec3 {
    let half = if eye == 0 { -0.5 } else { 0.5 } * pose.ipd;
    let right = right_axis(pose.orientation);
    [pose.position[0] + right[0] * half, pose.position[1] + right[1] * half, pose.position[2] + right[2] * half]
}

/// A pose `distance` metres ahead of the head along its heading, level and facing back at it.
/// Used to anchor the flat menu screen in the room.
pub fn ahead_of(head_orientation: Quat, head_position: Vec3, distance: f32) -> (Quat, Vec3) {
    let heading = yaw(head_orientation);
    let position = [
        head_position[0] - distance * heading.sin(),
        head_position[1],
        head_position[2] - distance * heading.cos(),
    ];
    (yaw_rotation(heading), position)
}

/// `angle` (radians) wrapped to -pi..pi.
pub fn wrap_radians(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// `angle` (degrees) wrapped to -180..180.
pub fn wrap_degrees(angle: f32) -> f32 {
    (angle + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests;
