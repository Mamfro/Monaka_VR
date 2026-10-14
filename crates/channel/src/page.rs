//! The game page beside a channel ([`monaka_core::page`]). The producer writes its rows and reads
//! the presses; the viewer (and the 2D window) reads the rows and leaves presses. Either side may
//! create it. Every access waits at most a couple of milliseconds; a miss is not an error.

use crate::shared::{Create, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_core::page::{Page, PageBlock, Press};

const WAIT_MS: u32 = 2;

pub struct PageChannel {
    block: SharedBlock<PageBlock>,
    /// The producer's: the newest press it has read.
    seen: u64,
}

impl PageChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let mut block = SharedBlock::<PageBlock>::create(&channel.page(), &channel.page_lock(), Create::OrOpen)?;
        let seen = {
            let mut record = block.lock(WAIT_MS).tolerant().ok_or(Error::Busy)?;
            if record.is_blank() {
                record.start();
            }
            if !record.is_compatible() {
                return Err(Error::Incompatible("page block magic or version"));
            }
            record.press_sequence
        };
        Ok(Self { block, seen })
    }

    /// Producer: the page as it is now.
    pub fn write(&mut self, page: &Page) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        record.write(page, crate::tick());
        true
    }

    /// Producer: the presses since the last call, oldest first.
    pub fn presses(&mut self) -> Vec<(u32, Press)> {
        let Some(record) = self.block.lock(WAIT_MS).tolerant() else { return Vec::new() };
        let (presses, seen) = record.presses_since(self.seen);
        self.seen = seen;
        presses
    }

    /// Viewer: the page, while its producer keeps writing it.
    pub fn read(&mut self) -> Option<Page> {
        let record = self.block.lock(WAIT_MS).tolerant()?;
        record.fresh(crate::tick()).copied()
    }

    /// Viewer: a press on button `id`; whether it was left.
    pub fn press(&mut self, id: u32, press: Press) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS * 5).tolerant() else { return false };
        record.press(id, press);
        true
    }
}
