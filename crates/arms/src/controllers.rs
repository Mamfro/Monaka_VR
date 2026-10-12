//! The VR controllers' poses: the hand block the viewer writes beside the pose channel, read on
//! demand on the game thread (once or twice per player camera update).

use monaka_channel::ChannelName;
use monaka_channel::fingers::FingerChannel;
use monaka_channel::hands::HandChannel;
use monaka_channel::reticles::ReticleChannel;
use monaka_core::protocol::{FingerPose, HandPose, LEFT_HAND, RIGHT_HAND};
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

/// Opens the hand block beside `channel`; without it, the controllers are never tracked.
pub fn open(channel: &ChannelName) {
    match HandChannel::open(channel) {
        Ok(hands) => *CHANNEL.locked() = Some(hands),
        Err(e) => log!("controllers: hand block unavailable ({e}); hand aim off"),
    }
    match FingerChannel::open(channel) {
        Ok(fingers) => *FINGERS.locked() = Some(fingers),
        Err(e) => log!("controllers: finger block unavailable ({e}); fingers as the game animates them"),
    }
    match ReticleChannel::open(channel) {
        Ok(reticles) => *RETICLES.locked() = Some(reticles),
        Err(e) => log!("controllers: reticle block unavailable ({e}); the reticles stay on the palms"),
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

/// Points hand `side`'s reticle along `direction` in its palm's frame (forward -Z), or back along
/// the palm (`None`): what the hand holds aims another way than the palm (a bow).
pub fn aim_reticle(side: usize, direction: Option<[f32; 3]>) {
    if let Some(reticles) = RETICLES.lock().ok().as_mut().and_then(|r| r.as_mut()) {
        reticles.write(side, direction);
    }
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
