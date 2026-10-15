//! Where the hands are, for every ready-made that reads them: one question with three answers
//! ([`pose`]): which point of the hand ([`Point`]), at which moment ([`At`]) and in which space
//! ([`In`]); and the hand rig's trail for speeds and history ([`HandRig::trail`]). Where the
//! game's own hand is after the rig and its animation is the hand rig's
//! ([`crate::hand_rig::posed_hand`]).
//!
//! The moment: the latest the viewer located the controllers, or the sample the frame being
//! presented was built with, which the present loop's step takes ([`sample_frame`]). The space:
//! the headset's tracking space, or the game's world, as the palms were noted on the trail through
//! the tracking origin the view is drawn from.

use crate::hand_rig::HandRig;
use monaka_core::camera::{Mat34, position, quaternion};
use monaka_core::math::Vec3;
use monaka_contract::protocol::HandPose;
use monaka_core::sync::Locked;
use std::sync::Mutex;

/// Which point of a hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    /// The controller's aim pose.
    Controller,
    /// The palm: the runtime's grip position with the aim's direction ([`HandPose::palm`]).
    Palm,
    /// Where the hand holds a bow's string: [`STRING_ON_PALM`] in the palm's frame.
    StringPoint,
}

/// At which moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    /// As the viewer located the controller last (within [`crate::controllers`]' age).
    Latest,
    /// As sampled with the frame being presented ([`sample_frame`]).
    Frame,
}

/// In which space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum In {
    /// The headset's tracking space.
    Tracking,
    /// The game's world: the palm as last noted on the hand rig's trail ([`HandRig::trail`]),
    /// within [`FRESH`] seconds. The trail notes palms at the arms' update, so only the palm and
    /// points on it, at their latest, are in the world.
    World,
}

/// The string point in the palm's frame (metres): the palm's own centre.
pub const STRING_ON_PALM: Vec3 = [0.0; 3];

/// How recently a palm on the trail must have been noted to stand for its hand (seconds).
pub const FRESH: f64 = 0.1;

/// The controllers as sampled with the frame being presented: each aim pose and grip pose.
struct Frame {
    aims: [Option<HandPose>; 2],
    grips: [Option<HandPose>; 2],
}

static FRAME: Mutex<Frame> = Mutex::new(Frame { aims: [None; 2], grips: [None; 2] });

/// The present loop's step, as the next frame's eye and head are chosen: the controllers as the
/// viewer located them now, for that frame ([`At::Frame`]).
pub fn sample_frame() {
    let (aims, grips) = crate::controllers::aims_and_grips();
    *FRAME.locked() = Frame { aims, grips };
}

/// Forgets the frame's sample (the loop's stop): until the next is taken, no hand at
/// [`At::Frame`].
pub fn forget_frame() {
    *FRAME.locked() = Frame { aims: [None; 2], grips: [None; 2] };
}

/// Hand `side` of game `G`: `point`, at moment `at`, in space `space`. None where it is not
/// tracked, or not known in that space at that moment (only palms are in the world, at their
/// latest).
pub fn pose<G: HandRig>(side: usize, point: Point, at: At, space: In) -> Option<HandPose> {
    match space {
        In::Tracking => {
            let (aim, grip) = match at {
                At::Latest => (crate::controllers::hand(side), crate::controllers::grip(side)),
                At::Frame => {
                    let frame = FRAME.locked();
                    (*frame.aims.get(side)?, *frame.grips.get(side)?)
                }
            };
            match point {
                Point::Controller => aim,
                Point::Palm => palm(at, aim, grip),
                Point::StringPoint => palm(at, aim, grip).map(|p| HandPose { position: on_palm(&p, STRING_ON_PALM), ..p }),
            }
        }
        In::World => {
            if at != At::Latest || point == Point::Controller {
                return None;
            }
            let m = G::trail()?.palm(side, FRESH)?;
            let place = match point {
                Point::StringPoint => offset(&m, STRING_ON_PALM),
                _ => position(&m),
            };
            Some(HandPose { orientation: quaternion(&m), position: place, valid: 1, received_tick: 0 })
        }
    }
}

/// The palm from an aim and a grip pose: at the latest, a tracked one only.
fn palm(at: At, aim: Option<HandPose>, grip: Option<HandPose>) -> Option<HandPose> {
    let palm = HandPose::palm(aim, grip);
    match at {
        At::Latest => palm.filter(|p| p.valid != 0),
        At::Frame => palm,
    }
}

/// `local` in the frame of `pose` (tracking space).
fn on_palm(pose: &HandPose, local: Vec3) -> Vec3 {
    let m = monaka_core::camera::apply_head(&monaka_core::camera::IDENTITY_MATRIX, pose.orientation, pose.position);
    offset(&m, local)
}

/// `local` in the frame of palm-to-world `m`.
fn offset(m: &Mat34, local: Vec3) -> Vec3 {
    let [x, y, z] = local;
    [m[3] + m[0] * x + m[1] * y + m[2] * z, m[7] + m[4] * x + m[5] * y + m[6] * z, m[11] + m[8] * x + m[9] * y + m[10] * z]
}

#[cfg(test)]
mod tests;
