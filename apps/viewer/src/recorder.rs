//! The last seconds of a run kept for the debug menu's "Save the last 30 s" ([`monaka_core::stream`]):
//! only while its Record switch is on; saved as `stream-N.bin` beside the run's captures.

use monaka_core::stream::{self, Frame};
use std::collections::VecDeque;
use std::path::PathBuf;

/// How long is kept (ms), and at most how much (bytes).
const KEEP_MS: u64 = 30_000;
const MOST_BYTES: usize = 256 << 20;

pub struct Recorder {
    /// (viewer clock, encoded frame), oldest first.
    frames: VecDeque<(u64, Vec<u8>)>,
    bytes: usize,
    folder: Option<PathBuf>,
}

impl Recorder {
    /// Saves go into `folder` (the run's captures folder; none: nothing can be saved).
    pub fn new(folder: Option<PathBuf>) -> Self {
        Self { frames: VecDeque::new(), bytes: 0, folder }
    }

    pub fn push(&mut self, frame: &Frame) {
        let encoded = frame.encode();
        self.bytes += encoded.len();
        self.frames.push_back((frame.tick, encoded));
        while let Some((tick, bytes)) = self.frames.front() {
            if frame.tick.saturating_sub(*tick) <= KEEP_MS && self.bytes <= MOST_BYTES {
                break;
            }
            self.bytes -= bytes.len();
            self.frames.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    /// Seconds kept.
    pub fn seconds(&self) -> f32 {
        match (self.frames.front(), self.frames.back()) {
            (Some((first, _)), Some((last, _))) => (last - first) as f32 / 1000.0,
            _ => 0.0,
        }
    }

    /// Writes what is kept to the next free `stream-N.bin`, on a thread of its own; the file's name.
    pub fn save(&self) -> Result<PathBuf, String> {
        let folder = self.folder.clone().ok_or("no folder to save to (--captures)")?;
        if self.frames.is_empty() {
            return Err("nothing kept yet".into());
        }
        std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let number = (1..).find(|n| !folder.join(format!("stream-{n}.bin")).exists()).unwrap_or(1);
        let path = folder.join(format!("stream-{number}.bin"));
        let mut file = stream::header();
        file.reserve(self.bytes);
        for (_, frame) in &self.frames {
            file.extend_from_slice(frame);
        }
        let written = path.clone();
        std::thread::spawn(move || {
            if let Err(e) = std::fs::write(&written, file) {
                println!("Recording: {} not written ({e})", written.display());
            }
        });
        Ok(path)
    }
}

#[cfg(test)]
mod tests;
