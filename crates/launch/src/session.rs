//! One VR session for one game: find (or start) the game, then attach-and-view runs until the game
//! exits or a stop is asked for, reporting [`Event`]s. The one implementation of the play path:
//! the window, `monaka_play` and the `Play-*.ps1` scripts (through `monaka_play`) all run it.
//!
//! A run makes a run folder, copies the producer DLL into it (so the build output stays unlocked),
//! writes the channel name and option files beside the copy, attaches it, and runs the viewer
//! until it ends; or, with the window's headset view open ([`crate::headset`]), links the game into
//! that viewer until the game ends or a stop is asked for. A run that ends while the game still runs is restarted, unless runs keep failing
//! right away. The producer is always stopped before the session returns while the game runs.

use crate::games::{EyeSize, Game, Launch, Values};
use crate::run_log::{self, RunLog};
use crate::headset::Linked;
use crate::{Paths, attach, system, text};
use monaka_contract::host;
use monaka_contract::protocol::LIVE_CHANNEL_FILE;
use monaka_contract::manifest::{DEBUG_MENU_MARKER, DIAGNOSTIC_LOGGING, DIAGNOSTIC_MARKER, MONAKA};
use monaka_contract::pack;
use monaka_contract::status::{REJECTED_LINE, code};
use monaka_core::home::{self, APPLY_LINE, DEBUG_MENU_ARG, FAILURE_LINE, SETTING_LINE};
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
    /// The player changed an option in the headset's home: its id and value, to save.
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

    /// The named event's name, for the viewer's `--stop-event`.
    pub(crate) fn event_name(&self) -> &str {
        &self.event.name
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
    /// ([`home::DESKTOP_ARG`]).
    Desktop,
}

/// Runs a session to its end. Blocks; call it on its own thread from a GUI. `linked`: the window's
/// headset view, where a run in the headset shows its game.
pub fn run(game: &Game, values: &Values, paths: &Paths, console: Console, shows: Shows, linked: Option<Linked>, stop: &Stop, report: &mut dyn FnMut(Event)) {
    match session(game, values, paths, console, shows, linked.as_ref(), stop, report) {
        Ok(()) => {}
        Err(failure) => report(Event::Failed(failure)),
    }
    report(Event::State(State::Stopped));
}

#[allow(clippy::too_many_arguments)]
fn session(game: &Game, values: &Values, paths: &Paths, console: Console, shows: Shows, linked: Option<&Linked>, stop: &Stop, report: &mut dyn FnMut(Event)) -> Result<(), String> {
    let viewer = paths.binaries.join("monaka_viewer.exe");
    let producer = game.dll(paths);
    for path in [&viewer, &producer] {
        if !path.exists() {
            let fix = if game.removable() && *path == producer { "Add the game pack again." } else { "Build first: cargo build --release" };
            return Err(format!("{} is missing. {fix}", path.display()));
        }
    }
    if system::viewer_running(linked.map(|l| l.pid)) {
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
        // Play starts the game once; a restart is its own.
        let Some(pid) = find_game(game, values, restarts == 0, stop, report)? else { return Ok(()) };
        let joined = Instant::now();
        // The headset view shows a run in the headset; the 2D window is a viewer of its own.
        let linked = linked.filter(|_| openxr.is_some());
        play(game, values, paths, console, stop, report, &viewer, &producer, openxr.as_ref(), linked, pid)?;
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
/// When VR starts in the headset, the game's window is brought to the front at these times after
/// the viewer starts (SteamVR may take the front as its session begins), unless it is there.
const FOCUS_GAME_AT: [Duration; 2] = [Duration::from_millis(1000), Duration::from_millis(4000)];

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
    linked: Option<&Linked>,
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
        // The headset view knows it already; else a short-lived viewer asks the runtime.
        EyeSize::Headset => openxr.and_then(|openxr| {
            let size = linked.and_then(Linked::eye_size).or_else(|| headset_eye_size(viewer, openxr, console));
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
        let ended = one_run(game, values, paths, viewer, &producer, openxr, linked, pid, eye, console, stop, report);
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
/// it (or Play did), and it is joined as soon as its process can take the producer, before it
/// makes anything.
fn join_at_start(executables: &[&str], steam_app: u32, stop: &Stop, report: &mut dyn FnMut(Event)) -> Result<Option<u32>, String> {
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
fn find_game(game: &Game, values: &Values, start: bool, stop: &Stop, report: &mut dyn FnMut(Event)) -> Result<Option<u32>, String> {
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
        // One Steam game installed here is started by Play (`start`), the wait for it already on.
        if start && !game.any_game() && system::steam_installed(&system::steam_libraries(), game.steam_app) {
            report(Event::Info(format!("Starting {} through Steam: Monaka joins it as it starts.", game.name)));
            system::launch_steam_game(game.steam_app)?;
        } else {
            report(Event::Info(format!("Start {} now: Monaka joins it as it starts.", executables.join(" or "))));
        }
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
    linked: Option<&Linked>,
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
    // A build tree always has developer mode, as it has the debug menu.
    let mut values = values.clone();
    if paths.dev_games.is_some() {
        values.set(monaka_contract::manifest::DEV_MODE.id, "1");
    }
    let values = &values;
    let mut plan = crate::games::plan(values, game, eye);
    // A build tree always has the debug menu; an install only once the player turned it on
    // (the plan gives the viewer the argument then).
    if paths.dev_games.is_some() && !plan.viewer_args.iter().any(|a| a == DEBUG_MENU_ARG) {
        plan.viewer_args.push(DEBUG_MENU_ARG.to_owned());
    }
    // A game joined as it starts shows its first frame only after its start-up (videos, loading).
    if game.launch == Launch::AtStart {
        plan.viewer_args.extend(["--producer-wait".to_owned(), AT_START_PRODUCER_WAIT.as_secs().to_string()]);
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
    let mut dll = folder.join(format!("{}-{version}.dll", game.producer));
    std::fs::copy(producer, &dll).map_err(|e| fatal(format!("cannot copy the producer: {e}")))?;
    let channel = format!(r"Local\monaka-{version}");
    let write = |name: &str, contents: &str| std::fs::write(folder.join(name), contents).map_err(|e| fatal(format!("cannot write {name}: {e}")));
    write(LIVE_CHANNEL_FILE, &channel)?;
    write(host::FEATURES_FILE, &host::features_text())?;
    // What the headset's home shows: the game, every setting's value, the games on offer.
    let about = &game.pack().about;
    write(home::MANIFEST_FILE, &pack::text(std::slice::from_ref(game.manifest), &about.author, &about.source, &about.version, &pack::Needs::NONE))?;
    write(home::VALUES_FILE, &home::values_text(&[game.manifest, &MONAKA], values))?;
    let listed: Vec<home::Listed> = crate::games::list()
        .iter()
        .map(|g| home::Listed { id: g.id.to_owned(), name: g.name.to_owned(), status: g.status.label().to_owned(), current: g.id == game.id, in_vr: g.id == game.id, running: g.id == game.id })
        .collect();
    write(home::GAMES_FILE, &home::games_text(&listed))?;
    plan.viewer_args.extend([home::RUN_FOLDER_ARG.to_owned(), folder.display().to_string()]);
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
        return Err(fatal(refusal(&log, outcome.code)));
    }

    let viewed = {
        // Settings applied in the headset: the producer started again with them, in the same run
        // folder on the same channel (a fresh copy of its DLL), the game and the viewer running on.
        let mut restarts = 0;
        let mut apply = |values: &Values| -> Result<(), String> {
            let plan = crate::games::plan(values, game, eye);
            for (name, contents) in &plan.files {
                std::fs::write(folder.join(name), contents).map_err(|e| format!("cannot write {name}: {e}"))?;
            }
            let _ = std::fs::write(folder.join(home::VALUES_FILE), home::values_text(&[game.manifest, &MONAKA], values));
            match attach::stop(pid, &dll) {
                Ok(o) if o.code == 0 => {}
                Ok(o) => return Err(format!("the producer did not confirm its stop (code {}); restart the game", o.code)),
                Err(why) => return Err(format!("could not stop the producer ({why}); restart the game")),
            }
            restarts += 1;
            // A channel of its own: the viewer takes it from the run folder, and can never mistake
            // the producer that is stopping for the one that replaces it.
            let next_channel = format!(r"Local\monaka-{version}-{restarts}");
            std::fs::write(folder.join(LIVE_CHANNEL_FILE), &next_channel).map_err(|e| format!("cannot write {LIVE_CHANNEL_FILE}: {e}"))?;
            let next = folder.join(format!("{}-{version}-{restarts}.dll", game.producer));
            std::fs::copy(producer, &next).map_err(|e| format!("cannot copy the producer: {e}"))?;
            dll = next;
            let outcome = attach::start(pid, &dll).map_err(|why| format!("attaching again failed: {why}"))?;
            if outcome.code != 0 {
                return Err(format!("the mod refused to start again: {}", refusal(&log, outcome.code)));
            }
            Ok(())
        };
        let captures = folder.join("captures");
        match linked {
            Some(linked) => view_linked(linked, &channel, &plan.viewer_args, &captures, pid, stop, values.clone(), &mut apply, report),
            None => view(viewer, openxr, &channel, &plan.viewer_args, &captures, pid, console, stop, values.clone(), &mut apply, report),
        }
    };
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
        format!("Monaka {} (pack contract {}) on Windows {}", env!("CARGO_PKG_VERSION"), monaka_contract::pack::CONTRACT, system::windows_build()),
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
    mut values: Values,
    apply: &mut dyn FnMut(&Values) -> Result<(), String>,
    report: &mut dyn FnMut(Event),
) -> Result<(), String> {
    let mut command = Command::new(viewer);
    command.args(["--gpu-map", channel, "--seconds", "86400", "--stop-event", stop.event_name()]);
    match openxr {
        Some(openxr) => command.arg("--loader").arg(&openxr.loader).arg("--runtime").arg(&openxr.runtime),
        None => command.arg(home::DESKTOP_ARG),
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
    let started = Instant::now();
    // In the headset only: the 2D window is where a desktop run is played.
    let mut focus_at = if openxr.is_some() { &FOCUS_GAME_AT[..] } else { &[] };
    let mut stop_asked_at: Option<Instant> = None;
    let mut game_gone_at: Option<Instant> = None;
    // Why the viewer said the run cannot go on, if it did.
    let mut failed: Option<String> = None;
    let status = loop {
        while let Ok(line) = lines.try_recv() {
            failed = failure(&line).or(failed);
            // The player closed the home to play: the game in front, if something else took it.
            if line.trim() == home::HOME_CLOSED_LINE {
                if !system::in_front(pid) && !system::focus(pid) {
                    report(Event::Detail("The game's window could not be brought to the front: click it to play.".into()));
                }
                continue;
            }
            if line.trim() == APPLY_LINE {
                // The viewer has let the channel go; the settings it changed are in `values`.
                report(Event::Info("Applying the settings changed in the headset: VR restarts in the game.".into()));
                if let Err(why) = apply(&values) {
                    report(Event::Warning(format!("The settings could not be applied: {why}")));
                }
                continue;
            }
            let event = viewer_line(line);
            if let Event::Setting(id, value) = &event {
                values.set(id, value.clone());
            }
            report(event);
        }
        if let [at, rest @ ..] = focus_at
            && started.elapsed() >= *at
        {
            focus_at = rest;
            if !system::in_front(pid) && !system::focus(pid) && rest.is_empty() {
                report(Event::Detail("The game's window could not be brought to the front: click it to play.".into()));
            }
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
        failed = failure(&line).or(failed);
        report(viewer_line(line));
    }
    // The player closed the 2D window: that is Stop VR, not a run to start again.
    if status.code() == Some(home::CLOSED_BY_PLAYER) {
        stop.request();
    }
    if status.success() || stop.requested() {
        return Ok(());
    }
    Err(failed.unwrap_or_else(|| format!("the viewer stopped with exit code {}", status.code().unwrap_or(-1))))
}

/// Shows the game in the window's headset view until a stop is asked for, the game (`pid`) is
/// gone or the view closes, forwarding the viewer's lines as [`view`] does; then lets the game go,
/// the view showing the home alone again.
#[allow(clippy::too_many_arguments)]
fn view_linked(
    linked: &Linked,
    channel: &str,
    extra: &[String],
    captures: &Path,
    pid: u32,
    stop: &Stop,
    mut values: Values,
    apply: &mut dyn FnMut(&Values) -> Result<(), String>,
    report: &mut dyn FnMut(Event),
) -> Result<(), String> {
    // Lines from before this run are another run's.
    while linked.lines.try_recv().is_ok() {}
    let mut args = extra.to_vec();
    args.extend(["--captures".to_owned(), captures.display().to_string()]);
    linked.show_game(channel, &args)?;
    report(Event::State(State::Running));
    let started = Instant::now();
    let mut focus_at = &FOCUS_GAME_AT[..];
    let ended = loop {
        loop {
            let line = match linked.lines.try_recv() {
                Ok(line) => line,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    linked.let_go();
                    return Err("the headset view closed".into());
                }
            };
            // The player closed the home to play: the game in front, if something else took it.
            if line.trim() == home::HOME_CLOSED_LINE {
                if !system::in_front(pid) && !system::focus(pid) {
                    report(Event::Detail("The game's window could not be brought to the front: click it to play.".into()));
                }
                continue;
            }
            if line.trim() == APPLY_LINE {
                report(Event::Info("Applying the settings changed in the headset: VR restarts in the game.".into()));
                if let Err(why) = apply(&values) {
                    report(Event::Warning(format!("The settings could not be applied: {why}")));
                }
                continue;
            }
            // The run goes on: what the viewer could not do is for the player to read.
            if let Some(why) = failure(&line) {
                report(Event::Warning(why));
                continue;
            }
            let event = viewer_line(line);
            if let Event::Setting(id, value) = &event {
                values.set(id, value.clone());
            }
            report(event);
        }
        if let [at, rest @ ..] = focus_at
            && started.elapsed() >= *at
        {
            focus_at = rest;
            if !system::in_front(pid) && !system::focus(pid) && rest.is_empty() {
                report(Event::Detail("The game's window could not be brought to the front: click it to play.".into()));
            }
        }
        if stop.requested() {
            break Ok(());
        }
        if !system::alive(pid) {
            report(Event::Info("The game ended; the headset shows Monaka's home.".into()));
            break Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    linked.let_go();
    ended
}

/// Why the viewer says the run cannot go on, if `line` says so (a [`FAILURE_LINE`]).
fn failure(line: &str) -> Option<String> {
    line.strip_prefix(FAILURE_LINE).map(|why| why.trim().to_owned())
}

/// What the player is told when `MonakaStart` answered `code`: the reason the producer logged
/// (its last [`REJECTED_LINE`] in `log`), else what the code alone says.
pub(crate) fn refusal(log: &Path, code: u32) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    refusal_from(&text, code)
}

fn refusal_from(log: &str, code: u32) -> String {
    let reason = log.lines().rev().find_map(|line| line.split_once(REJECTED_LINE).map(|(_, why)| why.trim())).filter(|why| !why.is_empty());
    match (reason, code) {
        (Some(why), code::REVISION_MISMATCH) => format!("This game version isn't one Monaka VR knows ({why}). The game may have updated; see the log."),
        (Some(why), _) => {
            let mut chars = why.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
            let end = if why.ends_with(['.', '!', '?']) { "" } else { "." };
            format!("{first}{}{end}", chars.as_str())
        }
        (None, code::REVISION_MISMATCH) => "This game version isn't one Monaka VR knows; the game may have updated. See the log.".to_owned(),
        (None, code) => format!("The mod refused to start in this game (code {code}); see the log."),
    }
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
mod tests;
