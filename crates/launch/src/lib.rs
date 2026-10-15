//! Playing a game in VR from one call, as a library for the `monaka` window and the `monaka_play`
//! command line (which the `tools\Play-*.ps1` scripts run).
//!
//! - [`games`]: every game the launcher offers, its options, and what those become.
//! - [`packs`]: game packs read from their DLLs, and the games folder they are added to.
//! - [`session`]: one VR session (find or start the game, attach, view, restart, stop).
//! - [`run_log`]: each run's `monaka.log` (what ran where, and what the run reported), and
//!   pruning old run folders.
//! - [`attach`]: injecting a producer and calling its `MonakaStart`/`MonakaStop` (also `monaka_loader`).
//! - [`system`]: processes and their command lines, SteamVR's OpenXR files, Steam libraries, the
//!   Documents folder.
//! - [`settings`]: the player's chosen options, kept between runs.
//! - [`check`]: the PC checks without a headset (harness, capture, probe) on the same table.
//!
//! This is an edge crate: the Win32 calls live in `attach`, `system` and the stop event in
//! `session`.

pub mod attach;
pub mod check;
pub mod games;
pub mod headset;
pub mod packs;
pub mod run_log;
pub mod session;
pub mod settings;
pub mod system;
pub mod text;

use std::path::{Path, PathBuf};

/// Where the launcher finds its binaries and puts its run folders.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `monaka_viewer.exe` and the producer DLLs: beside the running executable.
    pub binaries: PathBuf,
    /// Run folders: the repository's `runs` in a build tree, else `%LOCALAPPDATA%\monaka\runs`.
    pub runs: PathBuf,
    /// Third-party notices copied into run folders (Monaka's own games; a pack brings its own).
    pub licences: PathBuf,
    /// The games folder: game packs the player added, one folder each. `%LOCALAPPDATA%\monaka\games`,
    /// or `MONAKA_GAMES` when set.
    pub packs: PathBuf,
    /// In a build tree, the dev list: `local\dev-games.txt` in the repository (git-ignored), naming
    /// the producer DLLs built here that the launcher offers as packs, one per line.
    pub dev_games: Option<PathBuf>,
}

impl Paths {
    /// Paths for the running executable.
    pub fn locate() -> Paths {
        let exe = std::env::current_exe().unwrap_or_default();
        Paths::for_binaries(exe.parent().unwrap_or(Path::new(".")))
    }

    /// Paths for binaries in `binaries`: a cargo build tree (`<repo>\target\release`) keeps runs
    /// in the repository, as the PowerShell launchers do; anything else (an unzipped release) in
    /// the user's local app data.
    pub fn for_binaries(binaries: &Path) -> Paths {
        let profile = binaries.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase());
        let target = binaries.parent().filter(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("target")));
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| binaries.to_owned());
        let packs = std::env::var_os("MONAKA_GAMES").map(PathBuf::from).unwrap_or_else(|| local.join(r"monaka\games"));
        match (profile.as_deref(), target.and_then(Path::parent)) {
            (Some("release" | "debug"), Some(repo)) => Paths {
                binaries: binaries.to_owned(),
                runs: repo.join("runs"),
                licences: repo.join("licenses"),
                packs,
                dev_games: Some(repo.join(r"local\dev-games.txt")),
            },
            _ => Paths { binaries: binaries.to_owned(), runs: local.join(r"monaka\runs"), licences: binaries.join("licenses"), packs, dev_games: None },
        }
    }
}

#[cfg(test)]
mod tests;
