//! Head and eye geometry in OpenXR conventions: +X right, +Y up, looking down -Z, quaternions
//! stored xyzw.

use crate::protocol::{EyeGaze, HeadPose};

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
mod tests {
    #[test]
    fn lean_only_removes_the_necks_turning() {
        // Nodding 30 degrees down about the neck moves the eyes forward and down.
        let half = (-30f32).to_radians() * 0.5;
        let q = [half.sin(), 0.0, 0.0, half.cos()];
        let turned = rotate(q, NECK_TO_EYES);
        let nodded = [0, 1, 2].map(|k| turned[k] - NECK_TO_EYES[k]);
        assert!(nodded[2] < -0.01 && nodded[1] < 0.0, "forward and down: {nodded:?}");
        let lean = lean_only(q, nodded);
        assert!(lean.iter().all(|v| v.abs() < 1e-5), "{lean:?}");
        // A lean with the head level is kept.
        assert_eq!(lean_only(IDENTITY, [0.1, 0.0, -0.05]), [0.1, 0.0, -0.05]);
        // rotate agrees with the rotation matrix.
        let r = crate::camera::rotation(q);
        let v = [0.3, -0.2, 0.5];
        let m = [0, 1, 2].map(|i| r[3 * i] * v[0] + r[3 * i + 1] * v[1] + r[3 * i + 2] * v[2]);
        assert!(rotate(q, v).iter().zip(m).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    use super::*;

    #[test]
    fn angles_wrap_to_one_turn() {
        assert!((wrap_radians(3.0 * std::f32::consts::PI) - std::f32::consts::PI).abs() < 1e-5 || (wrap_radians(3.0 * std::f32::consts::PI) + std::f32::consts::PI).abs() < 1e-5);
        assert!((wrap_radians(-0.5) + 0.5).abs() < 1e-6);
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(45.0), 45.0);
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn along_follows_the_pointer() {
        assert_eq!(forward_axis(IDENTITY), [0.0, 0.0, -1.0]);
        // Turned 90 degrees left, forward is -X.
        let p = along([0.0, 1.6, 0.0], yaw_rotation(core::f32::consts::FRAC_PI_2), 10.0);
        assert!(close(p[0], -10.0) && close(p[1], 1.6) && p[2].abs() < 1e-5, "{p:?}");
    }

    #[test]
    fn yaw_round_trips() {
        assert!(close(yaw(IDENTITY), 0.0));
        let left = yaw_rotation(core::f32::consts::FRAC_PI_2);
        assert!(close(yaw(left), core::f32::consts::FRAC_PI_2));
    }

    #[test]
    fn eyes_straddle_the_head() {
        let pose = HeadPose { ipd: 0.064, valid: 1, ..HeadPose::default() };
        assert!(close(eye_position(&pose, 0)[0], -0.032));
        assert!(close(eye_position(&pose, 1)[0], 0.032));
        // Turned 90 degrees left (looking down -X), the right eye moves toward -Z.
        let turned = HeadPose { orientation: yaw_rotation(core::f32::consts::FRAC_PI_2), ..pose };
        let right = eye_position(&turned, 1);
        assert!(close(right[0], 0.0) && close(right[2], -0.032));
    }

    #[test]
    fn head_from_views_measures_ipd() {
        let head = head_from_views(IDENTITY, [[-0.03, 1.6, 0.0], [0.03, 1.6, 0.0]], [[0.0; 4]; 2]);
        assert!(close(head.ipd, 0.06));
        assert!(close(head.position[1], 1.6) && close(head.position[0], 0.0));
    }

    #[test]
    fn screen_anchors_ahead() {
        let (_, ahead) = ahead_of(IDENTITY, [0.0, 1.6, 0.0], 2.0);
        assert!(close(ahead[2], -2.0) && close(ahead[1], 1.6));
        let (rotation, left) = ahead_of(yaw_rotation(core::f32::consts::FRAC_PI_2), [0.0; 3], 2.0);
        assert!(close(left[0], -2.0) && close(left[2], 0.0));
        assert!(close(yaw(rotation), core::f32::consts::FRAC_PI_2));
    }
}
