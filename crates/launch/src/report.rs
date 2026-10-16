//! A report for the people who make Monaka when something goes wrong: one zip of a run's logs and
//! the files the launcher wrote for it, the player's settings and the window's details, with the
//! player's own folder names taken out (`monaka_core::scrub`). Captures, the DLL copies and
//! anything else that is not text stay out.

use crate::system;
use monaka_core::scrub::Scrub;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The run folder's files that go in, by extension: the text the launcher, the viewer and the
/// producer wrote (logs, option files, markers).
const TEXT: &[&str] = &["log", "txt", "on"];
/// A file longer than this goes in cut to its end (bytes): a detailed log can reach 64 MB.
const MAX_FILE: u64 = 16 << 20;

/// What a report holds.
pub struct Report<'a> {
    /// The run folder (none: no run yet).
    pub run: Option<&'a Path>,
    /// The player's settings file.
    pub settings: Option<&'a Path>,
    /// The window's details log.
    pub details: &'a str,
}

/// Writes `report` into the folder `into` as `monaka-report-<stamp>.zip`; its path.
pub fn save(report: &Report<'_>, into: &Path) -> Result<PathBuf, String> {
    let stamp = crate::session::version();
    let staging = into.join(format!(".report-{stamp}"));
    std::fs::create_dir_all(&staging).map_err(|e| format!("cannot make {}: {e}", staging.display()))?;
    let out = into.join(format!("monaka-report-{stamp}.zip"));
    let made = gather(report, &staging, &Scrub::from_env()).and_then(|()| zip(&staging, &out));
    let _ = std::fs::remove_dir_all(&staging);
    made.map(|()| out)
}

/// The newest run folder of the game whose runs start `prefix` in `runs`, if any.
pub fn latest_run(runs: &Path, prefix: &str) -> Option<PathBuf> {
    let start = format!("{prefix}-");
    let entries = std::fs::read_dir(runs).ok()?;
    // The folders' names end in a sortable time stamp.
    entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&start))).max()
}

/// Copies what the report holds into `staging`, each text scrubbed: `about.txt`, `details.txt`,
/// `settings.txt`, and the run folder's text files under its own name.
fn gather(report: &Report<'_>, staging: &Path, scrub: &Scrub) -> Result<(), String> {
    let write = |name: &Path, text: &str| std::fs::write(staging.join(name), scrub.text(text)).map_err(|e| format!("cannot write {}: {e}", name.display()));
    write(Path::new("about.txt"), &format!("Monaka {} on Windows {}\n", env!("CARGO_PKG_VERSION"), system::windows_build()))?;
    write(Path::new("details.txt"), report.details)?;
    if let Some(text) = report.settings.and_then(|file| std::fs::read_to_string(file).ok()) {
        write(Path::new("settings.txt"), &text)?;
    }
    let Some(run) = report.run else { return Ok(()) };
    let name = PathBuf::from(run.file_name().unwrap_or_default());
    std::fs::create_dir_all(staging.join(&name)).map_err(|e| format!("cannot make the run's folder in the report: {e}"))?;
    let entries = std::fs::read_dir(run).map_err(|e| format!("cannot read {}: {e}", run.display()))?;
    for file in entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()) {
        let text = file.extension().is_some_and(|x| TEXT.iter().any(|t| x.eq_ignore_ascii_case(t)));
        if let (true, Some(content)) = (text, tail(&file)) {
            write(&name.join(file.file_name().unwrap_or_default()), &content)?;
        }
    }
    Ok(())
}

/// `file`'s text, its last [`MAX_FILE`] bytes when longer (the first, cut line left out).
fn tail(file: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut opened = std::fs::File::open(file).ok()?;
    let length = opened.metadata().ok()?.len();
    let from = length.saturating_sub(MAX_FILE);
    opened.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    opened.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(if from > 0 { text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_owned()) } else { text.into_owned() })
}

/// Zips the folder `staging`'s contents as `out` with Windows' own `tar.exe` (bsdtar; `-a` makes a
/// zip from the name).
fn zip(staging: &Path, out: &Path) -> Result<(), String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let tar = system.join(r"System32\tar.exe");
    let output = Command::new(&tar)
        .arg("-a")
        .arg("-cf")
        .arg(out)
        .arg("-C")
        .arg(staging)
        .arg(".")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", tar.display()))?;
    if !output.status.success() {
        return Err(format!("cannot make the report: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
