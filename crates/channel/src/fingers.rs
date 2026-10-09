//! The finger block beside a stereo channel: the viewer writes each hand's finger bends when the
//! headset tracks them, a producer reads them. Either side may create it. Every access waits at
//! most a couple of milliseconds; a miss is not an error.

use crate::shared::{Create, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_core::protocol::{FingerBlock, FingerPose};

const WAIT_MS: u32 = 2;

pub struct FingerChannel {
    block: SharedBlock<FingerBlock>,
}

impl FingerChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let mut block = SharedBlock::<FingerBlock>::create(&channel.fingers(), &channel.fingers_lock(), Create::OrOpen)?;
        {
            let mut record = block.lock(WAIT_MS).tolerant().ok_or(Error::Busy)?;
            if record.is_blank() {
                *record = FingerBlock::fresh();
            }
            if !record.is_compatible() {
                return Err(Error::Incompatible("finger block magic or version"));
            }
        }
        Ok(Self { block })
    }

    /// Both hands' fingers (an untracked hand with `valid` 0).
    pub fn write(&mut self, hands: &[FingerPose; 2]) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        if !record.is_compatible() {
            return false;
        }
        record.hands = *hands;
        true
    }

    /// Hand `hand`'s fingers, if the viewer tracked them within the last `max_age_ms`.
    pub fn read(&mut self, hand: usize, max_age_ms: u64) -> Option<FingerPose> {
        let record = self.block.lock(WAIT_MS).tolerant()?;
        let pose = *record.is_compatible().then_some(record.hands.get(hand)?)?;
        (pose.valid != 0 && crate::tick().saturating_sub(pose.received_tick) <= max_age_ms).then_some(pose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingers_travel_and_go_stale() {
        let name = ChannelName::new(format!(r"Local\monaka-test-fingers-{}", std::process::id())).unwrap();
        let mut writer = FingerChannel::open(&name).unwrap();
        let mut reader = FingerChannel::open(&name).unwrap();
        assert!(reader.read(1, 1000).is_none());
        let mut fresh = FingerPose { valid: 1, received_tick: crate::tick(), ..FingerPose::default() };
        fresh.fingers[1] = [0.5, 0.4, 0.3, 0.1];
        assert!(writer.write(&[FingerPose::default(), fresh]));
        assert_eq!(reader.read(1, 1000), Some(fresh));
        assert!(reader.read(0, 1000).is_none(), "an untracked hand is not returned");
        let old = FingerPose { received_tick: crate::tick().saturating_sub(5000), ..fresh };
        assert!(writer.write(&[FingerPose::default(), old]));
        assert!(reader.read(1, 1000).is_none(), "stale fingers are not returned");
    }
}
