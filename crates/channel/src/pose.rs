//! The pose block beside a stereo channel: the viewer writes the latest head pose, the producer
//! writes the pose each published pair was rendered with. Either side may create it (waiting
//! at most a couple of milliseconds for its lock); after that nothing waits ([`crate::seqlock`]),
//! and a miss is not an error.

use crate::seqlock;
use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_contract::protocol::{Block, DisplayTiming, HeadPose, PoseBlock, RenderedPair};
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

    /// The headset's refresh, as the viewer's frame wait just returned (`DisplayTiming`).
    pub fn write_display(&mut self, timing: &DisplayTiming) -> bool {
        // SAFETY: a field of the block's record (appended in minor 1; the mapping's capacity holds
        // it whoever made the block).
        seqlock::write(&self.block, |block| unsafe { &raw mut (*block).display }, timing);
        true
    }

    /// The headset's refresh as the viewer last wrote it; None from a viewer that sends none.
    pub fn read_display(&mut self) -> Option<DisplayTiming> {
        // SAFETY: as in `write_display`.
        seqlock::read(&self.block, |block| unsafe { &raw const (*block).display }).filter(|d| d.period_ns > 0)
    }

    /// The poses behind pair `sequence`, if its record is still in the ring.
    pub fn read_pair(&mut self, sequence: u64) -> Option<RenderedPair> {
        let slot = PoseBlock::pair_slot(sequence);
        // SAFETY: a field of the block's record (the slot is in the ring).
        seqlock::read(&self.block, |block| unsafe { &raw const (*block).pairs[slot] }).filter(|pair| pair.sequence == sequence)
    }
}

/// Nanoseconds of the Windows performance counter now: the clock [`DisplayTiming`] counts in,
/// read alike by every process.
pub fn now_ns() -> u64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut count, mut frequency) = (0i64, 0i64);
    // SAFETY: two locals the counter writes.
    let read = unsafe { QueryPerformanceCounter(&mut count).is_ok() && QueryPerformanceFrequency(&mut frequency).is_ok() };
    if !read || frequency <= 0 {
        return 0;
    }
    (count as u128 * 1_000_000_000 / frequency as u128) as u64
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

    /// The headset's refresh as the viewer last wrote it (none from a synthetic head, or from a
    /// viewer that sends none).
    pub fn display(&self) -> Option<DisplayTiming> {
        self.channel.locked().as_mut()?.read_display()
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
