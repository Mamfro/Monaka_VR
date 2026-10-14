//! One VR session for one game: find (or start) the game, then attach-and-view runs until the game
//! exits or a stop is asked for, reporting [`Event`]s. The one implementation of the play path:
//! the window, `monaka_play` and the `Play-*.ps1` scripts (through `monaka_play`) all run it.
//!
//! A run makes a run folder, copies the producer DLL into it (so the build output stays unlocked),
//! writes the channel name and option files beside the copy, attaches it, and runs the viewer
//! until it ends. A run that ends while the game still runs is restarted, unless runs keep failing
//! right away. The producer is always stopped before the session returns while the game runs.

use crate::games::{EyeSize, Game, Launch, Values};
use crate::run_log::{self, RunLog};
use crate::{Paths, attach, system, text};
use monaka_core::manifest::{DEBUG_MENU_ARG, DEBUG_MENU_MARKER, DIAGNOSTIC_LOGGING, DIAGNOSTIC_MARKER, LIVE_ARG, MONAKA, SETTING_LINE};
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CreateEventW, SetEvent, WaitForSingleObject};
use windows::core::HSTRING;

/// What a session reports.
#[derive(Clone, Debug)]
pub enum Event {
    /// The session's state changed.
    State(State),
    /// A line for the player.
    Info(String),
    /// Something went wrong but the session goes on, or a hint.
    Warning(String),
    /// The session ended because of this.
    Failed(String),
    /// A line from the viewer or the producer's log (shown in the details).
    Detail(String),
    /// The run folder of the current run.
    RunFolder(PathBuf),
    /// The player changed an option in the headset's Monaka menu: its id and value, to save.
    Setting(String, String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Waiting for the game (starting it, or for its menu).
    WaitingForGame,
    Attaching,
    /// The viewer is up and the producer attached.
    Running,
    Stopping,
    Stopped,
}

/// Asks a session to stop, from any thread. Also stops the viewer of the current run through a
/// named event the viewer polls (`--stop-event`).
#[derive(Clone)]
pub struct Stop {
    flag: Arc<AtomicBool>,
    event: Arc<Event32>,
}

struct Event32 {
    handle: HANDLE,
    name: String,
}

// SAFETY: an event handle may be used from any thread.
unsafe impl Send for Event32 {}
unsafe impl Sync for Event32 {}

impl Drop for Event32 {
    fn drop(&mut self) {
        // SAFETY: closes the handle this value owns.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

impl Stop {
    pub fn new() -> Result<Stop, String> {
        let name = format!(r"Local\monaka-stop-{}-{:08x}", std::process::id(), random() as u32);
        // SAFETY: a manual-reset event, initially clear, owned by the returned value.
        let handle = unsafe { CreateEventW(None, true, false, &HSTRING::from(&name)) }.map_err(|e| format!("cannot create the stop event: {e}"))?;
        Ok(Stop { flag: Arc::new(AtomicBool::new(false)), event: Arc::new(Event32 { handle, name }) })
    }

    pub fn request(&self) {
        self.flag.store(true, Ordering::Relaxed);
        // SAFETY: signals the event this value owns.
        let _ = unsafe { SetEvent(self.event.handle) };
    }

    /// A stop was asked for here, or by anything that set the stop event from outside (a tool
    /// stopping a session it cannot send Ctrl+C to): that is a stop too, not a run to restart.
    pub fn requested(&self) -> bool {
        // SAFETY: a zero-timeout check of the event this value owns.
        self.flag.load(Ordering::Relaxed) || unsafe { WaitForSingleObject(self.event.handle, 0) } == WAIT_OBJECT_0
    }

    /// Sleeps up to `duration`, waking early on a stop. Returns whether a stop was asked for.
    fn sleep(&self, duration: Duration) -> bool {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            if self.requested() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100).min(until - Instant::now()));
        }
        self.requested()
    }
}

fn random() -> u128 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(std::process::id()) as u128
}

/// A unique version stamp for a run (`YYYYMMDDThhmmssfffffff-xxxxxxxx`, sortable like the
/// PowerShell launchers' folders).
pub(crate) fn version() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let (days, rem) = (now.as_secs() / 86_400, now.as_secs() % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}{:07}-{:08x}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        now.subsec_nanos() / 100,
        random() as u32
    )
}

/// The calendar date of a day count since the Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// How the viewer is shown.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Console {
    /// The viewer gets no console window (the GUI).
    Hidden,
    /// The viewer shares the caller's console (the command line).
    Shared,
}

/// What shows the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    /// The headset, through SteamVR's OpenXR.
    Headset,
    /// The 2D window on the desktop, with the debug menu and its panels: no headset
    /// ([`monaka_core::manifest::DESKTOP_ARG`]).
    Desktop,
}

/// Runs a session to its end. Blocks; call it on its own thread from a GUI.
pub fn run(game: &Game, values: &Values, paths: &Paths, console: Console, shows: Shows, stop: &Stop, report: &mut dyn FnMut(Event)) {
    match session(game, values, paths, console, shows, stop, report) {
        Ok(()) => {}
        Err(failure) => report(Event::Failed(failure)),
    }
    report(Event::State(State::Stopped));
}

fn session(game: &Game, values: &Values, paths: &Paths, console: Console, shows: Shows, stop: &Stop, report: &mut dyn FnMut(Event)) -> Result<(), String> {
    let viewer = paths.binaries.join("monaka_viewer.exe");
    let producer = game.dll(paths);
    for path in [&viewer, &producer] {
        if !path.exists() {
            let fix = if game.removable() && *path == producer { "Add the game pack again." } else { "Build first: cargo build --release" };
            return Err(format!("{} is missing. {fix}", path.display()));
        }
    }
    if system::viewer_running() {
        return Err("Another VR session is already running (a Monaka VR viewer is open). Stop it first.".into());
    }
    // The 2D window needs no headset: no OpenXR runtime.
    let openxr = match shows {
        Shows::Headset if !system::steamvr_running() => return Err("Start SteamVR and connect the headset first.".into()),
        Shows::Headset => Some(system::steamvr_openxr()?),
        Shows::Desktop => None,
    };

    report(Event::State(State::WaitingForGame));
    let mut restarts = 0;
    loop {
        let Some(pid) = find_game(game, values, stop, report)? else { return Ok(()) };
        let joined = Instant::now();
        play(game, values, paths, console, stop, report, &viewer, &producer, openxr.as_ref(), pid)?;
        // A game joined at its start that ends at once may have started itself again (through
        // Steam, or a launcher's own copy): it is waited for again, a few times.
        let again = game.launch == Launch::AtStart && !stop.requested() && !system::alive(pid) && joined.elapsed() < AT_START_RESTART && restarts < 3;
        if !again {
            return Ok(());
        }
        restarts += 1;
        report(Event::Info(format!("{} ended right after it started; waiting for it to start again.", game.name)));
        report(Event::State(State::WaitingForGame));
    }
}

/// A game joined at its start that ends within this is waited for again ([`Launch::AtStart`]).
const AT_START_RESTART: Duration = Duration::from_secs(30);

/// After the game is gone, how long the viewer has to end by itself before it is closed.
const GAME_GONE_GRACE: Duration = Duration::from_secs(5);

/// How long the viewer waits for the first frame of a game joined as it starts.
const AT_START_PRODUCER_WAIT: Duration = Duration::from_secs(600);

/// One game process played until it exits or a stop is asked for.
#[allow(clippy::too_many_arguments)]
fn play(
    game: &Game,
    values: &Values,
    paths: &Paths,
    console: Console,
    stop: &Stop,
    report: &mut dyn FnMut(Event),
    viewer: &Path,
    producer: &Path,
    openxr: Option<&system::OpenXr>,
    pid: u32,
) -> Result<(), String> {
    // A 32-bit game takes the pack's 32-bit twin.
    let producer = match system::is_32_bit(pid) {
        Some(true) => game.dll_for(paths, true),
        _ => producer.to_path_buf(),
    };
    if !producer.exists() {
        return Err(format!("{} is missing: {} is a 32-bit game. Build first: cargo build --release --target i686-pc-windows-msvc", producer.display(), game.name));
    }
    // An unreadable command line is let through: only arguments it is seen to hold are refused.
    if let Some(line) = system::command_line(pid).map(|l| l.to_ascii_lowercase())
        && let Some((_, why)) = game.refused_arguments.iter().find(|(argument, _)| line.contains(&argument.to_ascii_lowercase()))
    {
        return Err((*why).to_owned());
    }

    let eye = match crate::games::eye_size(values, game) {
        EyeSize::GameOwn => None,
        EyeSize::Fixed(w, h) => Some((w, h)),
        // The 2D window: no headset to ask, so the game keeps its own resolution.
        EyeSize::Headset => openxr.and_then(|openxr| {
            let size = headset_eye_size(viewer, openxr, console);
            match size {
                Some((w, h)) => report(Event::Info(format!("Headset eye size {w}x{h}"))),
                None => report(Event::Warning("Headset eye size unavailable; the game keeps its own resolution.".into())),
            }
            size
        }),
    };

    report(Event::Info(match openxr {
        Some(_) => "VR is starting. Forward follows the headset: use its own recentre control.".into(),
        None => "The 2D window is starting: the game as the headset would get it, the debug menu and its panels in a window. Click it; the game keeps focus.".into(),
    }));
    let mut quick_failures = 0;
    while system::alive(pid) && !stop.requested() {
        let started = Instant::now();
        let ended = one_run(game, values, paths, viewer, &producer, openxr, pid, eye, console, stop, report);
        if stop.requested() || !system::alive(pid) {
            if let Err(RunEnd::Fatal(why) | RunEnd::Retry(why)) = ended {
                report(Event::Warning(why));
            }
            break;
        }
        match ended {
            Err(RunEnd::Fatal(why)) => return Err(why),
            Err(RunEnd::Retry(why)) => report(Event::Warning(format!("VR run ended: {why}; restarting."))),
            Ok(()) => report(Event::Warning("VR run ended; restarting.".into())),
        }
        quick_failures = if started.elapsed() > Duration::from_secs(30) { 0 } else { quick_failures + 1 };
        if quick_failures >= 3 {
            return Err("VR keeps stopping right after it starts; see the details and the log.".into());
        }
        if stop.sleep(Duration::from_secs(3)) {
            break;
        }
    }
    if !system::alive(pid) {
        report(Event::Info(format!("{} has exited; VR stopped.", game.name)));
    }
    Ok(())
}

/// The moment a game started after VR can be attached ([`Launch::AtStart`]): the player starts
/// it, and it is joined as soon as its process can take the producer, before it makes anything.
fn join_at_start(executables: &[&str], steam_app: u32, stop: &Stop, report: &mut dyn FnMut(Event)) -> Result<Option<u32>, String> {
    report(Event::Info(format!("Start {} now: Monaka joins it as it starts.", executables.join(" or "))));
    loop {
        // The cheap check first; the Steam folder check once one is named so.
        if !system::processes_named(executables).is_empty()
            && let Some(process) = system::game_processes(executables, steam_app).first()
        {
            let seen = Instant::now();
            while system::alive(process.pid) && seen.elapsed() < Duration::from_secs(10) {
                if attach::ready(process.pid) {
                    report(Event::Info(format!("{} started; joining it.", process.executable)));
                    return Ok(Some(process.pid));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        if stop.requested() {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The game's process: already running, or started through Steam and given time to reach its menu
/// (unless its `launch` option is off; `settle_seconds` overrides the wait).
fn find_game(game: &Game, values: &Values, stop: &Stop, report: &mut dyn FnMut(Event)) -> Result<Option<u32>, String> {
    let runs_as = game.runs_as(values);
    if runs_as.is_empty() {
        return Err(format!("Pick the game in {}'s Game option first.", game.name));
    }
    let executables: Vec<&str> = runs_as.iter().map(String::as_str).collect();
    let running = system::game_processes(&executables, game.steam_app);
    if let Some(process) = running.first() {
        if game.launch == Launch::AtStart {
            report(Event::Info(format!("{} is running already: joining it now. What it made before stays unknown; start VR first, then the game, to see it all.", process.executable)));
        }
        return Ok(Some(process.pid));
    }
    if game.launch == Launch::AtStart {
        return join_at_start(&executables, game.steam_app, stop, report);
    }
    let Launch::Auto { settle_seconds } = game.launch else {
        return Err(format!("{} is not running. Start it from Steam first.", game.name));
    };
    if game.option("launch").is_some() && !values.switch(game, "launch") {
        return Err(format!("{} is not running.", game.name));
    }
    let settle_seconds = if game.option("settle_seconds").is_some() { values.number(game, "settle_seconds").round() as u64 } else { settle_seconds };
    report(Event::Info(format!("Starting {} through Steam...", game.name)));
    system::launch_steam_game(game.steam_app)?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let pid = loop {
        if let Some(process) = system::game_processes(&executables, game.steam_app).first() {
            break process.pid;
        }
        if Instant::now() > deadline {
            return Err(format!("{} did not start within three minutes.", game.name));
        }
        if stop.sleep(Duration::from_secs(1)) {
            return Ok(None);
        }
    };
    report(Event::Info(format!("Game started; giving it {settle_seconds} seconds to reach the menu.")));
    if stop.sleep(Duration::from_secs(settle_seconds)) {
        return Ok(None);
    }
    Ok(Some(pid))
}

fn headset_eye_size(viewer: &Path, openxr: &system::OpenXr, console: Console) -> Option<(u32, u32)> {
    let mut command = Command::new(viewer);
    command.arg("--info").arg("--loader").arg(&openxr.loader).arg("--runtime").arg(&openxr.runtime);
    if console == Console::Hidden {
        command.creation_flags(CREATE_NO_WINDOW.0);
    }
    let output = command.output().ok()?;
    text::eye_image_size(&String::from_utf8_lossy(&output.stdout))
}

/// How a run ended badly.
enum RunEnd {
    /// A restart would only repeat it (the producer refused this game build, or did not stop).
    Fatal(String),
    /// The viewer ended; a new run may work.
    Retry(String),
}

type RunResult = Result<(), RunEnd>;

fn fatal(why: String) -> RunEnd {
    RunEnd::Fatal(why)
}

#[allow(clippy::too_many_arguments)]
fn one_run(
    game: &Game,
    values: &Values,
    paths: &Paths,
    viewer: &Path,
    producer: &Path,
    openxr: Option<&system::OpenXr>,
    pid: u32,
    eye: Option<(u32, u32)>,
    console: Console,
    stop: &Stop,
    report: &mut dyn FnMut(Event),
) -> RunResult {
    // The installed launcher keeps a game's newest runs; a build tree keeps them all for research.
    if paths.dev_games.is_none() {
        run_log::prune_runs(&paths.runs, game.run_prefix, run_log::KEPT_RUNS - 1);
    }
    let version = version();
    let folder = paths.runs.join(format!("{}-{version}", game.run_prefix));
    std::fs::create_dir_all(&folder).map_err(|e| fatal(format!("cannot make the run folder {}: {e}", folder.display())))?;
    let mut plan = crate::games::plan(values, game, eye);
    // A build tree always has the debug menu; an install only once the player turned it on.
    if paths.dev_games.is_some() && !plan.viewer_args.iter().any(|a| a == DEBUG_MENU_ARG) {
        plan.viewer_args.push(DEBUG_MENU_ARG.to_owned());
    }
    // A game joined as it starts shows its first frame only after its start-up (videos, loading).
    if game.launch == Launch::AtStart {
        plan.viewer_args.extend(["--producer-wait".to_owned(), AT_START_PRODUCER_WAIT.as_secs().to_string()]);
    }
    if !plan.live.is_empty() {
        let live = plan.live.join(",");
        plan.viewer_args.extend([LIVE_ARG.to_owned(), live]);
    }
    let mut monaka_log = RunLog::create(&folder);
    for line in header(game, producer, pid, openxr, eye, &plan) {
        monaka_log.line(&line);
    }
    let report = &mut |event: Event| {
        monaka_log.event(&event);
        report(event);
    };
    report(Event::RunFolder(folder.clone()));
    let dll = folder.join(format!("{}-{version}.dll", game.producer));
    std::fs::copy(producer, &dll).map_err(|e| fatal(format!("cannot copy the producer: {e}")))?;
    let channel = format!(r"Local\monaka-{version}");
    let write = |name: &str, contents: &str| std::fs::write(folder.join(name), contents).map_err(|e| fatal(format!("cannot write {name}: {e}")));
    write("live-channel.txt", &channel)?;
    for (name, contents) in &plan.files {
        write(name, contents)?;
    }
    if values.switch(&MONAKA, DIAGNOSTIC_LOGGING.id) {
        write(DIAGNOSTIC_MARKER, "")?;
    }
    // The debug menu's tools in the game (the probe console) come with the menu.
    if plan.viewer_args.iter().any(|a| a == DEBUG_MENU_ARG) {
        write(DEBUG_MENU_MARKER, "")?;
    }
    let notices = game.licence_folder(paths);
    for licence in &plan.licences {
        let _ = std::fs::copy(notices.join(licence), folder.join(licence));
    }
    for hint in &plan.hints {
        report(Event::Warning(hint.clone()));
    }

    report(Event::State(State::Attaching));
    let log = folder.join(game.log);
    let outcome = attach::start(pid, &dll).map_err(|why| {
        log_tail(&log, 20, report);
        fatal(format!("Attaching failed: {why}"))
    })?;
    for note in outcome.notes {
        report(Event::Detail(note));
    }
    if outcome.code != 0 {
        log_tail(&log, 20, report);
        return Err(fatal(format!(
            "The mod refused to start in this game (code {}). The game may have updated to a version Monaka VR doesn't know yet; see the log.",
            outcome.code
        )));
    }

    let viewed = view(viewer, openxr, &channel, &plan.viewer_args, &folder.join("captures"), pid, console, stop, report);
    // Always stop the producer while the game runs, whatever the viewer did.
    let stopped = if system::alive(pid) {
        report(Event::State(State::Stopping));
        match attach::stop(pid, &dll) {
            Ok(o) if o.code == 0 => Ok(()),
            Ok(o) => Err(fatal(format!("The producer did not confirm its stop (code {}); restart the game before trying again.", o.code))),
            Err(why) => Err(fatal(format!("Could not stop the producer ({why}); restart the game before trying again."))),
        }
    } else {
        Ok(())
    };
    stopped?;
    viewed.map_err(RunEnd::Retry)
}

/// What a run runs where, at the top of its log: the first thing to know about a player's report.
fn header(game: &Game, producer: &Path, pid: u32, openxr: Option<&system::OpenXr>, eye: Option<(u32, u32)>, plan: &crate::games::Plan) -> Vec<String> {
    let mut lines = vec![
        format!("Monaka {} (pack contract {}) on Windows {}", env!("CARGO_PKG_VERSION"), monaka_core::pack::CONTRACT, system::windows_build()),
        format!("game: {} ({}), process {pid} {}", game.name, game.id, system::process_path(pid).map_or_else(|| "(path unreadable)".into(), |p| p.display().to_string())),
        format!("producer: {} sha256 {}", producer.display(), file_sha256(producer).unwrap_or_else(|| "(unreadable)".into())),
        format!("pack: {}", game.credit()),
        format!("OpenXR runtime: {}", openxr.map_or_else(|| "none (the 2D window)".into(), |o| o.runtime.display().to_string())),
        format!("eye size: {}", eye.map_or_else(|| "the game's own".into(), |(w, h)| format!("{w}x{h}"))),
    ];
    for (name, contents) in &plan.files {
        lines.push(format!("{name}:"));
        lines.extend(contents.lines().map(|line| format!("  {line}")));
    }
    lines
}

fn file_sha256(path: &Path) -> Option<String> {
    let mut hash = monaka_core::sha256::Sha256::default();
    hash.update(&std::fs::read(path).ok()?);
    Some(monaka_core::sha256::hex(&hash.finish()))
}

/// Runs the viewer until it ends, a stop is asked for or the game (`pid`) is gone, forwarding its
/// output: in the headset through `openxr`, else in the 2D window. What it shows is saved into
/// `captures` when asked for there (Start and Back held, or the debug menu's Save capture).
#[allow(clippy::too_many_arguments)]
fn view(
    viewer: &Path,
    openxr: Option<&system::OpenXr>,
    channel: &str,
    extra: &[String],
    captures: &Path,
    pid: u32,
    console: Console,
    stop: &Stop,
    report: &mut dyn FnMut(Event),
) -> Result<(), String> {
    let mut command = Command::new(viewer);
    command.args(["--gpu-map", channel, "--seconds", "86400", "--stop-event", &stop.event.name]);
    match openxr {
        Some(openxr) => command.arg("--loader").arg(&openxr.loader).arg("--runtime").arg(&openxr.runtime),
        None => command.arg(monaka_core::manifest::DESKTOP_ARG),
    };
    command
        .arg("--captures")
        .arg(captures)
        .args(extra)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if console == Console::Hidden {
        command.creation_flags(CREATE_NO_WINDOW.0);
    }
    let mut child = command.spawn().map_err(|e| format!("cannot start the viewer: {e}"))?;
    let lines = forward(&mut child);
    report(Event::State(State::Running));
    let mut stop_asked_at: Option<Instant> = None;
    let mut game_gone_at: Option<Instant> = None;
    let status = loop {
        while let Ok(line) = lines.try_recv() {
            report(viewer_line(line));
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if stop.requested() {
            let asked = *stop_asked_at.get_or_insert_with(Instant::now);
            // The viewer ends its OpenXR session within a frame; a stuck one is killed.
            if asked.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
            }
        }
        // A game gone before its first frame leaves the viewer waiting for one: the run is over.
        if !system::alive(pid) && game_gone_at.get_or_insert_with(Instant::now).elapsed() > GAME_GONE_GRACE {
            let _ = child.kill();
            let _ = child.wait();
            report(Event::Info("The game ended; the viewer is closed.".into()));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    while let Ok(line) = lines.recv_timeout(Duration::from_millis(200)) {
        report(viewer_line(line));
    }
    // The player closed the 2D window: that is Stop VR, not a run to start again.
    if status.code() == Some(monaka_core::manifest::CLOSED_BY_PLAYER) {
        stop.request();
    }
    if status.success() || stop.requested() { Ok(()) } else { Err(format!("the viewer stopped with exit code {}", status.code().unwrap_or(-1))) }
}

/// The viewer's stdout and stderr lines, read on their own threads.
/// A viewer line as the player hears of it: a setting changed in the headset, or a detail.
fn viewer_line(line: String) -> Event {
    let setting = line.strip_prefix(SETTING_LINE).and_then(|rest| rest.trim().split_once('='));
    match setting {
        Some((id, value)) => Event::Setting(id.to_owned(), value.to_owned()),
        None => Event::Detail(line),
    }
}

pub(crate) fn forward(child: &mut Child) -> mpsc::Receiver<String> {
    let (send, receive) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        let send = send.clone();
        std::thread::spawn(move || BufReader::new(out).lines().map_while(Result::ok).for_each(|l| drop(send.send(l))));
    }
    if let Some(err) = child.stderr.take() {
        std::thread::spawn(move || BufReader::new(err).lines().map_while(Result::ok).for_each(|l| drop(send.send(l))));
    }
    receive
}

/// The last `count` lines of the producer's log, as details.
pub(crate) fn log_tail(log: &Path, count: usize, report: &mut dyn FnMut(Event)) {
    if let Ok(text) = std::fs::read_to_string(log) {
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines[lines.len().saturating_sub(count)..] {
            report(Event::Detail((*line).to_owned()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewer_setting_line_is_a_setting() {
        assert!(matches!(viewer_line(format!("{SETTING_LINE} menu_reach_depth=20")), Event::Setting(id, value) if id == "menu_reach_depth" && value == "20"));
        assert!(matches!(viewer_line("GPU: something".into()), Event::Detail(_)));
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_367), (2025, 10, 6));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn versions_sort_and_differ() {
        let (a, b) = (version(), version());
        assert_eq!(a.len(), "20261006T1122330000000-1a2b3c4d".len());
        assert_ne!(a, b);
    }
}
