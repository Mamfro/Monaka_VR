//! Eye gaze for the game, written with each head pose ([`EyeGaze`]): from OpenXR's eye gaze
//! interaction (a pose action located in VIEW space), or, on headsets that offer only that, from
//! Meta's eye tracker (both eyes combined). Head-relative: +X right, +Y up, forward -Z.

use monaka_core::math::{self, Quat, Vec3};
use monaka_contract::protocol::EyeGaze;
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
                from_pose(pose.orientation.xyzw(), pose.position.xyz(), sampled, tick)
            }
            Source::Tracker(tracker) => {
                let (eyes, sampled) = tracker.gazes(view, time).ok()?;
                from_eyes(eyes.map(|eye| eye.map(|pose| (pose.orientation.xyzw(), pose.position.xyz()))), sampled, tick)
            }
        }
    }
}

#[cfg(test)]
mod tests;
