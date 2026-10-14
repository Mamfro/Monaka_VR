//! A run's own log, `monaka.log` in its run folder: a header saying what ran where (Monaka, Windows,
//! the game, the producer, the runtime, the options), then everything the run reported, the
//! viewer's lines among them. With the producer's log beside it, it is what a player sends when
//! something goes wrong, so the player's folder names are taken out ([`Scrub`]).
//!
//! The installed launcher keeps the newest [`KEPT_RUNS`] run folders of each game
//! ([`prune_runs`]).

use crate::session::Event;
use monaka_core::scrub::Scrub;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

pub const FILE: &str = "monaka.log";

/// How many run folders of one game the installed launcher keeps.
pub const KEPT_RUNS: usize = 20;

pub struct RunLog {
    file: Option<File>,
    scrub: Scrub,
    started: Instant,
}

impl RunLog {
    /// The log in `folder`; a log that cannot be made leaves the run going without one.
    pub fn create(folder: &Path) -> Self {
        Self { file: File::create(folder.join(FILE)).ok(), scrub: Scrub::from_env(), started: Instant::now() }
    }

    pub fn line(&mut self, text: &str) {
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{:>9.3} {}", self.started.elapsed().as_secs_f64(), self.scrub.text(text));
        }
    }

    /// One line per event, as the details show it.
    pub fn event(&mut self, event: &Event) {
        match event {
            Event::State(state) => self.line(&format!("state: {state:?}")),
            Event::Info(text) => self.line(text),
            Event::Warning(text) => self.line(&format!("warning: {text}")),
            Event::Failed(text) => self.line(&format!("failed: {text}")),
            Event::Detail(text) => self.line(&format!("| {text}")),
            Event::RunFolder(folder) => self.line(&format!("run folder: {}", folder.display())),
            Event::Setting(id, value) => self.line(&format!("set in the headset: {id}={value}")),
        }
    }
}

/// Deletes the run folders of the game whose runs are named `<prefix>-<version>` in `runs`, all but
/// the newest `keep` (versions sort by time). A folder whose producer copy is still loaded in a game
/// cannot lose its DLL, and is left whole.
pub fn prune_runs(runs: &Path, prefix: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(runs) else { return };
    let start = format!("{prefix}-");
    let mut folders: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|entry| entry.file_name().to_str().and_then(|name| name.strip_prefix(&start)).is_some_and(|version| version.starts_with(|c: char| c.is_ascii_digit())))
        .map(|entry| entry.path())
        .collect();
    folders.sort();
    let old = folders.len().saturating_sub(keep);
    for folder in &folders[..old] {
        if remove_dlls(folder) {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

/// Removes the DLLs in `folder`; whether none is left.
fn remove_dlls(folder: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(folder) else { return false };
    entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll"))).all(|dll| std::fs::remove_file(dll).is_ok())
}

#[cfg(test)]
mod tests;
