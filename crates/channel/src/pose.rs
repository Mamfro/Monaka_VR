//! The pose block beside a stereo channel: the viewer writes the latest head pose, the producer
//! writes the pose each published pair was rendered with. Either side may create it (waiting
//! at most a couple of milliseconds for its lock); after that nothing waits ([`crate::seqlock`]),
//! and a miss is not an error.

use crate::seqlock;
use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_contract::protocol::{Block, HeadPose, PoseBlock, RenderedPair};
use std::sync::Mutex;
use monaka_core::sync::Locked;

const WAIT_MS: u32 = 2;

pub struct PoseChannel {
    block: SharedBlock<PoseBlock>,
}

impl PoseChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let (name, lock) = channel.block(Block::Pose);
        Ok(Self { block: SharedBlock::<PoseBlock>::open_versioned(&name, &lock, WAIT_MS)? })
    }

    pub fn write_head(&mut self, pose: &HeadPose) -> bool {
        // SAFETY: a field of the block's record.
        seqlock::write(&self.block, |block| unsafe { &raw mut (*block).head }, pose);
        true
    }

    /// The latest head pose, if the viewer has written a valid one.
    pub fn read_head(&mut self) -> Option<HeadPose> {
        // SAFETY: a field of the block's record.
        seqlock::read(&self.block, |block| unsafe { &raw const (*block).head }).filter(|head| head.valid != 0)
    }

    pub fn write_pair(&mut self, pair: &RenderedPair) -> bool {
        let slot = PoseBlock::pair_slot(pair.sequence);
        // SAFETY: a field of the block's record (the slot is in the ring).
        seqlock::write(&self.block, |block| unsafe { &raw mut (*block).pairs[slot] }, pair);
        true
    }

    /// The poses behind pair `sequence`, if its record is still in the ring.
    pub fn read_pair(&mut self, sequence: u64) -> Option<RenderedPair> {
        let slot = PoseBlock::pair_slot(sequence);
        // SAFETY: a field of the block's record (the slot is in the ring).
        seqlock::read(&self.block, |block| unsafe { &raw const (*block).pairs[slot] }).filter(|pair| pair.sequence == sequence)
    }
}

/// A synthetic head for PC runs, asked for the pose each time (it may move).
pub type Synthetic = Box<dyn Fn() -> HeadPose + Send>;

/// Where a producer's head pose comes from: the viewer's pose channel, or a synthetic head for PC
/// runs (the channel, when open, still gets the rendered pairs' records). Shareable as a static.
pub struct HeadSource {
    channel: Mutex<Option<PoseChannel>>,
    synthetic: Mutex<Option<Synthetic>>,
}

impl HeadSource {
    pub const fn new() -> Self {
        Self { channel: Mutex::new(None), synthetic: Mutex::new(None) }
    }

    pub fn open(&self, channel: Option<PoseChannel>, synthetic: Option<Synthetic>) {
        *self.channel.locked() = channel;
        *self.synthetic.locked() = synthetic;
    }

    pub fn close(&self) {
        self.channel.locked().take();
        self.synthetic.locked().take();
    }

    /// Whether the viewer's pose channel is open.
    pub fn connected(&self) -> bool {
        self.channel.locked().is_some()
    }

    /// The current head pose: the synthetic one when set, else the viewer's latest valid one.
    pub fn current(&self) -> Option<HeadPose> {
        if let Some(synthetic) = self.synthetic.locked().as_ref() {
            return Some(synthetic());
        }
        self.channel.locked().as_mut()?.read_head()
    }

    /// The poses a published pair was rendered with; write it before publishing the images.
    pub fn write_pair(&self, sequence: u64, eyes: [HeadPose; 2]) -> bool {
        self.channel.locked().as_mut().is_some_and(|c| c.write_pair(&RenderedPair { sequence, eyes }))
    }
}

impl Default for HeadSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
