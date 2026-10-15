//! The producer's side of the fence header, the same for a D3D12 and a D3D11 producer: the header
//! written once its shared objects exist, each pair's slot chosen and marked under one lock, and
//! the pair committed after its copy is queued (kept for the next lock when it stays busy).
//!
//! A pair's depth goes in the depth block beside the header ([`DepthBlock`]), opened with the first
//! pair that has depth. Its slot is written after the pair's slot is marked and before the pair is
//! committed, so a reader that takes the pair finds its depth there; the reader uses a slot's depth
//! only when its sequence is the pair's, so a depth write that misses its lock leaves that pair
//! without depth, never with another's.

use crate::shared::{Create, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_contract::protocol::{Block, DepthBlock, DepthSlot, FENCE_SLOTS, FenceHeader, Luid, NO_SLOT, Stamp};
use monaka_core::fence_rules::{self, Commit, choose_slot};
use windows::Win32::System::Threading::GetCurrentProcessId;

/// How many times a producer tries the header lock to commit a submitted pair before leaving the
/// commit for its next publish: a stalled or suspended reader holding the lock must never hold the
/// game's thread.
const COMMIT_TRIES: u32 = 64;

/// What a producer publishes in its header: its images' size and format, its GPU, and the handle
/// values of its slot textures and fences (valid in the producer process).
pub(crate) struct Shared {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: u32,
    pub(crate) adapter: Luid,
    pub(crate) textures: [[u64; 2]; FENCE_SLOTS],
    pub(crate) produced: u64,
    pub(crate) consumed: u64,
}

/// A slot marked with a pair: which, and what it held before.
#[derive(Clone, Copy)]
pub(crate) struct Marked {
    pub(crate) slot: usize,
    before: (u64, u64),
}

pub(crate) struct HeaderWriter {
    channel: ChannelName,
    block: SharedBlock<FenceHeader>,
    /// The depth block, once a pair has come with depth.
    depth: Option<SharedBlock<DepthBlock>>,
    /// The depth block would not open (another side made it at another major): pairs go without
    /// depth for the rest of the run.
    depth_refused: bool,
    /// A submitted pair whose commit found the lock busy; written under the next lock.
    pending: Option<Commit>,
}

impl HeaderWriter {
    /// Claims `name`: its header made, empty (no reader opens it before [`HeaderWriter::publish`]).
    pub(crate) fn claim(name: &ChannelName) -> Result<Self> {
        let block = SharedBlock::<FenceHeader>::create(&name.header(), &name.header_lock(), Create::Exclusive)?;
        Ok(Self { channel: name.clone(), block, depth: None, depth_refused: false, pending: None })
    }

    /// Writes the header whole, once the shared objects exist.
    pub(crate) fn publish(&mut self, shared: Shared) -> Result<()> {
        let mut record = self.block.lock(0).tolerant().ok_or(Error::Busy)?;
        *record = FenceHeader {
            stamp: Stamp::of::<FenceHeader>(),
            width: shared.width,
            height: shared.height,
            format: shared.format,
            // SAFETY: no preconditions.
            producer_pid: unsafe { GetCurrentProcessId() },
            adapter: shared.adapter,
            textures: shared.textures,
            produced: shared.produced,
            consumed: shared.consumed,
            latest_slot: NO_SLOT,
            reader_slot: NO_SLOT,
            ..FenceHeader::default()
        };
        Ok(())
    }

    /// Whether pairs go without depth for the rest of the run: the depth block would not open.
    pub(crate) fn depth_refused(&self) -> bool {
        self.depth_refused
    }

    /// Names a new set of depth textures (`textures`, `format`, `width` x `height`) for the reader to
    /// open, opening the depth block first if this is the first. False: the block's lock was busy
    /// or the block would not open ([`HeaderWriter::depth_refused`] from then on), nothing changed.
    pub(crate) fn new_depth(&mut self, format: u32, width: u32, height: u32, textures: [[u64; 2]; FENCE_SLOTS]) -> bool {
        if self.depth.is_none() && !self.depth_refused {
            let (name, lock) = self.channel.block(Block::Depth);
            match SharedBlock::<DepthBlock>::open_versioned(&name, &lock, 0) {
                Ok(block) => self.depth = Some(block),
                // Busy: another side is starting it; tried again with the next pair.
                Err(Error::Busy) => {}
                Err(_) => self.depth_refused = true,
            }
        }
        let Some(mut record) = self.depth.as_mut().and_then(|block| block.lock(0).tolerant()) else { return false };
        let generation = record.generation.wrapping_add(1).max(1);
        *record = DepthBlock { stamp: record.stamp, generation, format, width, height, textures, slots: [DepthSlot::default(); FENCE_SLOTS] };
        true
    }

    /// Chooses a free slot for pair `sequence` (`consumed`: the reader's fence as completed) and
    /// marks it with the pair, in the same lock, so the reader never takes it while it is being
    /// rewritten; then gives the slot the pair's `depth` in the depth block, when there is one. A
    /// pair whose commit was kept is committed first. `None`: the lock was busy or no slot was free.
    pub(crate) fn mark(&mut self, consumed: u64, sequence: u64, tick: u64, depth: DepthSlot) -> Option<Marked> {
        let marked = {
            let mut record = self.block.lock(0).tolerant()?;
            if let Some(earlier) = self.pending.take() {
                fence_rules::commit(&mut record, earlier);
            }
            let slot = choose_slot(&record, consumed)? as usize;
            let before = (record.slot_sequences[slot], record.slot_ticks[slot]);
            (record.slot_sequences[slot], record.slot_ticks[slot]) = (sequence, tick);
            Marked { slot, before }
        };
        if let Some(mut record) = self.depth.as_mut().and_then(|block| block.lock(0).tolerant()) {
            record.slots[marked.slot] = depth;
        }
        Some(marked)
    }

    /// The pair was not copied after all: the slot holds what it held. False: the lock was busy and
    /// the mark stays (its sequence is then spent: it completes only with a later pair's signal, and
    /// the reader takes that newer pair). The slot's depth is left: it names the pair not copied, so
    /// the restored pair is read without depth.
    pub(crate) fn restore(&mut self, marked: Marked) -> bool {
        let Some(mut record) = self.block.lock(0).tolerant() else { return false };
        (record.slot_sequences[marked.slot], record.slot_ticks[marked.slot]) = marked.before;
        true
    }

    /// The pair's copy may have run in part: its slot is emptied. If the lock is busy the mark
    /// stays, which is harmless (as for [`HeaderWriter::restore`]).
    pub(crate) fn unmark(&mut self, marked: Marked) {
        if let Some(mut record) = self.block.lock(0).tolerant() {
            fence_rules::unmark(&mut record, marked.slot as u32);
        }
    }

    /// Commits the submitted pair `sequence` in `marked`'s slot, or keeps it for the next lock
    /// when the lock stays busy. Only the producer writes `sequence`; the lock orders it against a
    /// reader's claim.
    pub(crate) fn commit(&mut self, marked: Marked, sequence: u64, tick: u64) {
        let done = Commit { slot: marked.slot as u32, sequence, tick };
        for _ in 0..COMMIT_TRIES {
            if let Some(mut record) = self.block.lock(0).tolerant() {
                if let Some(earlier) = self.pending.take() {
                    fence_rules::commit(&mut record, earlier);
                }
                fence_rules::commit(&mut record, done);
                return;
            }
            std::hint::spin_loop();
        }
        self.pending = Some(done);
    }
}
