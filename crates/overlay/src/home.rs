//! Monaka's home in the headset: three flat panels round the player, Games, Settings and Log,
//! pointed at with a controller ([`place`]). The launcher leaves what they show in a folder
//! (`monaka_core::home`): the game's manifest, every setting's value and the games on offer; the
//! logs are the run's own and the game's. A setting changed here goes back to the launcher, which
//! saves it; one the viewer acts on ([`LIVE`]) applies at once, the rest with the next launch.
//! The Monaka window's own headset view reads that folder again as the window changes it
//! ([`Home::sync`]), and asks the window to show, play and stop games. [`pages`] draws them.

use monaka_contract::manifest::{DEBUG_MENU, DEV_MODE, Kind, LIVE, MONAKA, Manifest, Opt, Shown, TABS, Values};
use monaka_contract::pack::{self, About};
use monaka_core::home::{self as files, Listed};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub mod pages;
pub mod place;

#[cfg(test)]
mod tests;

/// The panels, left to right round the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Games,
    Settings,
    Log,
}

pub const PAGES: [Page; 3] = [Page::Games, Page::Settings, Page::Log];

impl Page {
    pub fn title(self) -> &'static str {
        match self {
            Page::Games => "Games",
            Page::Settings => "Settings",
            Page::Log => "Log",
        }
    }
}

/// One setting the Settings panel offers: whose it is (the game's, or Monaka's own) and which.
#[derive(Clone, Copy)]
pub struct Setting {
    pub owner: &'static Manifest,
    pub opt: &'static Opt,
}

impl Setting {
    /// Whether a change applies at once (the viewer acts on it) rather than with the next launch.
    pub fn live(&self) -> bool {
        LIVE.iter().any(|live| live.id == self.opt.id)
    }

    /// Whether the panel can change it (text and sizes are typed in the window).
    pub fn editable(&self) -> bool {
        !matches!(self.opt.kind, Kind::Text | Kind::Size)
    }
}

/// Which log the Log panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogSource {
    /// The run's own (`monaka.log`: the launcher and the viewer).
    Monaka,
    /// The game's producer.
    Game,
}

/// The newest lines of a log file, read again now and then.
pub struct LogTail {
    monaka: Option<PathBuf>,
    game: Option<PathBuf>,
    pub source: LogSource,
    pub lines: Vec<String>,
    read_at: Option<Instant>,
}

/// How much of a log's end is read, how many lines are kept, and how often it is read again.
const TAIL_BYTES: u64 = 48 * 1024;
const TAIL_LINES: usize = 300;
const TAIL_EVERY: Duration = Duration::from_secs(1);

impl LogTail {
    /// The logs in `run_folder`: the run's own and the game's (`game_log`).
    pub fn new(run_folder: Option<&Path>, game_log: Option<&str>) -> Self {
        let monaka = run_folder.map(|f| f.join("monaka.log"));
        let game = run_folder.zip(game_log.filter(|l| !l.is_empty())).map(|(f, l)| f.join(l));
        Self { monaka, game, source: LogSource::Monaka, lines: Vec::new(), read_at: None }
    }

    /// Shows `source` from now on.
    pub fn show(&mut self, source: LogSource) {
        if self.source != source {
            self.source = source;
            self.read_at = None;
        }
    }

    /// Reads the shown log again when it is time; whether its lines changed.
    pub fn refresh(&mut self, now: Instant) -> bool {
        if self.read_at.is_some_and(|at| now.duration_since(at) < TAIL_EVERY) {
            return false;
        }
        self.read_at = Some(now);
        let file = match self.source {
            LogSource::Monaka => self.monaka.as_deref(),
            LogSource::Game => self.game.as_deref(),
        };
        let lines = match file {
            Some(file) => tail(file).unwrap_or_else(|| vec![format!("{} is not there yet.", file.display())]),
            None => vec!["No log: the viewer was started without a run folder.".to_owned()],
        };
        let changed = lines != self.lines;
        self.lines = lines;
        changed
    }
}

/// The last lines of `file` (at most [`TAIL_LINES`], from its last [`TAIL_BYTES`]).
fn tail(file: &Path) -> Option<Vec<String>> {
    let mut opened = std::fs::File::open(file).ok()?;
    let length = opened.metadata().ok()?.len();
    let from = length.saturating_sub(TAIL_BYTES);
    opened.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    opened.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // A cut first line is left out.
    let mut lines: Vec<&str> = text.lines().skip(usize::from(from > 0)).collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    Some(lines.drain(start..).map(str::to_owned).collect())
}

pub struct Home {
    /// The game whose settings show (the one this run plays, or the one picked in the window).
    pub game: Option<&'static Manifest>,
    /// Who made its pack, as the pack says.
    pub about: About,
    pub games: Vec<Listed>,
    /// Play as the window has it (the window's own view).
    pub status: files::Status,
    /// Every setting's value as saved (the launcher's, then the player's changes here).
    pub values: Values,
    /// The Settings panel's tab.
    pub tab: Shown,
    pub log: LogTail,
    /// The panel in front of the player ([`PAGES`]).
    pub front: usize,
    /// The debug menu is on for this run: the Log panel offers its tools.
    pub debug: bool,
    /// The Monaka window's own headset view: games are picked, played and stopped from here.
    pub window: bool,
    changes: Vec<(&'static str, String)>,
    /// Settings changed that the game takes only when it starts again ([`Home::ask_apply`]).
    pub pending: Vec<&'static str>,
    /// The game is starting again with them; and what went wrong last time, if anything did.
    pub applying: bool,
    pub problem: Option<String>,
    apply: bool,
    stop: bool,
    recenter: bool,
    capture: bool,
    debug_menu: bool,
    select: Option<String>,
    play: bool,
    start_game: bool,
    leave: bool,
    /// The files' texts as last read ([`Home::sync`]): the manifest, the values, the games, the
    /// status.
    read: [String; 4],
    /// Panels whose image is out of date.
    stale: [bool; 3],
}

/// The files the home is made from, in [`Home::read`]'s order.
const FILES: [&str; 4] = [files::MANIFEST_FILE, files::VALUES_FILE, files::GAMES_FILE, files::STATUS_FILE];

impl Home {
    /// The home of the run in `run_folder`, from the files the launcher left there (none: a viewer
    /// started by hand, with nothing to set).
    pub fn load(run_folder: Option<&Path>) -> Self {
        let mut home = Self {
            game: None,
            about: About::default(),
            games: Vec::new(),
            status: files::Status::default(),
            values: Values::default(),
            tab: Shown::Main,
            log: LogTail::new(run_folder, None),
            front: 1,
            debug: false,
            window: false,
            changes: Vec::new(),
            pending: Vec::new(),
            applying: false,
            problem: None,
            apply: false,
            stop: false,
            recenter: false,
            capture: false,
            debug_menu: false,
            select: None,
            play: false,
            start_game: false,
            leave: false,
            read: Default::default(),
            stale: [true; 3],
        };
        if let Some(folder) = run_folder {
            home.sync(folder);
            home.log = LogTail::new(run_folder, home.game.map(|g| g.log));
        }
        home
    }

    /// Reads the files in `folder` again and takes what changed in them (the window picked another
    /// game, changed a setting, started or stopped one); whether anything did.
    pub fn sync(&mut self, folder: &Path) -> bool {
        let texts = FILES.map(|name| std::fs::read_to_string(folder.join(name)).unwrap_or_default());
        if texts == self.read {
            return false;
        }
        if texts[0] != self.read[0] {
            let parsed = pack::parse(&texts[0]).map_err(|e| println!("Monaka home: {}: {e}", files::MANIFEST_FILE)).ok().filter(|_| !texts[0].is_empty());
            let (game, about) = match parsed {
                Some(parsed) => (parsed.manifests.into_iter().next().map(|m| &*Box::leak(Box::new(m))), parsed.about),
                None => (None, About::default()),
            };
            // Another game's settings: what waited to apply was the last one's.
            if game.map(|g| g.id) != self.game.map(|g| g.id) {
                self.pending.clear();
                self.problem = None;
            }
            self.game = game;
            self.about = about;
        }
        if texts[1] != self.read[1] {
            self.values = files::parse_values(&texts[1]);
        }
        if texts[2] != self.read[2] {
            self.games = files::parse_games(&texts[2]);
        }
        if texts[3] != self.read[3] {
            self.status = files::parse_status(&texts[3]);
        }
        self.read = texts;
        let tabs = self.tabs();
        if !tabs.contains(&self.tab) {
            self.tab = tabs.first().copied().unwrap_or(Shown::Monaka);
        }
        self.stale = [true; 3];
        true
    }

    /// The Log panel shows the logs in `run_folder` from now on: the run that plays now, or none.
    pub fn show_run(&mut self, run_folder: Option<&Path>) {
        let source = self.log.source;
        self.log = LogTail::new(run_folder, self.game.map(|g| g.log));
        self.log.show(source);
        self.stale[2] = true;
    }

    /// The game shown in the headset now, as the launcher listed it.
    pub fn in_vr(&self) -> Option<&Listed> {
        self.games.iter().find(|g| g.in_vr)
    }

    /// Whether the game shown here runs already: Play joins it (Attach) rather than starting it.
    pub fn current_runs(&self) -> bool {
        self.games.iter().any(|g| g.current && g.running)
    }

    /// Developer mode: the settings for testers show.
    pub fn dev(&self) -> bool {
        self.values.switch(&MONAKA, DEV_MODE.id)
    }

    /// The settings on offer, in the window's terms: the game's, then Monaka's own; none that only
    /// the command line sets, none for testers outside developer mode, the debug menu's switch only
    /// once it is on, and none that means nothing with the others' values.
    pub fn settings(&self) -> Vec<Setting> {
        let dev = self.dev();
        let debug_menu = self.values.switch(&MONAKA, DEBUG_MENU.id);
        let game = self.game.into_iter().flat_map(|owner| owner.options.iter().filter(|o| o.shown != Shown::Monaka).map(move |opt| Setting { owner, opt }));
        let monaka = MONAKA.options.iter().map(|opt| Setting { owner: &MONAKA, opt });
        game.chain(monaka)
            .filter(|s| s.opt.shown != Shown::Line && (dev || !s.opt.dev) && (debug_menu || s.opt.id != DEBUG_MENU.id))
            .filter(|s| self.values.applies(s.owner, s.opt))
            .filter(|s| !matches!(s.opt.kind, Kind::Choice { .. }) || self.values.shown_choices(s.owner, s.opt).len() > 1)
            .collect()
    }

    /// The tabs that have settings, in the window's order.
    pub fn tabs(&self) -> Vec<Shown> {
        let settings = self.settings();
        TABS.iter().map(|(tab, _)| *tab).filter(|tab| settings.iter().any(|s| s.opt.shown == *tab)).collect()
    }

    /// A setting's value as the panel shows it.
    pub fn value(&self, setting: &Setting) -> String {
        self.values.text(setting.owner, setting.opt.id)
    }

    /// The player set `setting` to `value`: kept, and told to the viewer ([`Home::take_changes`]).
    pub fn set(&mut self, setting: &Setting, value: String) {
        if self.value(setting) == value {
            return;
        }
        self.values.set(setting.opt.id, value.clone());
        self.changes.push((setting.opt.id, value));
        // With no game of its own playing, a change simply applies when the game starts.
        if !setting.live() && self.can_apply() && !self.pending.contains(&setting.opt.id) {
            self.pending.push(setting.opt.id);
        }
        self.stale[1] = true;
    }

    /// The settings changed since last asked: each id and its new value.
    pub fn take_changes(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.changes)
    }

    /// Whether the settings waiting can be applied here: their game is the one that plays.
    pub fn can_apply(&self) -> bool {
        self.game.is_some() && self.games.iter().any(|g| g.current && g.in_vr)
    }

    /// The player asked for the settings waiting to apply now: the game's part of Monaka starts
    /// again with them (the game itself keeps running).
    pub fn ask_apply(&mut self) {
        if self.can_apply() && !self.applying {
            self.apply = true;
        }
    }

    pub fn take_apply(&mut self) -> bool {
        let asked = std::mem::take(&mut self.apply);
        if asked {
            self.applying = true;
            self.problem = None;
            self.stale[1] = true;
        }
        asked
    }

    /// The game is back with the settings applied (`problem`: or it is not, and why).
    pub fn applied(&mut self, problem: Option<String>) {
        if problem.is_none() {
            self.pending.clear();
            self.applying = false;
        }
        self.problem = problem;
        self.stale[1] = true;
    }

    /// The player asked to stop VR.
    pub fn ask_stop(&mut self) {
        self.stop = true;
    }

    pub fn take_stop(&mut self) -> bool {
        std::mem::take(&mut self.stop)
    }

    /// The player picked game `id` in the list: the window shows it (and its settings show here).
    pub fn ask_select(&mut self, id: &str) {
        self.select = Some(id.to_owned());
    }

    pub fn take_select(&mut self) -> Option<String> {
        self.select.take()
    }

    /// The player asked to play the game shown.
    pub fn ask_play(&mut self) {
        self.play = true;
    }

    pub fn take_play(&mut self) -> bool {
        std::mem::take(&mut self.play)
    }

    /// The player asked to start the game shown through Steam.
    pub fn ask_start_game(&mut self) {
        self.start_game = true;
    }

    pub fn take_start_game(&mut self) -> bool {
        std::mem::take(&mut self.start_game)
    }

    /// The player asked to leave VR: the headset view closes (the window stays).
    pub fn ask_leave(&mut self) {
        self.leave = true;
    }

    pub fn take_leave(&mut self) -> bool {
        std::mem::take(&mut self.leave)
    }

    /// The player asked for the panels round where the head is now.
    pub fn ask_recenter(&mut self) {
        self.recenter = true;
    }

    pub fn take_recenter(&mut self) -> bool {
        std::mem::take(&mut self.recenter)
    }

    /// The player asked for a capture of what the headset shows.
    pub fn ask_capture(&mut self) {
        self.capture = true;
    }

    pub fn take_capture(&mut self) -> bool {
        std::mem::take(&mut self.capture)
    }

    /// The player asked for the debug menu (the home closes for it).
    pub fn ask_debug_menu(&mut self) {
        self.debug_menu = true;
    }

    pub fn take_debug_menu(&mut self) -> bool {
        std::mem::take(&mut self.debug_menu)
    }

    /// Turns the ring by `steps` (negative: to the left), bringing another panel in front.
    pub fn turn(&mut self, steps: isize) {
        self.front = (self.front as isize + steps).rem_euclid(PAGES.len() as isize) as usize;
        self.stale = [true; 3];
    }

    /// The Settings panel shows `tab`.
    pub fn show_tab(&mut self, tab: Shown) {
        if self.tab != tab {
            self.tab = tab;
            self.stale[1] = true;
        }
    }

    /// The Log panel shows `source`.
    pub fn show_log(&mut self, source: LogSource) {
        self.log.show(source);
        self.stale[2] = true;
    }

    /// Reads the shown log again when it is time.
    pub fn refresh(&mut self, now: Instant) {
        if self.log.refresh(now) {
            self.stale[2] = true;
        }
    }

    /// Whether panel `page` must be drawn again; it is taken as drawn.
    pub fn take_stale(&mut self, page: usize) -> bool {
        std::mem::take(&mut self.stale[page])
    }

    /// Every panel must be drawn again (their images went somewhere new).
    pub fn all_stale(&mut self) {
        self.stale = [true; 3];
    }
}
