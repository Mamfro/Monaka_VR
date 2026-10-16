//! The Monaka window's own headset view: one viewer kept open from the window's start, showing
//! Monaka's home with no game (`monaka_core::home`). The window writes what the home shows into
//! its folder whenever that changes ([`Headset::show`]), so the window and the headset show the
//! same; a run links its game into the same viewer ([`Linked`]) instead of starting its own, and
//! the home stays up when the game ends.

use crate::session::{Console, Stop};
use crate::{Paths, system};
use monaka_core::home;
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

/// The folder under the runs folder that the headset view's files live in.
const FOLDER: &str = "headset";
/// How long the viewer has to end by itself once asked, before it is closed.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// The headset view, while its viewer runs.
pub struct Headset {
    child: Child,
    stop: Stop,
    folder: PathBuf,
    lines: mpsc::Receiver<String>,
    /// The home's files as last written: the manifest, the values, the games, the status.
    shown: [String; 4],
}

/// What a run needs to show its game in the headset view: its folder, the viewer's lines while
/// the run goes on (the window passes them on), and its process (not another session's viewer).
pub struct Linked {
    pub folder: PathBuf,
    pub lines: mpsc::Receiver<String>,
    pub pid: u32,
}

impl Headset {
    /// Starts the viewer on SteamVR's OpenXR with the home and no game; `wake` is called with each
    /// line it prints (the window draws again to take it).
    pub fn start(paths: &Paths, console: Console, wake: impl Fn() + Send + Clone + 'static) -> Result<Headset, String> {
        let viewer = paths.binaries.join("monaka_viewer.exe");
        if !viewer.exists() {
            return Err(format!("{} is missing. Build first: cargo build --release", viewer.display()));
        }
        let openxr = system::steamvr_openxr()?;
        let folder = paths.runs.join(FOLDER);
        std::fs::create_dir_all(&folder).map_err(|e| format!("cannot make {}: {e}", folder.display()))?;
        // No game until a run links one; the eye size is this viewer's to say.
        write(&folder, home::LINK_FILE, "")?;
        let _ = std::fs::remove_file(folder.join(home::EYE_SIZE_FILE));
        let stop = Stop::new()?;
        let mut command = Command::new(viewer);
        command
            .arg(home::HOME_ARG)
            .arg(home::RUN_FOLDER_ARG)
            .arg(&folder)
            .args(["--seconds", "86400", "--stop-event", stop.event_name()])
            .arg("--loader")
            .arg(&openxr.loader)
            .arg("--runtime")
            .arg(&openxr.runtime)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if console == Console::Hidden {
            command.creation_flags(CREATE_NO_WINDOW.0);
        }
        let mut child = command.spawn().map_err(|e| format!("cannot start the headset view: {e}"))?;
        let lines = forward(&mut child, wake);
        Ok(Headset { child, stop, folder, lines, shown: Default::default() })
    }

    /// The viewer's process.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The lines the viewer printed since last asked.
    pub fn lines(&self) -> Vec<String> {
        self.lines.try_iter().collect()
    }

    /// The viewer's exit code once it has ended.
    pub fn ended(&mut self) -> Option<i32> {
        self.child.try_wait().ok().flatten().map(|status| status.code().unwrap_or(-1))
    }

    /// Writes what the home shows: the game's manifest (its pack text; empty for none), every
    /// setting's value, the games on offer and Play's status (`monaka_core::home`). Only what
    /// changed is written.
    pub fn show(&mut self, manifest: String, values: String, games: String, status: String) {
        let texts = [manifest, values, games, status];
        for ((name, text), shown) in [home::MANIFEST_FILE, home::VALUES_FILE, home::GAMES_FILE, home::STATUS_FILE].into_iter().zip(texts).zip(&mut self.shown) {
            if *shown != text && write(&self.folder, name, &text).is_ok() {
                *shown = text;
            }
        }
    }

    /// What a run needs to show its game here; the window sends it the viewer's lines through the
    /// returned sender while the run goes on.
    pub fn link(&self) -> (Linked, mpsc::Sender<String>) {
        let (send, lines) = mpsc::channel();
        (Linked { folder: self.folder.clone(), lines, pid: self.pid() }, send)
    }
}

impl Drop for Headset {
    /// Asks the viewer to end (it ends its OpenXR session), and closes it if it does not.
    fn drop(&mut self) {
        self.stop.request();
        let asked = Instant::now();
        while asked.elapsed() < CLOSE_WAIT {
            if self.ended().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Linked {
    /// Shows the game on `channel` in the headset view, with the viewer's arguments for it.
    pub(crate) fn show_game(&self, channel: &str, args: &[String]) -> Result<(), String> {
        write(&self.folder, home::LINK_FILE, &home::link_text(channel, args))
    }

    /// The eye image size the headset view learned from the runtime at its start, once it said it.
    pub(crate) fn eye_size(&self) -> Option<(u32, u32)> {
        std::fs::read_to_string(self.folder.join(home::EYE_SIZE_FILE)).ok().and_then(|text| home::parse_eye_size(&text))
    }

    /// Lets the game go: the headset view shows the home alone again.
    pub(crate) fn let_go(&self) {
        let _ = write(&self.folder, home::LINK_FILE, "");
    }
}

/// Writes `text` as `name` in `folder` whole: through a file beside it, renamed over it, so the
/// viewer never reads half of it.
fn write(folder: &Path, name: &str, text: &str) -> Result<(), String> {
    let part = folder.join(format!("{name}.part"));
    std::fs::write(&part, text).and_then(|()| std::fs::rename(&part, folder.join(name))).map_err(|e| format!("cannot write {name}: {e}"))
}

/// The viewer's stdout and stderr lines, read on their own threads, `wake` called with each.
fn forward(child: &mut Child, wake: impl Fn() + Send + Clone + 'static) -> mpsc::Receiver<String> {
    let (send, receive) = mpsc::channel();
    let pipes: [Option<Box<dyn std::io::Read + Send>>; 2] =
        [child.stdout.take().map(|o| Box::new(o) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|e| Box::new(e) as Box<dyn std::io::Read + Send>)];
    for pipe in pipes.into_iter().flatten() {
        let (send, wake) = (send.clone(), wake.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    return;
                }
                wake();
            }
        });
    }
    receive
}

#[cfg(test)]
mod tests;
