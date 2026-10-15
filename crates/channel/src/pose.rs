//! The pose block beside a stereo channel: the viewer writes the latest head pose, the producer
//! writes the pose each published pair was rendered with. Either side may create it. Every access
//! waits at most a couple of milliseconds; a miss is not an error.

use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_core::protocol::{Block, HeadPose, PoseBlock, RenderedPair, Versioned};
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

    fn with<R>(&mut self, f: impl FnOnce(&mut PoseBlock) -> R) -> Option<R> {
        let mut record = self.block.lock(WAIT_MS).tolerant()?;
        record.is_compatible().then(|| f(&mut record))
    }

    pub fn write_head(&mut self, pose: &HeadPose) -> bool {
        self.with(|block| block.head = *pose).is_some()
    }

    /// The latest head pose, if the viewer has written a valid one.
    pub fn read_head(&mut self) -> Option<HeadPose> {
        self.with(|block| block.head).filter(|head| head.valid != 0)
    }

    pub fn write_pair(&mut self, pair: &RenderedPair) -> bool {
        self.with(|block| block.pairs[PoseBlock::pair_slot(pair.sequence)] = *pair).is_some()
    }

    /// The poses behind pair `sequence`, if its record is still in the ring.
    pub fn read_pair(&mut self, sequence: u64) -> Option<RenderedPair> {
        self.with(|block| block.pairs[PoseBlock::pair_slot(sequence)]).filter(|pair| pair.sequence == sequence)
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
