//! The Monaka VR window: pick a game, set its options, start VR. Drawn with egui; the work is
//! `monaka_launch` (a session runs on its own thread and wakes the window with each event).
//!
//! Left: the games, adding and removing game packs, and the details log. Right: the game's status,
//! where it comes from and what to set before playing, its options by tab, what is ready (SteamVR,
//! the game), and Start VR. Options are saved as they change and shared with `monaka_play`.
//!
//! `monaka.exe --reload-packs` asks the open window to read its game packs again ([`reload`]),
//! as its Reload packs button does, and exits without a window.
#![windows_subsystem = "windows"]

mod look;
mod options;
mod reload;
mod screenshot;

use eframe::egui::{self, Align, Button, Layout, RichText, ScrollArea, TextEdit, ViewportCommand};
use monaka_core::manifest::{DEBUG_MENU, DIAGNOSTIC_LOGGING, DIAGNOSTIC_MARKER};
use monaka_launch::games::{self, Game, Launch, MONAKA, Shown, TABS};
use monaka_launch::session::{self, Console, Event, Shows, State, Stop};
use monaka_launch::settings::Settings;
use monaka_launch::{Paths, packs, system};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::core::{HSTRING, PCWSTR};

/// Where Monaka's releases are, for an update.
const RELEASES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases");
/// SteamVR's Steam app id.
const STEAMVR_APP: u32 = 250820;
/// Checks SteamVR and the game this often.
const CHECK_EVERY: Duration = Duration::from_millis(1500);
/// The details log is emptied past this many bytes.
const DETAILS_MAX: usize = 200_000;
/// The left column (the games and the details).
const LEFT_WIDTH: f32 = 250.0;
/// `--screenshot`: frames drawn before the picture, for sizes to settle.
const SCREENSHOT_FRAME: u32 = 5;
/// Clicks on Monaka's version on the About page that turn the debug menu on, each within
/// [`UNLOCK_GAP`] of the last.
const UNLOCK_CLICKS: u32 = 5;
const UNLOCK_GAP: Duration = Duration::from_millis(1500);
/// What the state line says while VR runs.
const RUNNING_LINE: &str = "VR is running. Put on your headset. Recentre with the headset's own control.";

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
    libraries: Vec<PathBuf>,
    /// The games on offer (Monaka's own built ones, then the packs'); may be empty.
    list: &'static [Game],
    selected: usize,
    tab: Shown,
    /// Scrolls the options here on the next frame (to the top after a game or tab change).
    scroll_to: Option<f32>,
    /// The About page shows in place of the game's.
    about: bool,
    /// Clicks on Monaka's version so far, and when the last was.
    version_clicks: (u32, Option<Instant>),
    /// The debug menu was turned on this run: its switch stays on the Monaka tab even once off.
    debug_unlocked: bool,

    running: Option<Running>,
    run_folder: Option<PathBuf>,
    closing: bool,
    checks: Checks,
    checked: Instant,
    state_line: String,
    details: String,

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
        look::start(ctx, screenshot.as_ref().and_then(|r| r.dark));
        let settings = Settings::load();
        let paths = Paths::locate();
        let catalogue = games::rescan(&paths);
        let list = catalogue.games;
        let mut selected = settings.selected.as_deref().and_then(|id| list.iter().position(|g| g.id == id)).unwrap_or(0);
        let mut tab = Shown::Main;
        let mut scroll_to = Some(0.0);
        let about = screenshot.as_ref().is_some_and(|r| r.tab.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("about")));
        if let Some(request) = &screenshot {
            tab = request.tab.as_deref().and_then(|name| TABS.iter().find(|(_, n)| n.eq_ignore_ascii_case(name))).map_or(Shown::Main, |(t, _)| *t);
            selected = request.game.as_deref().and_then(|id| list.iter().position(|g| g.id == id)).unwrap_or(selected);
            scroll_to = Some(request.scroll);
        }
        let mut app = App {
            ctx: ctx.clone(),
            paths,
            settings,
            libraries: system::steam_libraries(),
            list,
            selected,
            tab,
            scroll_to,
            about,
            version_clicks: (0, None),
            debug_unlocked: false,
            running: None,
            run_folder: None,
            closing: false,
            checks: Checks { game_installed: true, ..Default::default() },
            checked: Instant::now(),
            state_line: String::new(),
            details: String::new(),
            dialog: None,
            told_too_new: Vec::new(),
            screenshot: screenshot.map(|r| (r.file, 0)),
            reload_watch: reload::Watch::new(),
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
        self.about = false;
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
        let Some(file) = pick_pack_file() else { return };
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
                let _ = system::open(RELEASES);
            }
            Dialog::Remove { .. } | Dialog::Update(_) => {}
        }
    }

    /// Updates what is ready (SteamVR, the game).
    fn refresh_checks(&mut self) {
        self.checked = Instant::now();
        self.checks.steamvr_up = system::steamvr_running();
        self.checks.other_session = self.running.is_none() && system::viewer_running();
        let Some(game) = self.game() else { return };
        let runs_as = game.runs_as(&self.settings.values(game.id));
        let executables: Vec<&str> = runs_as.iter().map(String::as_str).collect();
        self.checks.game_up = !executables.is_empty() && !system::game_processes(&executables, game.steam_app).is_empty();
        // A pack for any game knows no Steam app.
        self.checks.game_installed = self.libraries.is_empty() || game.any_game() || system::steam_installed(&self.libraries, game.steam_app);
    }

    fn start_session(&mut self, shows: Shows) {
        let stop = match Stop::new() {
            Ok(stop) => stop,
            Err(why) => return self.say(&why, true),
        };
        let Some(game) = self.game() else { return };
        let (send_event, events) = mpsc::channel();
        let values = self.settings.values(game.id);
        let paths = self.paths.clone();
        let session_stop = stop.clone();
        let ctx = self.ctx.clone();
        self.details.clear();
        self.run_folder = None;
        std::thread::spawn(move || {
            session::run(game, &values, &paths, Console::Hidden, shows, &session_stop, &mut |event| {
                let _ = send_event.send(event);
                ctx.request_repaint();
            });
        });
        self.running = Some(Running { stop, events });
        self.say("Starting...", false);
        self.refresh_checks();
    }

    fn stop_session(&mut self) {
        if let Some(running) = &self.running {
            running.stop.request();
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

    /// Saves an option the player changed in the headset's Monaka menu, as its control here would:
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
        if let Err(why) = system::open(&folder.to_string_lossy()) {
            self.say(&why, true);
        }
    }

    /// Left: the games, adding and removing packs, then the details.
    fn games_panel(&mut self, ui: &mut egui::Ui) {
        let idle = self.running.is_none();
        ui.add_space(6.0);
        let mut picked = None;
        // A long list scrolls, leaving room for the details under it.
        let list_height = ui.available_height() * 0.5;
        egui::Frame::group(ui.style()).fill(ui.visuals().extreme_bg_color).show(ui, |ui| {
            ScrollArea::vertical().id_salt("games").max_height(list_height).show(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| {
                    ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                        for (i, game) in self.list.iter().enumerate() {
                            let text = format!("{}  ({})", game.name, game.status.label());
                            if ui.add(Button::selectable(i == self.selected, text).min_size(egui::vec2(0.0, 24.0))).clicked() {
                                picked = Some(i);
                            }
                        }
                        if self.list.is_empty() {
                            ui.label(RichText::new("No games yet").weak());
                        }
                    });
                });
            });
        });
        if let Some(i) = picked.filter(|i| *i != self.selected) {
            self.select(i);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(idle, Button::new("Add game pack...")).clicked() {
                self.add_pack();
            }
            let removable = idle && self.game().is_some_and(Game::removable);
            if ui.add_enabled(removable, Button::new("Remove pack")).clicked() {
                self.remove_pack();
            }
        });
        if ui.add_enabled(idle, Button::new("Reload packs")).clicked() {
            self.reload_packs();
        }
        ui.add_space(12.0);
        ui.label("Details");
        // Room under the details for the About link.
        let details_height = (ui.available_height() - 52.0).max(60.0);
        egui::Frame::group(ui.style()).fill(ui.visuals().extreme_bg_color).inner_margin(4.0).show(ui, |ui| {
            ScrollArea::vertical().id_salt("details").stick_to_bottom(true).auto_shrink(false).max_height(details_height).show(ui, |ui| {
                // Read-only, but selectable for copying.
                ui.add(TextEdit::multiline(&mut self.details.as_str()).desired_width(f32::INFINITY).frame(egui::Frame::NONE).font(egui::FontId::proportional(look::SMALL)));
            });
        });
        ui.add_space(6.0);
        if ui.selectable_label(self.about, "About").clicked() {
            self.about = !self.about;
        }
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

    /// Right, at the bottom: what is ready, Start VR, the state line.
    fn start_panel(&mut self, ui: &mut egui::Ui) {
        let idle = self.running.is_none();
        let checks = self.checks;
        let game = self.game();
        ui.add_space(8.0);
        let steamvr_line = match (checks.steamvr_up, checks.other_session) {
            (_, true) => "Another VR session is already running. Stop it first.",
            (true, false) => "SteamVR: running",
            (false, false) => "SteamVR: not running. Start it and turn on your headset.",
        };
        if ready_line(ui, steamvr_line, "Start SteamVR", !checks.steamvr_up)
            && let Err(why) = system::launch_steam_game(STEAMVR_APP)
        {
            self.say(&why, true);
        }
        let game_line = match game.map(|g| (checks.game_up, checks.game_installed, g.launch)) {
            None => "Game: none yet",
            Some((true, _, _)) => "Game: running",
            Some((false, false, _)) => "Game: not installed in Steam",
            Some((false, true, Launch::Auto { .. })) => "Game: not running (Start VR starts it)",
            Some((false, true, Launch::Manual)) => "Game: not running. Start it and load into gameplay first.",
            Some((false, _, Launch::AtStart)) => "Game: not running. Start VR, then the game: Monaka joins it as it starts.",
        };
        let can_start_game = idle && !checks.game_up && checks.game_installed && game.is_some_and(|g| !g.any_game());
        if ready_line(ui, game_line, "Start game", can_start_game)
            && let Some(game) = game
        {
            match system::launch_steam_game(game.steam_app) {
                Ok(()) => self.say(&format!("Starting {} through Steam...", game.name), false),
                Err(why) => self.say(&why, true),
            }
        }
        ui.add_space(10.0);
        let game_ready = checks.game_up || game.is_some_and(|g| g.launch == Launch::AtStart) || (checks.game_installed && game.is_some_and(|g| matches!(g.launch, Launch::Auto { .. })));
        let stopping = self.running.as_ref().is_some_and(|r| r.stop.requested());
        let can_press = if idle { game.is_some() && checks.steamvr_up && game_ready && !checks.other_session } else { !stopping && !self.closing };
        ui.horizontal(|ui| {
            let label = match (idle, stopping) {
                (true, _) => "Start VR",
                (false, false) => "Stop VR",
                (false, true) => "Stopping...",
            };
            let mut start = Button::new(RichText::new(label).family(look::semibold()).size(look::BIG)).min_size(egui::vec2(220.0, 44.0));
            if idle && can_press {
                start = start.fill(ui.visuals().selection.bg_fill);
            }
            if ui.add_enabled(can_press, start).clicked() {
                if idle {
                    self.start_session(Shows::Headset);
                } else {
                    self.stop_session();
                }
            }
            ui.add_space(4.0);
            // The 2D window: the run without a headset, for testers who have the debug menu.
            if (self.debug_menu_shown() || self.paths.dev_games.is_some())
                && idle
                && ui.add_enabled(game.is_some() && game_ready && !checks.other_session, Button::new("Debug without VR")).on_hover_text("The game in a window with the debug menu and its panels, no headset").clicked()
            {
                self.start_session(Shows::Desktop);
            }
            if ui.button("Open logs").clicked() {
                self.open_logs();
            }
            if ui.add_enabled(idle && game.is_some(), Button::new("Reset to defaults")).clicked() {
                self.reset_defaults();
            }
        });
        ui.add_space(6.0);
        ui.allocate_ui(egui::vec2(ui.available_width(), 40.0), |ui| {
            ui.set_min_height(40.0);
            ui.add(egui::Label::new(self.state_line.as_str()).wrap());
        });
    }

    /// Right, from the top: the game's texts, its tabs and the options.
    fn game_panel(&mut self, ui: &mut egui::Ui) {
        if self.about {
            return self.about_page(ui);
        }
        let Some(game) = self.game() else {
            // No game on offer yet: a Monaka without its own builds and no pack added.
            ui.heading("No games yet");
            ui.label(RichText::new("Add a game pack (a .zip from the people who made it) with Add game pack.").size(look::SMALL).weak());
            return;
        };
        ui.heading(game.name);
        ui.add_space(2.0);
        ui.label(format!("Status: {}", game.status.label()));
        ui.label(RichText::new(game.status.meaning()).size(look::SMALL).weak());
        let origin = game.credit();
        if !origin.is_empty() {
            ui.label(RichText::new(origin).size(look::SMALL).weak());
        }
        ui.add_space(4.0);
        ui.label(format!("Before you play: {}", game.before));
        ui.add_space(12.0);

        let rows = options::rows(game, self.debug_menu_shown());
        let tabs = options::tabs(game, &rows);
        // A tab the game lacks falls back to its first (Main, where it has one).
        if !tabs.contains(&self.tab) {
            self.tab = tabs.first().copied().unwrap_or(Shown::Main);
        }
        if tabs.len() > 1 {
            ui.horizontal(|ui| {
                for (tab, name) in TABS.iter().filter(|(t, _)| tabs.contains(t)) {
                    if ui.selectable_value(&mut self.tab, *tab, *name).clicked() {
                        self.scroll_to = Some(0.0);
                    }
                }
            });
            ui.separator();
        }
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
        self.session_events();
        self.reload_asked();
        if self.checked.elapsed() >= CHECK_EVERY {
            self.refresh_checks();
        }
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
        let margin = egui::Margin::same(14);
        let fill = ui.visuals().panel_fill;
        egui::Panel::left("games")
            .resizable(false)
            .exact_size(LEFT_WIDTH + 2.0 * margin.left as f32)
            .frame(egui::Frame::new().fill(fill).inner_margin(margin))
            .show(ui, |ui| self.games_panel(ui));
        egui::Panel::bottom("start").frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin { left: 6, right: 14, top: 0, bottom: 10 })).show(ui, |ui| self.start_panel(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin { left: 6, right: 14, top: 10, bottom: 0 })).show(ui, |ui| self.game_panel(ui));
        let ctx = ui.ctx().clone();
        self.dialog(&ctx);
    }
}

/// A readiness line with its button at the right; true when the button was pressed.
fn ready_line(ui: &mut egui::Ui, text: &str, button: &str, enabled: bool) -> bool {
    ui.horizontal(|ui| {
        ui.label(text);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.add_enabled(enabled, Button::new(button).min_size(egui::vec2(140.0, 26.0))).clicked()).inner
    })
    .inner
}

/// The player's pick of a game pack file (a .zip, or a lone .dll), if any.
fn pick_pack_file() -> Option<PathBuf> {
    let mut file = vec![0u16; 1024];
    let filter: Vec<u16> = "Game packs (*.zip, *.dll)\0*.zip;*.dll\0\0".encode_utf16().collect();
    let title = HSTRING::from("Add game pack");
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        // The button was just pressed, so ours is the active window: the dialog stays over it.
        // SAFETY: reads this thread's active window.
        hwndOwner: unsafe { GetActiveWindow() },
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: the dialog writes the chosen path into our buffer, which outlives the call.
    if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        return None;
    }
    let length = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(PathBuf::from(String::from_utf16_lossy(&file[..length])))
}
