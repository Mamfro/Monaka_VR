//! PC checks without a headset, from the same game table and option files as a play session
//! (`monaka_play harness|capture|probe`): the producer attached to the running game with a
//! stand-in for the viewer, then stopped, and its log shown.
//!
//! - **Harness**: `examples\harness.exe` (`crates\channel\examples\harness.rs`) plays a timed
//!   script of head and controller poses, fingers and gamepad in place of the viewer, saving the
//!   frames the script asks for as PNG in the run folder's `frames`.
//! - **Capture**: `examples\probe.exe` reads the channel for a few seconds with a synthetic head
//!   (and a fake controller with `--hand`), prints the pair rate and the pose records, and keeps
//!   pairs as PNG with `--save`.
//! - **Probe**: the producer alone for a few seconds, for what its own probes log.
//!
//! Most games pause without window focus; a harness script gives it (`focus <exe>`), and the
//! others take a delay to click into the game.

use crate::games::{self, Game, Values};
use crate::session::{Event, forward, log_tail, version};
use crate::{Paths, attach, system};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// What stands in for the viewer.
#[derive(Clone, Debug)]
pub enum Check {
    /// The scripted stand-in: a script file of timed poses and inputs.
    Harness { script: PathBuf },
    /// The channel probe for `seconds`, with its own arguments (`--save N` keeps that many pairs;
    /// `--hand YAW`, `--hand-sweep`, `--hand-swing`, `--pad-walk`, `--pad-hold BUTTON`,
    /// `--verify-moving` as the probe takes them).
    Capture { seconds: u64, probe_args: Vec<String> },
    /// The producer alone for `seconds`.
    Probe { seconds: u64 },
}

impl Check {
    fn kind(&self) -> &'static str {
        match self {
            Check::Harness { .. } => "harness",
            Check::Capture { .. } => "capture",
            Check::Probe { .. } => "probe",
        }
    }
}

/// Runs one check of `game` with `values` (the options as a play session would write them, plus
/// the extras) and returns the run folder. `delay_seconds` waits before attaching, for clicking
/// into the game; `eye` is written as the producer's `eye_size`.
pub fn run(game: &Game, values: &Values, paths: &Paths, check: &Check, delay_seconds: u64, eye: Option<(u32, u32)>, report: &mut dyn FnMut(Event)) -> Result<PathBuf, String> {
    let producer = game.dll(paths);
    let stand_in = match check {
        Check::Harness { script } => {
            if !script.is_file() {
                return Err(format!("the script {} is missing", script.display()));
            }
            Some(paths.binaries.join(r"examples\harness.exe"))
        }
        Check::Capture { .. } => Some(paths.binaries.join(r"examples\probe.exe")),
        Check::Probe { .. } => None,
    };
    for path in [Some(&producer), stand_in.as_ref()].into_iter().flatten() {
        if !path.exists() {
            return Err(format!("{} is missing. Build first: cargo build --release -p monaka_channel --examples", path.display()));
        }
    }
    if system::viewer_running() {
        return Err("A VR session is running (a Monaka VR viewer is open). Stop it first.".into());
    }
    // The game, or when it is not running another on the same producer (one Wolfenstein DLL serves
    // two games): with both Kingdom Comes running, a check of one is not refused for the other.
    let own: Vec<(&Game, system::Process)> = system::game_processes(game.executables, game.steam_app).into_iter().map(|p| (game, p)).collect();
    let running: Vec<(&Game, system::Process)> = if own.is_empty() {
        games::list()
            .iter()
            .filter(|g| g.producer == game.producer && g.id != game.id)
            .flat_map(|g| system::game_processes(g.executables, g.steam_app).into_iter().map(move |p| (g, p)))
            .collect()
    } else {
        own
    };
    let Some((game, process)) = running.first() else {
        return Err(format!("{} is not running. Start it first.", game.name));
    };
    if running.len() > 1 {
        return Err(format!("{} is running more than once; quit all but one.", game.name));
    }
    let pid = process.pid;
    report(Event::Info(format!("{} ({}, pid {pid})", game.name, process.executable)));

    let version = version();
    let folder = paths.runs.join(format!("{}-{}-{version}", game.run_prefix, check.kind()));
    std::fs::create_dir_all(&folder).map_err(|e| format!("cannot make the run folder {}: {e}", folder.display()))?;
    report(Event::RunFolder(folder.clone()));
    let dll = folder.join(format!("{}-{version}.dll", game.producer));
    std::fs::copy(&producer, &dll).map_err(|e| format!("cannot copy the producer: {e}"))?;
    let channel = format!(r"Local\monaka-{version}");
    let plan = games::plan(values, game, eye);
    let write = |name: &str, contents: &str| std::fs::write(folder.join(name), contents).map_err(|e| format!("cannot write {name}: {e}"));
    write("live-channel.txt", &channel)?;
    for (name, contents) in &plan.files {
        write(name, contents)?;
        report(Event::Detail(format!("{name}: {}", contents.replace('\n', " "))));
    }
    let notices = game.licence_folder(paths);
    for licence in &plan.licences {
        let _ = std::fs::copy(notices.join(licence), folder.join(licence));
    }
    if let Check::Harness { script } = check {
        std::fs::copy(script, folder.join("script.txt")).map_err(|e| format!("cannot copy the script: {e}"))?;
    }

    if delay_seconds > 0 {
        report(Event::Info(format!("Click into the game now; attaching in {delay_seconds} seconds.")));
        std::thread::sleep(Duration::from_secs(delay_seconds));
    }
    let log = folder.join(game.log);
    let outcome = attach::start(pid, &dll).map_err(|why| {
        log_tail(&log, 30, report);
        format!("Attaching failed: {why}")
    })?;
    for note in outcome.notes {
        report(Event::Detail(note));
    }
    if outcome.code != 0 {
        log_tail(&log, 30, report);
        return Err(format!("The producer refused to start (code {}); see the log.", outcome.code));
    }

    let ran = match check {
        Check::Harness { .. } => stand_in_run(stand_in.as_deref().unwrap_or(Path::new("")), &[channel.clone(), folder.join("script.txt").display().to_string(), folder.join("frames").display().to_string()], 1, report),
        Check::Capture { seconds, probe_args } => {
            let mut args = vec![channel.clone(), seconds.to_string()];
            let mut rest = probe_args.iter();
            while let Some(arg) = rest.next() {
                args.push(arg.clone());
                if arg == "--save" {
                    args.push(folder.join("pairs").display().to_string());
                    if let Some(count) = rest.next() {
                        args.push(count.clone());
                    }
                }
            }
            // Opening can lose a race for the channel's header lock while the producer publishes.
            stand_in_run(stand_in.as_deref().unwrap_or(Path::new("")), &args, 3, report)
        }
        Check::Probe { seconds } => {
            std::thread::sleep(Duration::from_secs(*seconds));
            Ok(())
        }
    };
    let stopped = if system::alive(pid) {
        match attach::stop(pid, &dll) {
            Ok(o) if o.code == 0 => Ok(()),
            Ok(o) => Err(format!("The producer did not confirm its stop (code {}); another version must not be attached.", o.code)),
            Err(why) => Err(format!("Could not stop the producer ({why}); another version must not be attached.")),
        }
    } else {
        Err(format!("{} exited during the check.", game.name))
    };
    report(Event::Info("--- producer log ---".into()));
    log_tail(&log, if matches!(check, Check::Harness { .. }) { 30 } else { usize::MAX }, report);
    ran?;
    stopped?;
    Ok(folder)
}

/// Runs the stand-in with `args`, forwarding its output, up to `attempts` times until it exits
/// with success.
fn stand_in_run(exe: &Path, args: &[String], attempts: u32, report: &mut dyn FnMut(Event)) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 1..=attempts {
        let mut child = Command::new(exe).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        let lines = forward(&mut child);
        let status = loop {
            while let Ok(line) = lines.try_recv() {
                report(Event::Detail(line));
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        while let Ok(line) = lines.recv_timeout(Duration::from_millis(200)) {
            report(Event::Detail(line));
        }
        if status.success() {
            return Ok(());
        }
        last = format!("{} stopped with exit code {} (attempt {attempt})", exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), status.code().unwrap_or(-1));
        report(Event::Warning(last.clone()));
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(last)
}
