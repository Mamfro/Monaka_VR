//! The debug block beside a stereo channel ([`monaka_contract::debug_draw`]). The producer writes
//! each game frame's primitives, which frames each published pair's eyes were drawn in, and what
//! it can draw. The viewer writes what its menu wants and reads the rest. Either side may create
//! it. Every access waits at most a couple of milliseconds; a miss is not an error.

use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_contract::protocol::Block;
use monaka_contract::debug_draw::{COSTS, Categories, CostTime, DEPTH_CANDIDATES, DebugBlock, DebugDepth, DepthCandidate, GameReport, Primitive};

const WAIT_MS: u32 = 2;
/// A side that has not said what it wants or offers for this long (ms) is taken to want or offer
/// nothing: a viewer that went away stops the drawing.
pub const SAID_MS: u64 = 2000;

pub struct DebugChannel {
    block: SharedBlock<DebugBlock>,
}

/// Where an eye's primitives came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The frame the eye was drawn in.
    Synced(u64),
    /// The newest frame written: the pair's publisher said no frames, so the drawing may be a
    /// frame or so off the image.
    Newest(u64),
    /// Nothing written yet, or the block was busy.
    Nothing,
}

impl DebugChannel {
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let (name, lock) = channel.block(Block::Debug);
        Ok(Self { block: SharedBlock::<DebugBlock>::open_versioned(&name, &lock, WAIT_MS)? })
    }

    /// Producer: frame `key`'s primitives (at most [`monaka_contract::debug_draw::DEBUG_PRIMITIVES`]).
    pub fn write_frame(&mut self, key: u64, primitives: &[Primitive]) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        record.write_frame(key, primitives, crate::tick());
        true
    }

    /// Producer: the frames the eyes of published pair `sequence` were drawn in.
    pub fn write_eye_keys(&mut self, sequence: u64, keys: [u64; 2]) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        record.write_eye_keys(sequence, keys);
        true
    }

    /// Producer: what it can draw or do.
    pub fn offer(&mut self, offered: Categories) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        record.offered = offered.0;
        record.offered_tick = crate::tick();
        true
    }

    /// Producer: what the viewer wants, and the depth pass it wants shared; nothing once it has
    /// not said so for [`SAID_MS`].
    pub fn wanted(&mut self) -> Wanted {
        let Some(record) = self.block.lock(WAIT_MS).tolerant() else { return Wanted::default() };
        Wanted { categories: fresh(record.wanted, record.wanted_tick), pass: record.wanted_pass, buffer: record.wanted_buffer, target: record.wanted_target }
    }

    /// Producer: the game's timing (`text` is kept: [`DebugChannel::write_game_text`]).
    pub fn write_game(&mut self, stretch: &Stretch<'_>) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        let game = &mut record.game;
        (game.fps, game.frame_ms, game.worst_frame_ms, game.width, game.height) = (stretch.fps, stretch.frame_ms, stretch.worst_frame_ms, stretch.size[0], stretch.size[1]);
        game.push_frames(stretch.frames);
        game.costs = stretch.costs;
        game.tick = crate::tick();
        true
    }

    /// Producer: the run's rendering settings for the Stats panel, once at its start.
    pub fn write_game_text(&mut self, text: &str) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS * 5).tolerant() else { return false };
        record.game.set_text(text);
        true
    }

    /// Viewer: the game's timing and settings; the timing zeroed once it has not been written for
    /// [`SAID_MS`] (the game stopped presenting).
    pub fn game(&mut self) -> Option<GameReport> {
        let record = self.block.lock(WAIT_MS).tolerant()?;
        let mut game = record.game;
        if crate::tick().saturating_sub(game.tick) > SAID_MS {
            (game.fps, game.frame_ms, game.worst_frame_ms) = (0.0, 0.0, 0.0);
        }
        Some(game)
    }

    /// Producer: the depth buffers it saw drawn with, and which it shares (1 the first, 0 none).
    pub fn write_candidates(&mut self, candidates: &[DepthCandidate], chosen: u32) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        let count = candidates.len().min(DEPTH_CANDIDATES);
        record.candidates[..count].copy_from_slice(&candidates[..count]);
        record.candidate_count = count as u32;
        record.chosen = chosen;
        true
    }

    /// Producer: the render target it shares now ([`crate::debug_image`]).
    pub fn write_target(&mut self, target: DebugDepth) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        record.target = target;
        true
    }

    /// Viewer: the render target the producer shares, if it has said anything yet.
    pub fn target(&mut self) -> Option<DebugDepth> {
        let record = self.block.lock(WAIT_MS).tolerant()?;
        (record.target.generation != 0).then_some(record.target)
    }

    /// Producer: the render targets it saw drawn into, and which it shares (1 the first, 0 none).
    pub fn write_targets(&mut self, targets: &[DepthCandidate], chosen: u32) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        let count = targets.len().min(DEPTH_CANDIDATES);
        record.targets[..count].copy_from_slice(&targets[..count]);
        record.target_count = count as u32;
        record.target_chosen = chosen;
        true
    }

    /// Viewer: the render targets the producer lists, and which it shares.
    pub fn targets(&mut self) -> (Vec<DepthCandidate>, u32) {
        let Some(record) = self.block.lock(WAIT_MS).tolerant() else { return (Vec::new(), 0) };
        let count = (record.target_count as usize).min(DEPTH_CANDIDATES);
        (record.targets[..count].to_vec(), record.target_chosen)
    }

    /// Viewer: the depth buffers the producer lists, and which it shares (1 the first, 0 none).
    pub fn candidates(&mut self) -> (Vec<DepthCandidate>, u32) {
        let Some(record) = self.block.lock(WAIT_MS).tolerant() else { return (Vec::new(), 0) };
        let count = (record.candidate_count as usize).min(DEPTH_CANDIDATES);
        (record.candidates[..count].to_vec(), record.chosen)
    }

    /// Producer: the depth it shares now ([`crate::debug_depth`]).
    pub fn write_depth(&mut self, depth: DebugDepth) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        record.depth = depth;
        true
    }

    /// Viewer: the depth the producer shares, if it has said anything yet.
    pub fn depth(&mut self) -> Option<DebugDepth> {
        let record = self.block.lock(WAIT_MS).tolerant()?;
        (record.depth.generation != 0).then_some(record.depth)
    }

    /// Viewer: what its menu wants, and the depth pass. Said again at least every second while it
    /// runs.
    pub fn want(&mut self, wanted: Wanted) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        record.wanted = wanted.categories.0;
        record.wanted_pass = wanted.pass;
        record.wanted_buffer = wanted.buffer;
        record.wanted_target = wanted.target;
        record.wanted_tick = crate::tick();
        true
    }

    /// Viewer: what the producer can draw or do; nothing once it has not said so for [`SAID_MS`].
    pub fn offered(&mut self) -> Categories {
        let Some(record) = self.block.lock(WAIT_MS).tolerant() else { return Categories::NONE };
        fresh(record.offered, record.offered_tick)
    }

    /// Viewer: the frames the eyes of published pair `sequence` were drawn in, where its
    /// publisher named them.
    pub fn eye_keys(&mut self, sequence: u64) -> Option<[u64; 2]> {
        self.block.lock(WAIT_MS).tolerant()?.eye_keys(sequence)
    }

    /// Viewer: the primitives of each eye of published pair `sequence`, into `eyes` (cleared
    /// first), and where each came from.
    pub fn read_pair(&mut self, sequence: u64, eyes: &mut [Vec<Primitive>; 2]) -> [Source; 2] {
        eyes.iter_mut().for_each(Vec::clear);
        let Some(record) = self.block.lock(WAIT_MS).tolerant() else { return [Source::Nothing; 2] };
        let keys = record.eye_keys(sequence);
        let mut sources = [Source::Nothing; 2];
        for (eye, into) in eyes.iter_mut().enumerate() {
            let found = match keys {
                Some(keys) => record.frame(keys[eye]).map(|f| (f, Source::Synced(f.key))),
                None => record.newest().map(|f| (f, Source::Newest(f.key))),
            };
            if let Some((frame, source)) = found {
                into.extend_from_slice(frame.primitives());
                sources[eye] = source;
            }
        }
        sources
    }
}

/// What the viewer wants: the categories, the depth pass and the depth buffer
/// ([`DebugBlock::wanted_pass`], [`DebugBlock::wanted_buffer`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    pub categories: Categories,
    pub pass: u32,
    pub buffer: u32,
    /// The render target wanted shared (0: the producer's pick, else that one of its list).
    pub target: u32,
}

/// One measured stretch of the game's frames, for [`DebugChannel::write_game`].
#[derive(Clone, Copy, Debug)]
pub struct Stretch<'a> {
    pub fps: f32,
    /// The frames' mean and longest time (ms).
    pub frame_ms: f32,
    pub worst_frame_ms: f32,
    /// The game's swapchain (0: unknown).
    pub size: [u32; 2],
    /// Each frame's time (ms) since the last stretch, oldest first.
    pub frames: &'a [f32],
    pub costs: [CostTime; COSTS],
}

fn fresh(bits: u64, said: u64) -> Categories {
    if crate::tick().saturating_sub(said) <= SAID_MS { Categories(bits) } else { Categories::NONE }
}

#[cfg(test)]
mod tests;
