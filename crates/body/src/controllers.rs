//! The VR controllers' poses: the hand block the viewer writes beside the pose channel, read on
//! demand on the game thread (once or twice per player camera update).

use monaka_channel::ChannelName;
use monaka_channel::fingers::FingerChannel;
use monaka_channel::hands::HandChannel;
use monaka_channel::reticles::ReticleChannel;
use monaka_contract::protocol::{FingerPose, HandPose, HeadPose, LEFT_HAND, RIGHT_HAND};
use monaka_producer::log;
use std::sync::Mutex;
use std::time::Instant;
use monaka_core::sync::Locked;

/// PC checks without a headset: palms made here instead of read ([`use_synthetic`]).
static SYNTHETIC: Mutex<Option<Synthetic>> = Mutex::new(None);

struct Synthetic {
    since: Instant,
    /// Both palms lost for a second, two seconds after they are first asked for.
    loss_test: bool,
    first_asked: Option<Instant>,
}

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
    *SYNTHETIC.locked() = None;
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
    palms().get(side).copied().flatten()
}

/// Both palms, by side, from one viewer write (aim and grip of the same moment); the synthetic
/// ones while PC checks make them up ([`use_synthetic`]).
pub fn palms() -> [Option<HandPose>; 2] {
    if let Some(synthetic) = SYNTHETIC.locked().as_mut() {
        return synthetic_palms(synthetic);
    }
    let Ok(mut channel) = CHANNEL.lock() else { return [None; 2] };
    let Some(hands) = channel.as_mut() else { return [None; 2] };
    hands.read_palms(MAX_AGE_MS)
}

/// PC checks without a headset (`on`): the palms swing gently side to side in front of the head
/// instead of being read; `loss_test` loses both for a second, two seconds after they are first
/// asked for, to see what a game does without them. Off: read again.
pub fn use_synthetic(on: bool, loss_test: bool) {
    *SYNTHETIC.locked() = on.then(|| Synthetic { since: Instant::now(), loss_test, first_asked: None });
}

fn synthetic_palms(synthetic: &mut Synthetic) -> [Option<HandPose>; 2] {
    let asked = synthetic.first_asked.get_or_insert_with(Instant::now).elapsed().as_secs_f32();
    if synthetic.loss_test && (2.0..3.0).contains(&asked) {
        return [None; 2];
    }
    let t = synthetic.since.elapsed().as_secs_f32();
    std::array::from_fn(|i| {
        let side = if i == LEFT_HAND { -1.0 } else { 1.0 };
        let head = HeadPose::synthetic(side * 15.0 * (t * 1.5).sin(), 0.0, HeadPose::SYNTHETIC_FOV, 0.064);
        Some(HandPose {
            orientation: head.orientation,
            position: [side * (0.24 + 0.05 * (t * 1.5).sin()), -0.20 + 0.06 * (t * 1.5 + side).cos(), -0.42],
            valid: 1,
            received_tick: monaka_channel::tick(),
        })
    })
}
