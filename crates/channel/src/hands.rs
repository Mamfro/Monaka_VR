//! The hand block beside a stereo channel: the viewer writes where each controller points (aim)
//! and where it sits in the hand (grip), a producer reads them. Either side may create it. Every access waits at most a couple of
//! milliseconds; a miss is not an error.

use crate::shared::{Create, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_core::protocol::{HandBlock, HandPose, PadState};

const WAIT_MS: u32 = 2;

pub struct HandChannel {
    block: SharedBlock<HandBlock>,
}

impl HandChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let mut block = SharedBlock::<HandBlock>::create(&channel.hands(), &channel.hands_lock(), Create::OrOpen)?;
        {
            let mut record = block.lock(WAIT_MS).tolerant().ok_or(Error::Busy)?;
            if record.is_blank() {
                *record = HandBlock::fresh();
            }
            if !record.is_compatible() {
                return Err(Error::Incompatible("hand block magic or version"));
            }
        }
        Ok(Self { block })
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut HandBlock) -> R) -> Option<R> {
        let mut record = self.block.lock(WAIT_MS).tolerant()?;
        record.is_compatible().then(|| f(&mut record))
    }

    /// Both hands' aim and grip poses.
    pub fn write(&mut self, aims: &[HandPose; 2], grips: &[HandPose; 2]) -> bool {
        self.with(|block| {
            block.hands = *aims;
            block.grips = *grips;
        })
        .is_some()
    }

    /// Hand `hand`'s aim pose ([`monaka_core::protocol::LEFT_HAND`] or `RIGHT_HAND`), if the viewer
    /// located it within the last `max_age_ms`.
    pub fn read(&mut self, hand: usize, max_age_ms: u64) -> Option<HandPose> {
        fresh(self.with(|block| block.hands.get(hand).copied())??, max_age_ms)
    }

    /// The controllers as one gamepad.
    pub fn write_pad(&mut self, pad: &PadState) -> bool {
        self.with(|block| block.pad = *pad).is_some()
    }

    /// The controllers' gamepad, if the viewer read it within the last `max_age_ms`.
    pub fn read_pad(&mut self, max_age_ms: u64) -> Option<monaka_core::pad::Gamepad> {
        let pad = self.with(|block| block.pad)?;
        (pad.valid != 0 && crate::tick().saturating_sub(pad.received_tick) <= max_age_ms).then_some(pad.gamepad)
    }

    /// Hand `hand`'s palm ([`HandPose::palm`]), if the viewer located it within the last
    /// `max_age_ms`.
    pub fn read_palm(&mut self, hand: usize, max_age_ms: u64) -> Option<HandPose> {
        HandPose::palm(self.read(hand, max_age_ms), self.read_grip(hand, max_age_ms))
    }

    /// Both palms from one locked viewer snapshot, so aim and grip cannot come
    /// from different writes. Each side independently rejects stale tracking.
    pub fn read_palms(&mut self, max_age_ms: u64) -> [Option<HandPose>; 2] {
        self.with(|block| std::array::from_fn(|i| HandPose::palm(fresh(block.hands[i], max_age_ms), fresh(block.grips[i], max_age_ms)))).unwrap_or([None; 2])
    }

    /// Hand `hand`'s grip pose, if the viewer located it within the last `max_age_ms`.
    pub fn read_grip(&mut self, hand: usize, max_age_ms: u64) -> Option<HandPose> {
        fresh(self.with(|block| block.grips.get(hand).copied())??, max_age_ms)
    }
}

fn fresh(pose: HandPose, max_age_ms: u64) -> Option<HandPose> {
    (pose.valid != 0 && crate::tick().saturating_sub(pose.received_tick) <= max_age_ms).then_some(pose)
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_core::protocol::RIGHT_HAND;

    #[test]
    fn hands_travel_and_go_stale() {
        let name = ChannelName::new(format!(r"Local\monaka-test-hands-{}", std::process::id())).unwrap();
        let mut writer = HandChannel::open(&name).unwrap();
        let mut reader = HandChannel::open(&name).unwrap();
        assert!(reader.read(RIGHT_HAND, 1000).is_none());
        let fresh = HandPose { valid: 1, position: [0.2, -0.3, -0.4], received_tick: crate::tick(), ..HandPose::default() };
        assert!(writer.write(&[HandPose::default(), fresh], &[HandPose::default(), fresh]));
        assert_eq!(reader.read(RIGHT_HAND, 1000), Some(fresh));
        assert_eq!(reader.read_grip(RIGHT_HAND, 1000), Some(fresh));
        let grip=HandPose { position:[0.3,-0.4,-0.5],..fresh };
        assert!(writer.write(&[HandPose::default(),fresh], &[HandPose::default(),grip]));
        assert_eq!(reader.read_palms(1000),[None,Some(HandPose { position:grip.position,..fresh })]);
        assert!(reader.read(0, 1000).is_none(), "an invalid hand is not returned");
        let old = HandPose { received_tick: crate::tick().saturating_sub(5000), ..fresh };
        assert!(writer.write(&[HandPose::default(), old], &[HandPose::default(); 2]));
        assert!(reader.read(RIGHT_HAND, 1000).is_none(), "a stale hand is not returned");
        assert_eq!(reader.read_palms(1000),[None,None]);
        assert!(reader.read(2, 1000).is_none());
    }
}
