//! The reticle block beside a stereo channel: a producer writes where a hand's reticle points when
//! that is not along the palm (a bow held at an angle in it), the viewer reads it. Either side may
//! create it. Every access waits at most a couple of milliseconds; a miss is not an error.

use crate::shared::{Create, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_core::protocol::{ReticleAim, ReticleBlock};

const WAIT_MS: u32 = 2;

pub struct ReticleChannel {
    block: SharedBlock<ReticleBlock>,
}

impl ReticleChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let mut block = SharedBlock::<ReticleBlock>::create(&channel.reticles(), &channel.reticles_lock(), Create::OrOpen)?;
        {
            let mut record = block.lock(WAIT_MS).tolerant().ok_or(Error::Busy)?;
            if record.is_blank() {
                *record = ReticleBlock::fresh();
            }
            if !record.is_compatible() {
                return Err(Error::Incompatible("reticle block magic or version"));
            }
        }
        Ok(Self { block })
    }

    /// Hand `hand`'s reticle along `direction` in its palm's frame, or back on the palm (`None`).
    pub fn write(&mut self, hand: usize, direction: Option<[f32; 3]>) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        if !record.is_compatible() || hand >= record.aims.len() {
            return false;
        }
        record.aims[hand] = match direction {
            Some(direction) => ReticleAim { valid: 1, direction, written_tick: crate::tick(), ..ReticleAim::default() },
            None => ReticleAim::default(),
        };
        true
    }

    /// Hand `hand`'s reticle direction in its palm's frame, if the producer said one within the last
    /// `max_age_ms`.
    pub fn read(&mut self, hand: usize, max_age_ms: u64) -> Option<[f32; 3]> {
        let aim = self.block.lock(WAIT_MS).tolerant().filter(|r| r.is_compatible()).and_then(|r| r.aims.get(hand).copied())?;
        (aim.valid != 0 && crate::tick().saturating_sub(aim.written_tick) <= max_age_ms).then_some(aim.direction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reticle_travels_and_goes_stale() {
        let name = ChannelName::new(format!(r"Local\monaka-test-reticles-{}", std::process::id())).unwrap();
        let mut producer = ReticleChannel::open(&name).unwrap();
        let mut viewer = ReticleChannel::open(&name).unwrap();
        assert_eq!(viewer.read(0, 1000), None);
        assert!(producer.write(0, Some([0.0, 0.3, -0.95])));
        assert_eq!(viewer.read(0, 1000), Some([0.0, 0.3, -0.95]));
        assert_eq!(viewer.read(1, 1000), None);
        assert!(producer.write(0, None));
        assert_eq!(viewer.read(0, 1000), None);
    }
}
