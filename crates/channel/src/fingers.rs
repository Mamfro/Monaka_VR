//! The finger block beside a stereo channel: the viewer writes each hand's finger bends when the
//! headset tracks them, a producer reads them. Either side may create it (waiting
//! at most a couple of milliseconds for its lock); after that nothing waits ([`crate::seqlock`]),
//! and a miss is not an error.

use crate::seqlock;
use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_contract::protocol::{Block, FingerBlock, FingerPose};

const WAIT_MS: u32 = 2;

pub struct FingerChannel {
    block: SharedBlock<FingerBlock>,
}

impl FingerChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let (name, lock) = channel.block(Block::Fingers);
        Ok(Self { block: SharedBlock::<FingerBlock>::open_versioned(&name, &lock, WAIT_MS)? })
    }

    /// Both hands' fingers (an untracked hand with `valid` 0).
    pub fn write(&mut self, hands: &[FingerPose; 2]) -> bool {
        // SAFETY: a field of the block's record.
        seqlock::write(&self.block, |block| unsafe { &raw mut (*block).hands }, hands);
        true
    }

    /// Hand `hand`'s fingers, if the viewer tracked them within the last `max_age_ms`.
    pub fn read(&mut self, hand: usize, max_age_ms: u64) -> Option<FingerPose> {
        // SAFETY: a field of the block's record.
        let hands = seqlock::read(&self.block, |block| unsafe { &raw const (*block).hands })?;
        let pose = *hands.get(hand)?;
        (pose.valid != 0 && crate::tick().saturating_sub(pose.received_tick) <= max_age_ms).then_some(pose)
    }
}

#[cfg(test)]
mod tests;
