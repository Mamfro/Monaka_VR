//! The HUD panels block beside a stereo channel ([`monaka_contract::panels`]): a producer declares the
//! pieces it puts on the hands and keeps the block open while it runs; the viewer reads it. Either
//! side may create it. Every access waits at most a couple of milliseconds; a miss is not an error.

use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_contract::panels::{PanelsBlock, Place};
use monaka_contract::protocol::{Block, Versioned};

const WAIT_MS: u32 = 2;

pub struct PanelsChannel {
    block: SharedBlock<PanelsBlock>,
}

impl PanelsChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let (name, lock) = channel.block(Block::Panels);
        Ok(Self { block: SharedBlock::<PanelsBlock>::open_versioned(&name, &lock, WAIT_MS * 50)? })
    }

    /// Producer: declares `pieces`. How many were (none when the block was busy).
    pub fn declare<'a>(&mut self, pieces: impl IntoIterator<Item = (&'a str, Place)>) -> usize {
        self.block.lock(WAIT_MS * 50).tolerant().map_or(0, |mut record| record.declare(pieces))
    }

    /// Viewer: the pieces declared, in order (none yet, or the block busy: empty).
    pub fn pieces(&mut self) -> Vec<(String, Place)> {
        let Some(record) = self.block.lock(WAIT_MS).tolerant() else { return Vec::new() };
        if !record.is_compatible() {
            return Vec::new();
        }
        record.pieces().map(|(name, place)| (name.to_owned(), place)).collect()
    }
}
