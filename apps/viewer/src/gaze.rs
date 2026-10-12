//! Eye gaze for the game, written with each head pose ([`EyeGaze`]): from OpenXR's eye gaze
//! interaction (a pose action located in VIEW space), or, on headsets that offer only that, from
//! Meta's eye tracker (both eyes combined). Head-relative: +X right, +Y up, forward -Z.

use monaka_core::math::{self, Quat, Vec3};
use monaka_core::protocol::EyeGaze;
use monaka_openxr::{Action, EyeTracker, Session, Space, Time};

/// The eye gaze interaction profile and its one pose.
pub const PROFILE: &str = "/interaction_profiles/ext/eye_gaze_interaction";
pub const BINDING: &str = "/user/eyes_ext/input/gaze_ext/pose";

/// A gaze from a pose in VIEW space: the direction is the pose's forward (-Z), the origin its
/// position. `None` for a pose without a finite direction.
pub fn from_pose(orientation: Quat, position: Vec3, sample_time: Time, received_tick: u64) -> Option<EyeGaze> {
    let direction = math::unit(math::forward_axis(orientation))?;
    Some(EyeGaze { direction, origin: position, sample_time, received_tick, valid: 1 })
}

/// One gaze from each eye's (left, right), either alone standing in for both: the mean of the
/// directions, from between the eyes.
pub fn from_eyes(eyes: [Option<(Quat, Vec3)>; 2], sample_time: Time, received_tick: u64) -> Option<EyeGaze> {
    let gazes: Vec<EyeGaze> = eyes.into_iter().flatten().filter_map(|(q, p)| from_pose(q, p, sample_time, received_tick)).collect();
    match gazes.as_slice() {
        [one] => Some(*one),
        [left, right] => {
            let direction = math::unit(math::add(left.direction, right.direction))?;
            Some(EyeGaze { direction, origin: math::midpoint(left.origin, right.origin), ..*left })
        }
        _ => None,
    }
}

/// Where the gaze comes from in a running session.
pub enum Source<'s> {
    /// The gaze pose action (synced with the controllers' action set) and its space.
    Action(&'s Action<'s>, Space<'s>),
    /// Meta's eye tracker.
    Tracker(EyeTracker<'s>),
}

impl Source<'_> {
    /// The gaze at `time` relative to `view` (the session's VIEW space), if the eyes are tracked;
    /// the action set must have been synced this frame.
    pub fn read(&self, session: &Session<'_>, view: &Space<'_>, time: Time, tick: u64) -> Option<EyeGaze> {
        match self {
            Source::Action(action, space) => {
                if !session.pose_active(action, monaka_openxr::sys::NULL_HANDLE).unwrap_or(false) {
                    return None;
                }
                let (pose, sampled) = session.locate_gaze(space, view, time).ok().flatten()?;
                from_pose(quat(pose), vec3(pose), sampled, tick)
            }
            Source::Tracker(tracker) => {
                let (eyes, sampled) = tracker.gazes(view, time).ok()?;
                from_eyes(eyes.map(|eye| eye.map(|pose| (quat(pose), vec3(pose)))), sampled, tick)
            }
        }
    }
}

fn quat(pose: monaka_openxr::Posef) -> Quat {
    let q = pose.orientation;
    [q.x, q.y, q.z, q.w]
}

fn vec3(pose: monaka_openxr::Posef) -> Vec3 {
    let p = pose.position;
    [p.x, p.y, p.z]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    /// A turn of `degrees` about unit axis `axis`.
    fn turn(axis: Vec3, degrees: f32) -> Quat {
        let (s, c) = (degrees.to_radians() * 0.5).sin_cos();
        [axis[0] * s, axis[1] * s, axis[2] * s, c]
    }

    #[test]
    fn a_pose_looks_along_its_forward() {
        let ahead = from_pose(math::IDENTITY, [0.0, 0.0, 0.0], 77, 5).unwrap();
        assert_eq!(ahead, EyeGaze { direction: [0.0, 0.0, -1.0], origin: [0.0; 3], sample_time: 77, received_tick: 5, valid: 1 });
        // Looking 90 degrees to the left (a turn about +Y): -X in VIEW space.
        let left = from_pose(turn([0.0, 1.0, 0.0], 90.0), [0.01, 0.0, -0.02], 0, 0).unwrap();
        assert!(close(left.direction, [-1.0, 0.0, 0.0]), "{:?}", left.direction);
        assert_eq!(left.origin, [0.01, 0.0, -0.02]);
        // Looking 30 degrees down (a turn about +X, negative).
        let down = from_pose(turn([1.0, 0.0, 0.0], -30.0), [0.0; 3], 0, 0).unwrap();
        let (s, c) = 30f32.to_radians().sin_cos();
        assert!(close(down.direction, [0.0, -s, -c]), "{:?}", down.direction);
        assert!(from_pose([f32::NAN, 0.0, 0.0, 1.0], [0.0; 3], 0, 0).is_none(), "no finite direction");
    }

    #[test]
    fn two_eyes_meet_between_them() {
        let ipd = 0.064;
        // Each eye turned 5 degrees inward, converging ahead.
        let left = (turn([0.0, 1.0, 0.0], -5.0), [-ipd / 2.0, 0.0, 0.0]);
        let right = (turn([0.0, 1.0, 0.0], 5.0), [ipd / 2.0, 0.0, 0.0]);
        let both = from_eyes([Some(left), Some(right)], 9, 3).unwrap();
        assert!(close(both.direction, [0.0, 0.0, -1.0]), "{:?}", both.direction);
        assert!(close(both.origin, [0.0; 3]));
        assert_eq!((both.sample_time, both.received_tick, both.valid), (9, 3, 1));
        // One eye alone stands in for both.
        let one = from_eyes([None, Some(right)], 9, 3).unwrap();
        assert_eq!(one, from_pose(right.0, right.1, 9, 3).unwrap());
        assert!(from_eyes([None, None], 9, 3).is_none());
    }
}
