//! The hand block beside a stereo channel: the viewer writes where each controller points (aim)
//! and where it sits in the hand (grip), a producer reads them. Either side may create it. Every access waits at most a couple of
//! milliseconds; a miss is not an error.

use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_core::protocol::{Block, HandBlock, HandPose, PadState, Versioned};

const WAIT_MS: u32 = 2;

pub struct HandChannel {
    block: SharedBlock<HandBlock>,
}

impl HandChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let (name, lock) = channel.block(Block::Hands);
        Ok(Self { block: SharedBlock::<HandBlock>::open_versioned(&name, &lock, WAIT_MS)? })
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
mod tests;
