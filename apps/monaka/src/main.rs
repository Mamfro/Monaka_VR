//! The Monaka VR window: pick a game, set its options, start VR. Drawn with egui; the work is
//! `monaka_launch` (a session runs on its own thread and wakes the window with each event).
//!
//! Left: the games, with the packs that cannot run here among them, and adding packs. Right: the
//! game, with Play and what it will do (or why it cannot yet), what is ready (SteamVR, the game),
//! what to set before playing, and its options by tab. At the bottom: the latest message, and the
//! details log under a toggle. Options are saved as they change and shared with `monaka_play`.
//!
//! With SteamVR running the window also opens its headset view ([`Headset`]): Monaka's home in the
//! headset with no game, the same games and settings as here (written for it as they change), and
//! where Play shows the game. What the player does there comes back as lines ([`App::headset_lines`]).
//!
//! `monaka.exe --reload-packs` asks the open window to read its game packs again ([`reload`]),
//! as its Reload packs button does, and exits without a window.
#![windows_subsystem = "windows"]

mod look;
mod options;
mod pc;
mod reload;
mod screenshot;

use eframe::egui::{self, Align, Button, Color32, Layout, RichText, ScrollArea, Sense, Stroke, TextEdit, ViewportCommand};
use monaka_contract::manifest::{CLOSED_BY_PLAYER, DEBUG_MENU, DEV_MODE, DIAGNOSTIC_LOGGING, DIAGNOSTIC_MARKER, Manifest, SETTING_LINE};
use monaka_contract::pack;
use monaka_core::home;
use monaka_launch::headset::Headset;
use monaka_launch::games::{self, Game, Launch, MONAKA, Shown, TABS};
use monaka_launch::session::{self, Console, Event, Shows, State, Stop};
use monaka_launch::settings::Settings;
use monaka_launch::{Paths, packs};
use pc::Pc;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Where Monaka's releases are, for an update.
const RELEASES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases");
/// SteamVR's Steam app id.
const STEAMVR_APP: u32 = 250820;
/// Checks SteamVR and the game this often.
const CHECK_EVERY: Duration = Duration::from_millis(1500);
/// The details log is emptied past this many bytes.
const DETAILS_MAX: usize = 200_000;
/// The left column (the games).
const LEFT_WIDTH: f32 = 250.0;
/// The column of Play and the line under it.
const PLAY_WIDTH: f32 = 230.0;
/// `--screenshot`: frames drawn before the picture, for sizes to settle.
const SCREENSHOT_FRAME: u32 = 5;
/// After the headset view failed to open, it is tried again this much later.
const HEADSET_RETRY: Duration = Duration::from_secs(15);
/// Clicks on Monaka's version on the About page that turn the debug menu on, each within
/// [`UNLOCK_GAP`] of the last.
const UNLOCK_CLICKS: u32 = 5;
const UNLOCK_GAP: Duration = Duration::from_millis(1500);
/// What the state line says while VR runs.
const RUNNING_LINE: &str = "VR is running. Put on your headset. Recentre with the headset's own control.";
/// A check that passed, shown as a dot beside it (the colours of a check that did not are the
/// theme's warning and weak text).
const READY: Color32 = Color32::from_rgb(46, 160, 67);

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == reload::ARG) {
        std::process::exit(if reload::ask() { 0 } else { 1 });
    }
    let screenshot = screenshot::request();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(concat!("Monaka VR ", env!("CARGO_PKG_VERSION"), ""))
        .with_inner_size([940.0, 700.0])
        .with_min_inner_size([760.0, 420.0]);
    if screenshot.is_some() {
        viewport = viewport.with_position([-20_000.0, -20_000.0]).with_active(false);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("monaka", options, Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, screenshot)))))
}

/// A session in progress.
struct Running {
    stop: Stop,
    events: Receiver<Event>,
    /// Its game.
    game: &'static str,
    /// Where the headset view's lines go while the run shows its game there.
    to_headset: Option<mpsc::Sender<String>>,
}

/// A question over the window, waiting for the player's answer.
enum Dialog {
    /// A staged pack, added on OK.
    Add { staged: packs::Staged, question: String },
    /// A pack's folder, deleted on OK.
    Remove { folder: PathBuf, names: String },
    /// Packs made for a newer Monaka; OK opens the download page.
    Update(String),
}

/// What the right side shows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Page {
    Game,
    About,
    /// A pack folder that cannot run here (its index among the catalogue's left out).
    LeftOut(usize),
}

/// What is ready, as last checked.
#[derive(Clone, Copy, Default)]
struct Checks {
    steamvr_up: bool,
    /// Another session's viewer is running (not ours).
    other_session: bool,
    game_up: bool,
    game_installed: bool,
}

struct App {
    ctx: egui::Context,
    paths: Paths,
    settings: Settings,
    pc: Box<dyn Pc>,
    libraries: Vec<PathBuf>,
    /// The games on offer (Monaka's own built ones, then the packs'); may be empty.
    list: &'static [Game],
    /// The pack folders whose pack cannot run here, listed after the games.
    left_out: Vec<packs::LeftOut>,
    selected: usize,
    tab: Shown,
    /// Scrolls the options here on the next frame (to the top after a game or tab change).
    scroll_to: Option<f32>,
    page: Page,
    /// Clicks on Monaka's version so far, and when the last was.
    version_clicks: (u32, Option<Instant>),
    /// The debug menu was turned on this run: its switch stays on the Monaka tab even once off.
    debug_unlocked: bool,

    running: Option<Running>,
    run_folder: Option<PathBuf>,
    /// The headset view, while it runs; whether the window opens it by itself (not in tests or
    /// screenshots, nor once the player left VR, until they open it again); and when opening it
    /// last failed.
    headset: Option<Headset>,
    headset_auto: bool,
    headset_failed: Option<Instant>,
    closing: bool,
    checks: Checks,
    checked: Instant,
    state_line: String,
    details: String,
    /// The details log shows above the bottom line.
    log_open: bool,

    dialog: Option<Dialog>,
    /// Packs made for a newer Monaka told about this run.
    told_too_new: Vec<String>,
    /// `--screenshot`: where the picture goes, and the frames drawn so far.
    screenshot: Option<(PathBuf, u32)>,
    /// Reloads asked for from outside ([`reload`]), and one waiting for VR to stop.
    reload_watch: Option<reload::Watch>,
    reload_waiting: bool,
}

impl App {
    fn new(ctx: &egui::Context, screenshot: Option<screenshot::Request>) -> App {
        let headset = screenshot.is_none();
        App::with(ctx, screenshot, Paths::locate(), Settings::load(), Box::new(pc::ThisPc), reload::Watch::new(), headset)
    }

    /// The window over `paths` and `settings`, asking `pc`, reloading when `reload_watch` says;
    /// opening the headset view by itself when `headset`.
    fn with(ctx: &egui::Context, screenshot: Option<screenshot::Request>, paths: Paths, settings: Settings, pc: Box<dyn Pc>, reload_watch: Option<reload::Watch>, headset: bool) -> App {
        look::start(ctx, screenshot.as_ref().and_then(|r| r.dark));
        let catalogue = games::rescan(&paths);
        let (list, left_out) = (catalogue.games, catalogue.left_out);
        let mut selected = settings.selected.as_deref().and_then(|id| list.iter().position(|g| g.id == id)).unwrap_or(0);
        let mut tab = Shown::Main;
        let mut scroll_to = Some(0.0);
        let about = screenshot.as_ref().is_some_and(|r| r.tab.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("about")));
        let page = if about { Page::About } else { Page::Game };
        if let Some(request) = &screenshot {
            tab = request.tab.as_deref().and_then(|name| TABS.iter().find(|(_, n)| n.eq_ignore_ascii_case(name))).map_or(Shown::Main, |(t, _)| *t);
            selected = request.game.as_deref().and_then(|id| list.iter().position(|g| g.id == id)).unwrap_or(selected);
            scroll_to = Some(request.scroll);
        }
        let mut app = App {
            ctx: ctx.clone(),
            paths,
            settings,
            libraries: pc.steam_libraries(),
            pc,
            list,
            left_out,
            selected,
            tab,
            scroll_to,
            page,
            version_clicks: (0, None),
            debug_unlocked: false,
            running: None,
            run_folder: None,
            headset: None,
            headset_auto: headset,
            headset_failed: None,
            closing: false,
            checks: Checks { game_installed: true, ..Default::default() },
            checked: Instant::now(),
            state_line: String::new(),
            details: String::new(),
            log_open: false,
            dialog: None,
            told_too_new: Vec::new(),
            screenshot: screenshot.map(|r| (r.file, 0)),
            reload_watch,
            reload_waiting: false,
        };
        for problem in &catalogue.problems {
            app.detail(problem);
        }
        app.tell_too_new(&catalogue.too_new);
        app.refresh_checks();
        app
    }

    /// The selected game; none while no game is on offer.
    fn game(&self) -> Option<&'static Game> {
        self.list.get(self.selected)
    }

    /// Shows game `index`, and remembers it for next time.
    fn select(&mut self, index: usize) {
        self.page = Page::Game;
        if index != self.selected {
            self.scroll_to = Some(0.0);
        }
        self.selected = index.min(self.list.len().saturating_sub(1));
        if let Some(game) = self.game() {
            self.settings.selected = Some(game.id.to_owned());
            let _ = self.settings.save();
        }
        self.refresh_checks();
    }

    /// Asks the player to update Monaka for packs made for a newer one not asked about yet (not
    /// while taking a screenshot).
    fn tell_too_new(&mut self, too_new: &[String]) {
        let fresh: Vec<String> = too_new.iter().filter(|p| !self.told_too_new.contains(p)).cloned().collect();
        if fresh.is_empty() || self.screenshot.is_some() {
            return;
        }
        self.told_too_new.extend(fresh.iter().cloned());
        self.dialog = Some(Dialog::Update(fresh.join("\n")));
    }

    /// Reads the games again after a pack came or went, and shows `select` (a game id), else the
    /// game shown before, else the first.
    fn reload(&mut self, select: Option<&str>) {
        let before = self.game().map(|g| g.id);
        let catalogue = games::rescan(&self.paths);
        self.list = catalogue.games;
        self.left_out = catalogue.left_out;
        for problem in &catalogue.problems {
            self.detail(problem);
        }
        self.tell_too_new(&catalogue.too_new);
        let index = select.or(before).and_then(|id| self.list.iter().position(|g| g.id == id)).unwrap_or(0);
        self.select(index);
    }

    /// Reads the game packs again from their files (a pack rebuilt meanwhile shows as it is now).
    fn reload_packs(&mut self) {
        self.reload(None);
        self.say("Game packs reloaded.", true);
    }

    /// A reload asked for from outside: now, or once VR has stopped.
    fn reload_asked(&mut self) {
        if self.reload_watch.as_ref().is_some_and(reload::Watch::asked) {
            self.reload_waiting = true;
        }
        if self.reload_waiting && self.running.is_none() {
            self.reload_waiting = false;
            self.reload_packs();
        }
    }

    /// Unpacks a pack the player picks, and asks with the warning before adding it.
    fn add_pack(&mut self) {
        let Some(file) = self.pc.pick_pack_file() else { return };
        let staged = match packs::stage(&file, &self.paths.packs) {
            Ok(staged) => staged,
            Err(why) => return self.say(&format!("Game pack not added: {why}"), true),
        };
        if let Some(what) = staged.too_new() {
            self.say(&format!("Game pack not added: {what} was made for a newer Monaka."), true);
            self.dialog = Some(Dialog::Update(what));
            return;
        }
        let clashes = games::clashes(staged.manifests(), staged.replaces().as_deref());
        if !clashes.is_empty() {
            return self.say(&format!("Game pack not added: {}", clashes.join(" ")), true);
        }
        let replaces = if staged.replaces().is_some() { format!("\n\nIt replaces the installed {}.", staged.name()) } else { String::new() };
        let question = format!(
            "Add {} (game pack {})?{replaces}\n\nGame packs run their own code inside the game. Only add packs from people you trust.",
            staged.names(),
            staged.credit()
        );
        self.dialog = Some(Dialog::Add { staged, question });
    }

    /// Asks before removing the selected game's pack.
    fn remove_pack(&mut self) {
        let Some(pack) = self.game().filter(|g| g.removable()).map(Game::pack) else { return };
        let folder = pack.folder().to_owned();
        let names = games::names_in(&folder);
        self.dialog = Some(Dialog::Remove { folder, names });
    }

    /// Asks before removing a pack folder that cannot run here.
    fn remove_left_out(&mut self, index: usize) {
        let Some(left) = self.left_out.get(index) else { return };
        self.dialog = Some(Dialog::Remove { folder: left.folder.clone(), names: left.name.clone() });
    }

    /// Acts on the player's answer to `dialog`.
    fn answer(&mut self, dialog: Dialog, ok: bool) {
        match dialog {
            Dialog::Add { staged, .. } if ok => {
                let names = staged.names();
                let first = staged.manifests().next().map(|m| m.id.to_owned());
                match staged.install() {
                    Ok(folder) => {
                        self.say(&format!("Added {names} ({}).", folder.display()), true);
                        self.reload(first.as_deref());
                    }
                    Err(why) => self.say(&format!("Game pack not added: {why}"), true),
                }
            }
            Dialog::Add { .. } => self.say("Game pack not added.", false),
            Dialog::Remove { folder, names } if ok => match packs::remove(&folder, &self.paths.packs) {
                Ok(()) => {
                    self.say(&format!("Removed {names}."), true);
                    self.reload(None);
                }
                Err(why) => self.say(&why, true),
            },
            Dialog::Update(_) if ok => {
                let _ = self.pc.open(RELEASES);
            }
            Dialog::Remove { .. } | Dialog::Update(_) => {}
        }
    }

    /// Updates what is ready (SteamVR, the game).
    fn refresh_checks(&mut self) {
        self.checked = Instant::now();
        let game = self.game();
        let runs_as = game.map(|game| game.runs_as(&self.settings.values(game.id))).unwrap_or_default();
        let executables: Vec<&str> = runs_as.iter().map(String::as_str).collect();
        let running = self.pc.running(&executables, self.headset.as_ref().map(Headset::pid));
        self.checks.steamvr_up = running.steamvr;
        self.checks.other_session = self.running.is_none() && running.viewer;
        let Some(game) = game else { return };
        self.checks.game_up = !executables.is_empty() && running.game;
        // A pack for any game knows no Steam app.
        self.checks.game_installed = self.libraries.is_empty() || game.any_game() || self.pc.steam_installed(&self.libraries, game.steam_app);
    }

    fn start_session(&mut self, shows: Shows) {
        let stop = match Stop::new() {
            Ok(stop) => stop,
            Err(why) => return self.say(&why, true),
        };
        let Some(game) = self.game() else { return };
        let (send_event, events) = mpsc::channel();
        let values = self.settings.values(game.id);
        // A run in the headset shows its game in the headset view, while it is open.
        let (linked, to_headset) = match (&self.headset, shows) {
            (Some(headset), Shows::Headset) => {
                let (linked, send) = headset.link();
                (Some(linked), Some(send))
            }
            _ => (None, None),
        };
        let paths = self.paths.clone();
        let session_stop = stop.clone();
        let ctx = self.ctx.clone();
        self.details.clear();
        self.run_folder = None;
        std::thread::spawn(move || {
            session::run(game, &values, &paths, Console::Hidden, shows, linked, &session_stop, &mut |event| {
                let _ = send_event.send(event);
                ctx.request_repaint();
            });
        });
        self.running = Some(Running { stop, events, game: game.id, to_headset });
        self.say("Starting...", false);
        self.refresh_checks();
    }

    fn stop_session(&mut self) {
        if let Some(running) = &self.running {
            running.stop.request();
        }
    }

    /// Opens the headset view (SteamVR's OpenXR); a failure is said in the details and tried
    /// again later.
    fn open_headset(&mut self) {
        let ctx = self.ctx.clone();
        match Headset::start(&self.paths, Console::Hidden, move || ctx.request_repaint()) {
            Ok(headset) => {
                self.headset = Some(headset);
                self.headset_failed = None;
                self.detail("Monaka's home is open in the headset.");
            }
            Err(why) => {
                self.headset_failed = Some(Instant::now());
                self.detail(&format!("The headset view did not open: {why}"));
            }
        }
    }

    /// Closes the headset view: a run showing its game there stops with it.
    fn close_headset(&mut self) {
        if self.running.as_ref().is_some_and(|r| r.to_headset.is_some()) {
            self.stop_session();
            if let Some(running) = &mut self.running {
                running.to_headset = None;
            }
        }
        self.headset = None;
    }

    /// The headset view kept as it should be: noticed when it closed (the player left VR, or it
    /// failed), opened while SteamVR runs and nothing else has the headset.
    fn keep_headset(&mut self) {
        if let Some(code) = self.headset.as_mut().and_then(Headset::ended) {
            if code == CLOSED_BY_PLAYER {
                self.headset_auto = false;
                self.say("You left VR. Open in headset takes you back.", true);
            } else {
                self.headset_failed = Some(Instant::now());
                self.say(&format!("The headset view closed (exit code {code}); see the log."), true);
            }
            self.close_headset();
            return;
        }
        let retry = self.headset_failed.is_none_or(|at| at.elapsed() >= HEADSET_RETRY);
        // A run with its own viewer (the 2D window, or one started before) keeps the headset.
        if self.headset.is_none() && self.headset_auto && retry && self.checks.steamvr_up && !self.checks.other_session && self.running.is_none() {
            self.open_headset();
        }
    }

    /// What the player did in the headset view: show a game, play it, stop it; a setting changed
    /// (saved here, and passed on to a run showing its game there with the rest of its lines).
    fn headset_lines(&mut self) {
        let Some(lines) = self.headset.as_ref().map(Headset::lines) else { return };
        for line in lines {
            let said = line.trim();
            if let Some(id) = said.strip_prefix(home::SELECT_LINE) {
                if let (None, Some(index)) = (&self.running, self.list.iter().position(|g| g.id == id.trim())) {
                    self.select(index);
                }
                continue;
            }
            if said == home::PLAY_LINE {
                let (can_press, why) = self.play_line();
                if self.running.is_none() && can_press {
                    self.start_session(Shows::Headset);
                } else if !why.is_empty() {
                    self.say(why, false);
                }
                continue;
            }
            if said == home::STOP_LINE {
                self.stop_session();
                continue;
            }
            match self.running.as_ref().and_then(|r| r.to_headset.as_ref()) {
                Some(send) => {
                    let _ = send.send(line);
                }
                None => match said.strip_prefix(SETTING_LINE).and_then(|rest| rest.trim().split_once('=')) {
                    Some((id, value)) => self.set_from_headset(id, value.to_owned()),
                    None => self.detail(&format!("  {line}")),
                },
            }
        }
    }

    /// Writes what the headset view shows: the game shown here with its settings, and the games,
    /// marking the one that plays.
    fn show_in_headset(&mut self) {
        if self.headset.is_none() {
            return;
        }
        let game = self.game();
        let mut values = game.map_or_else(|| self.settings.monaka.clone(), |g| self.settings.values(g.id));
        // A build tree has developer mode, as its runs do.
        if self.paths.dev_games.is_some() {
            values.set(DEV_MODE.id, "1");
        }
        let manifest = game.map_or_else(String::new, |g| {
            let about = &g.pack().about;
            pack::text(std::slice::from_ref(g.manifest), &about.author, &about.source, &about.version, &pack::Needs::NONE)
        });
        let owners: Vec<&Manifest> = game.map(|g| g.manifest).into_iter().chain([&MONAKA]).collect();
        let playing = self.running.as_ref().map(|r| r.game);
        let games: Vec<home::Listed> = self
            .list
            .iter()
            .enumerate()
            .map(|(i, g)| home::Listed { id: g.id.to_owned(), name: g.name.to_owned(), status: g.status.label().to_owned(), current: i == self.selected, playing: playing == Some(g.id) })
            .collect();
        let values = home::values_text(&owners, &values);
        if let Some(headset) = &mut self.headset {
            headset.show(manifest, values, home::games_text(&games));
        }
    }

    /// The state line, and the details.
    fn say(&mut self, line: &str, detail: bool) {
        line.clone_into(&mut self.state_line);
        if detail {
            self.detail(line);
        }
    }

    fn detail(&mut self, line: &str) {
        // Keep the log from growing without end.
        if self.details.len() > DETAILS_MAX {
            self.details.clear();
        }
        self.details.push_str(line);
        self.details.push('\n');
    }

    fn session_events(&mut self) {
        let Some(running) = &self.running else { return };
        let events: Vec<Event> = running.events.try_iter().collect();
        for event in events {
            match event {
                Event::State(state) => self.session_state(state),
                Event::Info(line) | Event::Warning(line) => self.say(&line, true),
                Event::Failed(line) => {
                    self.say(&line, false);
                    self.detail(&format!("Error: {line}"));
                    self.log_open = true;
                }
                Event::Detail(line) => self.detail(&format!("  {line}")),
                Event::RunFolder(folder) => {
                    self.run_folder = Some(folder);
                    // A run restarted mid-session takes the setting as it is now.
                    self.mark_diagnostics();
                }
                Event::Setting(id, value) => self.set_from_headset(&id, value),
            }
        }
    }

    fn session_state(&mut self, state: State) {
        match state {
            State::WaitingForGame => self.say("Waiting for the game...", false),
            State::Attaching => self.say("Attaching to the game...", false),
            State::Running => self.say(RUNNING_LINE, false),
            State::Stopping => self.say("Stopping VR...", false),
            State::Stopped => {
                let ended_well = matches!(self.state_line.as_str(), RUNNING_LINE | "Stopping VR..." | "Starting...");
                if ended_well {
                    self.say("VR stopped.", true);
                }
                self.running = None;
                if self.closing {
                    self.ctx.send_viewport_cmd(ViewportCommand::Close);
                    return;
                }
                self.refresh_checks();
            }
        }
    }

    /// Saves an option the player changed in the headset's home, as its control here would:
    /// Monaka's own for every game, else the running game's.
    fn set_from_headset(&mut self, id: &str, value: String) {
        let Some(game) = self.game() else { return };
        let monakas = MONAKA.option(id).is_some() && game.option(id).is_none();
        let saved = if monakas { &mut self.settings.monaka } else { self.settings.values.entry(game.id.to_owned()).or_default() };
        saved.set(id, value);
        let _ = self.settings.save();
    }

    /// The game's options back to their defaults (forgotten in the settings).
    fn reset_defaults(&mut self) {
        let Some(game) = self.game() else { return };
        self.settings.values.remove(game.id);
        let _ = self.settings.save();
        self.say(&format!("{}: options back to their defaults.", game.name), false);
    }

    /// Leaves or takes away the diagnostic-logging marker in the running run's folder, as the
    /// setting is now; the producer follows it within a second.
    fn mark_diagnostics(&self) {
        let (Some(_), Some(folder)) = (&self.running, &self.run_folder) else { return };
        let marker = folder.join(DIAGNOSTIC_MARKER);
        let _ = if self.settings.monaka.switch(&MONAKA, DIAGNOSTIC_LOGGING.id) { std::fs::write(&marker, "") } else { std::fs::remove_file(&marker) };
    }

    fn open_logs(&mut self) {
        let folder = self.run_folder.clone().unwrap_or_else(|| self.paths.runs.clone());
        let _ = std::fs::create_dir_all(&folder);
        if let Err(why) = self.pc.open(&folder.to_string_lossy()) {
            self.say(&why, true);
        }
    }

    /// Left: the games, then the packs that cannot run here, then adding packs and About.
    fn games_panel(&mut self, ui: &mut egui::Ui) {
        let idle = self.running.is_none();
        let mut picked = None;
        let mut picked_left_out = None;
        let bottom = 3.0 * ui.spacing().interact_size.y + 2.0 * ui.spacing().item_spacing.y + 12.0;
        ScrollArea::vertical().id_salt("games").auto_shrink([false, true]).max_height(ui.available_height() - bottom).show(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| {
                for (i, game) in self.list.iter().enumerate() {
                    if list_row(ui, self.page == Page::Game && i == self.selected, game.name, game.status.label(), None).clicked() {
                        picked = Some(i);
                    }
                }
                let warn = ui.visuals().warn_fg_color;
                for (i, left) in self.left_out.iter().enumerate() {
                    if list_row(ui, self.page == Page::LeftOut(i), &left.name, "Needs update", Some(warn)).clicked() {
                        picked_left_out = Some(i);
                    }
                }
            });
            if self.list.is_empty() && self.left_out.is_empty() {
                ui.label(RichText::new("No games yet").weak());
            }
        });
        if let Some(i) = picked {
            self.select(i);
        }
        if let Some(i) = picked_left_out {
            self.page = Page::LeftOut(i);
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            if ui.selectable_label(self.page == Page::About, "About").clicked() {
                self.page = if self.page == Page::About { Page::Game } else { Page::About };
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(idle, Button::new("Add game pack...")).clicked() {
                    self.add_pack();
                }
                if ui.add_enabled(idle, Button::new("Reload packs")).clicked() {
                    self.reload_packs();
                }
            });
            ui.separator();
        });
    }

    /// Whether the settings for testers show: developer mode on, or a build tree.
    fn dev_mode(&self) -> bool {
        self.settings.monaka.switch(&MONAKA, DEV_MODE.id) || self.paths.dev_games.is_some()
    }

    /// Whether the debug menu's switch shows on the Monaka tab: on, or turned on this run.
    fn debug_menu_shown(&self) -> bool {
        self.debug_unlocked || self.settings.monaka.switch(&MONAKA, DEBUG_MENU.id)
    }

    /// Right, in place of the game: the versions of Monaka and of each game pack.
    fn about_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("About");
        ui.add_space(10.0);
        let unknown = |text: &str| if text.is_empty() { "unknown".to_owned() } else { text.to_owned() };
        let mut clicked = false;
        ScrollArea::vertical().id_salt("about").auto_shrink(false).show(ui, |ui| egui::Grid::new("versions").num_columns(3).spacing([28.0, 8.0]).show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new("Version").weak());
            ui.label(RichText::new("Built with Monaka").weak());
            ui.end_row();
            ui.label("Monaka");
            clicked = ui.add(egui::Label::new(env!("CARGO_PKG_VERSION")).sense(egui::Sense::click())).clicked();
            ui.label("");
            ui.end_row();
            // One row per pack DLL; a pack may serve several games.
            let mut shown: Vec<&std::path::Path> = Vec::new();
            for game in self.list {
                let pack = game.pack();
                if shown.contains(&pack.dll.as_path()) {
                    continue;
                }
                shown.push(&pack.dll);
                ui.label(pack.names());
                ui.label(unknown(&pack.about.version));
                ui.label(unknown(&pack.about.monaka));
                ui.end_row();
            }
        }));
        if clicked {
            self.version_clicked();
        }
    }

    /// A click on Monaka's version: the fifth in a row turns the debug menu on.
    fn version_clicked(&mut self) {
        let now = Instant::now();
        let (count, last) = self.version_clicks;
        let count = if last.is_some_and(|t| now.duration_since(t) < UNLOCK_GAP) { count + 1 } else { 1 };
        self.version_clicks = (count, Some(now));
        if count < UNLOCK_CLICKS || self.debug_menu_shown() {
            return;
        }
        self.debug_unlocked = true;
        self.settings.monaka.set(DEBUG_MENU.id, "1");
        let _ = self.settings.save();
        self.say("Debug menu on (Monaka tab).", false);
    }

    /// What Play does now, or why it cannot yet: whether it can be pressed, and the line under it.
    fn play_line(&self) -> (bool, &'static str) {
        let checks = self.checks;
        let Some(game) = self.game() else { return (false, "") };
        if let Some(running) = &self.running {
            return (!running.stop.requested() && !self.closing, "");
        }
        if checks.other_session {
            return (false, "Another VR session is running. Stop it first.");
        }
        if !checks.steamvr_up {
            return (false, "Start SteamVR and turn on your headset first.");
        }
        match (checks.game_up, checks.game_installed, game.launch) {
            (true, _, _) => (true, "Joins the running game."),
            (false, _, Launch::AtStart) => (true, "Then start the game: Monaka joins it as it starts."),
            (false, false, _) => (false, "The game is not installed in Steam."),
            (false, true, Launch::Auto { .. }) => (true, "Starts the game through Steam."),
            (false, true, Launch::Manual) => (false, "Start the game first and load into gameplay."),
        }
    }

    /// The top of the game's page: its name, status and origin at the left; Play, and what it
    /// does, at the right. Under them what is ready, and what to set before playing.
    fn game_header(&mut self, ui: &mut egui::Ui, game: &'static Game) {
        let idle = self.running.is_none();
        let (can_press, why) = self.play_line();
        let text_width = (ui.available_width() - PLAY_WIDTH - 16.0).max(200.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(text_width);
                ui.heading(game.name);
                ui.add(egui::Label::new(RichText::new(format!("{}: {}", game.status.label(), game.status.meaning())).weak()).wrap());
                // A local build's file is in the tooltip, out of the way.
                let origin = if game.removable() { game.credit() } else { "Built on this PC".to_owned() };
                ui.add(egui::Label::new(RichText::new(origin).size(look::SMALL).weak()).wrap()).on_hover_text(game.pack().dll.display().to_string());
                if game.removable() && ui.add_enabled(idle, Button::new("Remove pack").small()).clicked() {
                    self.remove_pack();
                }
            });
            ui.vertical(|ui| {
                ui.set_width(PLAY_WIDTH);
                ui.with_layout(Layout::top_down_justified(Align::Center), |ui| {
                    let stopping = self.running.as_ref().is_some_and(|r| r.stop.requested());
                    let label = match (idle, stopping) {
                        (true, _) => "Play",
                        (false, false) => "Stop",
                        (false, true) => "Stopping...",
                    };
                    let mut play = Button::new(RichText::new(label).family(look::semibold()).size(look::BIG)).min_size(egui::vec2(PLAY_WIDTH, 40.0));
                    if idle && can_press {
                        play = play.fill(ui.visuals().selection.bg_fill);
                    }
                    // Enter presses it too, while nothing else has the keyboard (a text field, or
                    // a focused button that Enter presses itself).
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter)) && ui.memory(|m| m.focused().is_none());
                    if ui.add_enabled(can_press, play).clicked() || (enter && can_press) {
                        if idle {
                            self.start_session(Shows::Headset);
                        } else {
                            self.stop_session();
                        }
                    }
                    if !why.is_empty() {
                        ui.add(egui::Label::new(RichText::new(why).size(look::SMALL).weak()).wrap());
                    }
                    // The 2D window: the run without a headset, for testers who have the debug menu.
                    if (self.debug_menu_shown() || self.paths.dev_games.is_some())
                        && idle
                        && ui
                            .add_enabled(can_press || (self.checks.game_up && !self.checks.other_session), Button::new("Debug without VR").small())
                            .on_hover_text("The game in a window with the debug menu and its panels, no headset")
                            .clicked()
                    {
                        self.start_session(Shows::Desktop);
                    }
                });
            });
        });
        ui.add_space(8.0);
        self.ready_lines(ui, game);
        ui.add_space(4.0);
        ui.add(egui::Label::new(format!("Before you play: {}", game.before)).wrap());
    }

    /// What is ready, each with a dot, and the button that readies it where there is one.
    fn ready_lines(&mut self, ui: &mut egui::Ui, game: &'static Game) {
        let checks = self.checks;
        let idle = self.running.is_none();
        let (warn, off) = (ui.visuals().warn_fg_color, ui.visuals().weak_text_color());
        ui.horizontal(|ui| {
            if !checks.steamvr_up {
                ready(ui, warn, "SteamVR not running");
                if ui.add(Button::new("Start SteamVR").small()).clicked()
                    && let Err(why) = self.pc.launch_steam_game(STEAMVR_APP)
                {
                    self.say(&why, true);
                }
            } else if self.headset.is_some() {
                ready(ui, READY, "In the headset: Monaka's home");
                if ui.add(Button::new("Leave VR").small()).on_hover_text("Closes the headset view; Open in headset brings it back").clicked() {
                    self.headset_auto = false;
                    self.close_headset();
                }
            } else {
                ready(ui, off, "Headset view closed");
                if ui.add_enabled(idle && !checks.other_session, Button::new("Open in headset").small()).clicked() {
                    self.headset_auto = true;
                    self.open_headset();
                }
            }
            ui.add_space(18.0);
            let line = match (checks.game_up, checks.game_installed) {
                (true, _) => (READY, "Game running"),
                (false, false) => (warn, "Game not installed in Steam"),
                (false, true) => (off, "Game not running"),
            };
            ready(ui, line.0, line.1);
            let can_start_game = idle && !checks.game_up && checks.game_installed && !game.any_game();
            if can_start_game && ui.add(Button::new("Start game").small()).clicked() {
                match self.pc.launch_steam_game(game.steam_app) {
                    Ok(()) => self.say(&format!("Starting {} through Steam...", game.name), false),
                    Err(why) => self.say(&why, true),
                }
            }
        });
    }

    /// The bottom line: the latest message, opening the logs' folder and the details log.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(if self.log_open { "Hide log" } else { "Show log" }).clicked() {
                    self.log_open = !self.log_open;
                }
                if ui.button("Open logs").clicked() {
                    self.open_logs();
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(self.state_line.as_str()).weak()).truncate());
                });
            });
        });
    }

    /// The details log, over the bottom line while it is open.
    fn details_panel(&mut self, ui: &mut egui::Ui) {
        ScrollArea::vertical().id_salt("details").stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
            // Read-only, but selectable for copying.
            ui.add(TextEdit::multiline(&mut self.details.as_str()).desired_width(f32::INFINITY).frame(egui::Frame::NONE).font(egui::FontId::proportional(look::SMALL)));
        });
    }

    /// Right, for a pack folder that cannot run here: what it is, why, and removing it.
    fn left_out_page(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(left) = self.left_out.get(index) else {
            self.page = Page::Game;
            return;
        };
        ui.heading(&left.name);
        ui.add_space(2.0);
        ui.label(RichText::new("This game pack cannot run with this Monaka.").color(ui.visuals().warn_fg_color));
        ui.add(egui::Label::new(&left.why).wrap());
        ui.add(egui::Label::new(RichText::new(left.folder.display().to_string()).size(look::SMALL).weak()).wrap());
        ui.add_space(8.0);
        if ui.add_enabled(self.running.is_none(), Button::new("Remove pack")).clicked() {
            self.remove_left_out(index);
        }
    }

    /// Right, from the top: the game's header, its tabs and the options.
    fn game_panel(&mut self, ui: &mut egui::Ui) {
        match self.page {
            Page::About => return self.about_page(ui),
            Page::LeftOut(index) => return self.left_out_page(ui, index),
            Page::Game => {}
        }
        let Some(game) = self.game() else {
            // No game on offer yet: a Monaka without its own builds and no pack added.
            ui.heading("No games yet");
            ui.label(RichText::new("Add a game pack (a .zip from the people who made it) with Add game pack.").size(look::SMALL).weak());
            return;
        };
        self.game_header(ui, game);
        ui.add_space(14.0);

        let rows = options::rows(game, self.debug_menu_shown(), self.dev_mode());
        let tabs = options::tabs(game, &rows);
        // A tab the game lacks falls back to its first (Main, where it has one).
        if !tabs.contains(&self.tab) {
            self.tab = tabs.first().copied().unwrap_or(Shown::Main);
        }
        let idle = self.running.is_none();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            if tabs.len() > 1 {
                for (tab, name) in TABS.iter().filter(|(t, _)| tabs.contains(t)) {
                    if tab_button(ui, self.tab == *tab, name).clicked() && self.tab != *tab {
                        self.tab = *tab;
                        self.scroll_to = Some(0.0);
                    }
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(idle, Button::new("Reset to defaults").small()).clicked() {
                    self.reset_defaults();
                }
            });
        });
        let line = ui.visuals().widgets.noninteractive.bg_stroke;
        let (left, right) = (ui.max_rect().left(), ui.max_rect().right());
        let y = ui.cursor().top();
        ui.painter().hline(left..=right, y, line);
        ui.add_space(10.0);
        let mut area = ScrollArea::vertical().id_salt("options").auto_shrink(false);
        if let Some(offset) = self.scroll_to.take() {
            area = area.vertical_scroll_offset(offset);
        }
        let running = self.running.is_some();
        let changed = area.show(ui, |ui| options::show(ui, &mut self.settings, game, &rows, self.tab, running)).inner;
        if changed == Some(DIAGNOSTIC_LOGGING.id) {
            self.mark_diagnostics();
        }
    }

    /// The question over the window, if one waits; Escape or a click beside it is Cancel.
    fn dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.dialog else { return };
        let (title, text, ok) = match dialog {
            Dialog::Add { question, .. } => ("Add game pack", question.clone(), "Add"),
            Dialog::Remove { folder, names } => (
                "Remove game pack",
                format!("Remove the game pack {names}?\n\nIts folder is deleted: {}\nYour options for its games are kept.", folder.display()),
                "Remove",
            ),
            Dialog::Update(what) => {
                ("Update Monaka", format!("This game pack needs a newer version of Monaka:\n\n{what}\n\nUpdate Monaka to use it. Open the download page now?"), "Open")
            }
        };
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.label(RichText::new(title).family(look::semibold()).size(look::BIG));
            ui.add_space(6.0);
            ui.add(egui::Label::new(text).wrap());
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
                if ui.button(ok).clicked() {
                    answer = Some(true);
                }
            });
        });
        if answer.is_none() && response.should_close() {
            answer = Some(false);
        }
        if let (Some(ok), Some(dialog)) = (answer, answer.and_then(|_| self.dialog.take())) {
            self.answer(dialog, ok);
        }
    }

    /// `--screenshot`: asks for the picture once the window has drawn a few frames, saves it when
    /// it comes, then closes.
    fn take_screenshot(&mut self, ctx: &egui::Context) {
        let Some((file, frames)) = &mut self.screenshot else { return };
        *frames += 1;
        if *frames == SCREENSHOT_FRAME {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let _ = screenshot::save(&image, file);
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else if *frames > SCREENSHOT_FRAME + 60 {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.headset_lines();
        self.session_events();
        self.reload_asked();
        if self.checked.elapsed() >= CHECK_EVERY {
            self.refresh_checks();
            self.keep_headset();
        }
        self.show_in_headset();
        ctx.request_repaint_after(CHECK_EVERY);
        if ctx.input(|i| i.viewport().close_requested()) && self.running.is_some() {
            // Stop the producer before the window goes; the Stopped event closes it.
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if !self.closing {
                self.closing = true;
                self.say("Stopping VR before closing...", false);
                self.stop_session();
            }
        }
        self.take_screenshot(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let fill = ui.visuals().panel_fill;
        let side = look::side(ui.visuals());
        let line = ui.visuals().widgets.noninteractive.bg_stroke;
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(fill).stroke(line).inner_margin(egui::Margin::symmetric(14, 6)))
            .show(ui, |ui| self.status_bar(ui));
        if self.log_open {
            egui::Panel::bottom("details")
                .resizable(true)
                .default_size(180.0)
                .min_size(80.0)
                .frame(egui::Frame::new().fill(side).inner_margin(egui::Margin::symmetric(14, 8)))
                .show(ui, |ui| self.details_panel(ui));
        }
        egui::Panel::left("games")
            .resizable(false)
            .exact_size(LEFT_WIDTH + 20.0)
            .frame(egui::Frame::new().fill(side).inner_margin(egui::Margin::same(10)))
            .show(ui, |ui| self.games_panel(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin { left: 20, right: 20, top: 16, bottom: 0 })).show(ui, |ui| self.game_panel(ui));
        let ctx = ui.ctx().clone();
        self.dialog(&ctx);
    }
}

/// A game in the list: its name, and under it its status (in `status_color`, else weak); the
/// whole row is the button.
fn list_row(ui: &mut egui::Ui, selected: bool, name: &str, status: &str, status_color: Option<Color32>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), selected, name));
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 4.0, visuals.selection.bg_fill);
        } else if response.hovered() {
            painter.rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let text = if selected { visuals.selection.stroke.color } else { visuals.text_color() };
        let weak = if selected { visuals.selection.stroke.color.gamma_multiply(0.8) } else { status_color.unwrap_or(visuals.weak_text_color()) };
        let width = rect.width() - 20.0;
        let name = egui::WidgetText::from(name).into_galley(ui, Some(egui::TextWrapMode::Truncate), width, egui::TextStyle::Body);
        let status = egui::WidgetText::from(RichText::new(status).size(look::SMALL)).into_galley(ui, Some(egui::TextWrapMode::Truncate), width, egui::TextStyle::Small);
        let top = rect.center().y - (name.size().y + status.size().y) / 2.0;
        let status_top = top + name.size().y;
        painter.galley(egui::pos2(rect.left() + 10.0, top), name, text);
        painter.galley(egui::pos2(rect.left() + 10.0, status_top), status, weak);
    }
    response
}

/// A tab: its name, underlined in the accent while it is the one shown.
fn tab_button(ui: &mut egui::Ui, selected: bool, name: &str) -> egui::Response {
    let text = if selected { RichText::new(name).family(look::semibold()) } else { RichText::new(name) };
    let response = ui.add(Button::new(text).frame(false).min_size(egui::vec2(0.0, 30.0)));
    let color = if selected {
        ui.visuals().selection.bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_stroke.color
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().hline(response.rect.x_range(), response.rect.bottom() + 1.0, Stroke::new(2.0, color));
    response
}

/// One thing that is ready or not: a dot in `color`, then what it is.
fn ready(ui: &mut egui::Ui, color: Color32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    ui.label(text);
}

#[cfg(test)]
mod tests;
