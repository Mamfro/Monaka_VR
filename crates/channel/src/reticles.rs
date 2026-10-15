//! The reticle block beside a stereo channel: a producer writes where a hand's reticle points when
//! that is not along the palm (a bow held at an angle in it), the viewer reads it. Either side may
//! create it (waiting at most a couple of milliseconds for its lock); after that nothing waits
//! ([`crate::seqlock`]), and a miss is not an error.

use crate::seqlock;
use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_contract::protocol::{Block, ReticleAim, ReticleBlock};

const WAIT_MS: u32 = 2;

pub struct ReticleChannel {
    block: SharedBlock<ReticleBlock>,
}

impl ReticleChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let (name, lock) = channel.block(Block::Reticles);
        Ok(Self { block: SharedBlock::<ReticleBlock>::open_versioned(&name, &lock, WAIT_MS)? })
    }

    /// Hand `hand`'s reticle along `direction` in its palm's frame, or back on the palm (`None`).
    pub fn write(&mut self, hand: usize, direction: Option<[f32; 3]>) -> bool {
        if hand >= 2 {
            return false;
        }
        let aim = match direction {
            Some(direction) => ReticleAim { valid: 1, direction, written_tick: crate::tick(), ..ReticleAim::default() },
            None => ReticleAim::default(),
        };
        // SAFETY: a field of the block's record (`hand` is one of its two).
        seqlock::write(&self.block, |block| unsafe { &raw mut (*block).aims[hand] }, &aim);
        true
    }

    /// Hand `hand`'s reticle direction in its palm's frame, if the producer said one within the last
    /// `max_age_ms`.
    pub fn read(&mut self, hand: usize, max_age_ms: u64) -> Option<[f32; 3]> {
        if hand >= 2 {
            return None;
        }
        // SAFETY: a field of the block's record (`hand` is one of its two).
        let aim = seqlock::read(&self.block, |block| unsafe { &raw const (*block).aims[hand] })?;
        (aim.valid != 0 && crate::tick().saturating_sub(aim.written_tick) <= max_age_ms).then_some(aim.direction)
    }
}

#[cfg(test)]
mod tests;
