//! The VR controllers' poses: the hand block the viewer writes beside the pose channel, read on
//! demand on the game thread (once or twice per player camera update).

use monaka_channel::ChannelName;
use monaka_channel::fingers::FingerChannel;
use monaka_channel::hands::HandChannel;
use monaka_channel::reticles::ReticleChannel;
use monaka_contract::protocol::{FingerPose, HandPose, LEFT_HAND, RIGHT_HAND};
use monaka_producer::log;
use std::sync::Mutex;
use monaka_core::sync::Locked;

/// A controller not located for this long is off or asleep: head aim takes over.
const MAX_AGE_MS: u64 = 250;

static CHANNEL: Mutex<Option<HandChannel>> = Mutex::new(None);
/// The finger block beside it (finger tracking, when the headset offers it).
static FINGERS: Mutex<Option<FingerChannel>> = Mutex::new(None);
/// The reticle block beside it (where a hand's reticle points, when not along the palm).
static RETICLES: Mutex<Option<ReticleChannel>> = Mutex::new(None);

/// Opens the hand block beside `channel`; without it, the controllers are never tracked. A block
/// already open stays as it is, so each component that reads the controllers opens them.
pub fn open(channel: &ChannelName) {
    let mut hands = CHANNEL.locked();
    if hands.is_none() {
        match HandChannel::open(channel) {
            Ok(opened) => *hands = Some(opened),
            Err(e) => log!("controllers: hand block unavailable ({e}); hand aim off"),
        }
    }
    let mut fingers = FINGERS.locked();
    if fingers.is_none() {
        match FingerChannel::open(channel) {
            Ok(opened) => *fingers = Some(opened),
            Err(e) => log!("controllers: finger block unavailable ({e}); fingers as the game animates them"),
        }
    }
    let mut reticles = RETICLES.locked();
    if reticles.is_none() {
        match ReticleChannel::open(channel) {
            Ok(opened) => *reticles = Some(opened),
            Err(e) => log!("controllers: reticle block unavailable ({e}); the reticles stay on the palms"),
        }
    }
}

pub fn close() {
    *CHANNEL.locked() = None;
    *FINGERS.locked() = None;
    if let Some(mut reticles) = RETICLES.locked().take() {
        reticles.write(LEFT_HAND, None);
        reticles.write(RIGHT_HAND, None);
    }
}

/// Hand `side`'s tracked fingers, if the viewer tracked them lately (open or clenched with binary
/// fingers on, [`crate::fingers::set_binary`]).
pub fn fingers(side: usize) -> Option<FingerPose> {
    let tracked = FINGERS.lock().ok()?.as_mut()?.read(side, MAX_AGE_MS)?;
    Some(crate::fingers::shape(side, tracked))
}

/// Controller `side`'s aim pose in the headset's tracking space, if tracked.
pub fn hand(side: usize) -> Option<HandPose> {
    CHANNEL.lock().ok()?.as_mut()?.read(side, MAX_AGE_MS)
}

/// Controller `side`'s grip pose in the headset's tracking space, if tracked.
pub fn grip(side: usize) -> Option<HandPose> {
    CHANNEL.lock().ok()?.as_mut()?.read_grip(side, MAX_AGE_MS)
}

/// Both controllers' aim poses and grip poses, by side, each if tracked.
pub fn aims_and_grips() -> ([Option<HandPose>; 2], [Option<HandPose>; 2]) {
    let Ok(mut channel) = CHANNEL.lock() else { return ([None; 2], [None; 2]) };
    let Some(hands) = channel.as_mut() else { return ([None; 2], [None; 2]) };
    let (left, right) = ((hands.read(LEFT_HAND, MAX_AGE_MS), hands.read_grip(LEFT_HAND, MAX_AGE_MS)), (hands.read(RIGHT_HAND, MAX_AGE_MS), hands.read_grip(RIGHT_HAND, MAX_AGE_MS)));
    ([left.0, right.0], [left.1, right.1])
}

/// Controller `side`'s palm ([`HandPose::palm`]) in the headset's tracking space, if tracked.
pub fn palm(side: usize) -> Option<HandPose> {
    CHANNEL.lock().ok()?.as_mut()?.read_palm(side, MAX_AGE_MS)
}

/// Both palms, by side.
pub fn palms() -> [Option<HandPose>; 2] {
    let Ok(mut channel) = CHANNEL.lock() else { return [None; 2] };
    let Some(hands) = channel.as_mut() else { return [None; 2] };
    [hands.read_palm(LEFT_HAND, MAX_AGE_MS), hands.read_palm(RIGHT_HAND, MAX_AGE_MS)]
}
