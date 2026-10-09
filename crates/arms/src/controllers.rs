//! The VR controllers' poses: the hand block the viewer writes beside the pose channel, read on
//! demand on the game thread (once or twice per player camera update).

use monaka_channel::ChannelName;
use monaka_channel::fingers::FingerChannel;
use monaka_channel::hands::HandChannel;
use monaka_core::protocol::{FingerPose, HandPose, LEFT_HAND, RIGHT_HAND};
use monaka_producer::log;
use std::sync::Mutex;

/// A controller not located for this long is off or asleep: head aim takes over.
const MAX_AGE_MS: u64 = 250;

static CHANNEL: Mutex<Option<HandChannel>> = Mutex::new(None);
/// The finger block beside it (finger tracking, when the headset offers it).
static FINGERS: Mutex<Option<FingerChannel>> = Mutex::new(None);

/// Opens the hand block beside `channel`; without it, the controllers are never tracked.
pub fn open(channel: &ChannelName) {
    match HandChannel::open(channel) {
        Ok(hands) => *CHANNEL.lock().unwrap_or_else(|e| e.into_inner()) = Some(hands),
        Err(e) => log!("controllers: hand block unavailable ({e}); hand aim off"),
    }
    match FingerChannel::open(channel) {
        Ok(fingers) => *FINGERS.lock().unwrap_or_else(|e| e.into_inner()) = Some(fingers),
        Err(e) => log!("controllers: finger block unavailable ({e}); fingers as the game animates them"),
    }
}

pub fn close() {
    *CHANNEL.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *FINGERS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Hand `side`'s tracked fingers, if the viewer tracked them lately (open or clenched with binary
/// fingers on, [`crate::fingers::set_binary`]).
pub fn fingers(side: usize) -> Option<FingerPose> {
    let tracked = FINGERS.lock().ok()?.as_mut()?.read(side, MAX_AGE_MS)?;
    Some(crate::fingers::shape(side, tracked))
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
