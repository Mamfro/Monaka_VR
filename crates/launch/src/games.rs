//! Every game the launcher offers, and what a run makes of it on this PC: the headset's eye size,
//! the files other Steam games bring (a switch can take DLLs another game installs), the viewer's
//! arguments and the notes. No game has code of its own here: all of it is what the manifest
//! declares, so every game comes as a pack.
//!
//! Monaka compiles in no game. The games on offer are the producer DLLs a build tree's dev list
//! names ([`Paths::dev_games`]: game crates built on this PC, read as packs) and the packs in the
//! games folder ([`crate::packs`]), read from their DLLs ([`catalogue`]).
//!
//! A run's options file is one `id=value` line per producer option of the manifest
//! ([`Values::producer_lines`]), so the producer reads the launcher's own ids; the game's
//! `config.rs` says what each means to it.

pub use monaka_contract::manifest::{HowTo, Kind, Launch, MONAKA, Manifest, Opt, PLAY_WITH, Shown, Status, SteamFiles, TABS, Values, describe, number_text};
use crate::Paths;
use crate::packs::{self, Pack};
use monaka_contract::manifest::{DEBUG_MENU, DEBUG_MENU_ARG, OPTION_ARG};
use monaka_contract::manifest::aims;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use monaka_core::sync::Locked;

/// The headset size the producer should render at, when the player asked for one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EyeSize {
    GameOwn,
    /// SteamVR's recommended size (asked from the viewer).
    Headset,
    Fixed(u32, u32),
}

/// What one run writes and runs.
#[derive(Debug, Default)]
pub struct Plan {
    /// Files written beside the producer's DLL copy: (name, contents).
    pub files: Vec<(String, String)>,
    /// Extra viewer arguments.
    pub viewer_args: Vec<String>,
    /// Notices from the licence folder copied into the run folder.
    pub licences: Vec<&'static str>,
    /// Shown to the player when the run starts.
    pub hints: Vec<String>,
}

/// One game: its manifest, and the pack its producer comes from. Everything the launcher does for
/// a game comes from the manifest, so a game is what its pack declares (`monaka_contract::pack`).
pub struct Game {
    pub manifest: &'static Manifest,
    pub origin: Origin,
}

/// Where a game's pack comes from.
#[derive(Clone, Copy, Debug)]
pub enum Origin {
    /// A producer DLL built on this PC and named in the build tree's dev list.
    Local(&'static Pack),
    /// A pack the player added to the games folder.
    Pack(&'static Pack),
}

impl Game {
    /// The pack it comes from.
    pub fn pack(&self) -> &'static Pack {
        match self.origin {
            Origin::Local(pack) | Origin::Pack(pack) => pack,
        }
    }

    /// The producer DLL a run copies and injects.
    pub fn dll(&self, _paths: &Paths) -> PathBuf {
        self.pack().dll.clone()
    }

    /// The producer DLL for a game process that is `thirty_two_bit` or not. The pack's DLL is
    /// 64-bit; a pack for 32-bit games carries a 32-bit twin of the same name: in an `x86` folder
    /// beside it, or for a local build in the i686 target's release folder (`target\release` ->
    /// `target\i686-pc-windows-msvc\release`).
    pub fn dll_for(&self, paths: &Paths, thirty_two_bit: bool) -> PathBuf {
        let dll = self.dll(paths);
        if !thirty_two_bit {
            return dll;
        }
        let (Some(folder), Some(name)) = (dll.parent(), dll.file_name()) else { return dll };
        match self.origin {
            Origin::Local(_) => folder.parent().map_or_else(|| dll.clone(), |target| target.join(r"i686-pc-windows-msvc\release").join(name)),
            Origin::Pack(_) => folder.join("x86").join(name),
        }
    }

    /// Where the manifest's licence files are: beside a pack's DLL, or for a local build the
    /// build tree's licence folder.
    pub fn licence_folder(&self, paths: &Paths) -> PathBuf {
        match self.origin {
            Origin::Local(_) => paths.licences.clone(),
            Origin::Pack(pack) => pack.folder().to_owned(),
        }
    }

    /// Whether the player can remove it (a pack in the games folder; a local build goes when the
    /// dev list stops naming it).
    pub fn removable(&self) -> bool {
        matches!(self.origin, Origin::Pack(_))
    }

    /// Where it comes from, for the player: a local build, or a pack's own word on who made it.
    pub fn credit(&self) -> String {
        let pack = self.pack();
        match self.origin {
            Origin::Local(_) => format!("Built on this PC ({}), {}.", pack.dll.display(), packs::credit(&pack.about)),
            Origin::Pack(_) => format!("Game pack {}, as the pack says (contract {}).", packs::credit(&pack.about), pack.contract),
        }
    }
}

impl Deref for Game {
    type Target = Manifest;

    fn deref(&self) -> &Manifest {
        self.manifest
    }
}

/// The games on offer, and the packs that could not be used.
pub struct Catalogue {
    pub games: &'static [Game],
    /// One line per pack or dev list entry left out, saying why.
    pub problems: Vec<String>,
    /// The packs left out because they were made for a newer Monaka, by what the player knows
    /// them as.
    pub too_new: Vec<String>,
    /// The pack folders whose pack cannot run, for the list.
    pub left_out: Vec<packs::LeftOut>,
}

/// The games on offer with `paths`: the dev list's local builds in its order, then the packs in
/// the games folder (by folder). A pack offering a game that is already on offer is left out
/// whole, with why. The list lives for the rest of the process (a session holds its game), so
/// reading the packs again leaves the old one in memory.
pub fn catalogue(paths: &Paths) -> Catalogue {
    catalogue_with(paths, packs::read, packs::scan)
}

fn catalogue_with(paths: &Paths, read: impl Fn(&Path) -> Result<Pack, String>, scan: impl Fn(&Path) -> packs::Scan) -> Catalogue {
    let mut games: Vec<Game> = Vec::new();
    let (mut problems, mut too_new) = (Vec::new(), Vec::new());
    let offer = |pack: Pack, local: bool, games: &mut Vec<Game>, problems: &mut Vec<String>, too_new: &mut Vec<String>| {
        let pack: &'static Pack = Box::leak(Box::new(pack));
        let name = if local {
            format!("Local build {}", pack.dll.display())
        } else {
            format!("Game pack {}", pack.folder().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        };
        if let Some(why) = pack.unusable() {
            if pack.needs_newer_monaka() {
                too_new.push(format!("{} ({name})", pack.label()));
            }
            problems.push(format!("{name} ({}): {why}", pack.label()));
            return;
        }
        if let Some((mine, other)) = pack.manifests.iter().find_map(|m| games.iter().find(|g| g.id.eq_ignore_ascii_case(m.id)).map(|g| (m, g))) {
            problems.push(format!("{name}: {} ({}) is already offered by {}; it is not used.", mine.name, mine.id, offered_by(other)));
            return;
        }
        let origin = if local { Origin::Local(pack) } else { Origin::Pack(pack) };
        games.extend(pack.manifests.iter().map(|manifest| Game { manifest, origin }));
    };
    if let Some(list) = &paths.dev_games {
        for dll in dev_list(list, &paths.binaries) {
            match read(&dll) {
                Ok(pack) => offer(pack, true, &mut games, &mut problems, &mut too_new),
                Err(why) => problems.push(format!("Dev list {}: {why}", list.display())),
            }
        }
    }
    let scan = scan(&paths.packs);
    problems.extend(scan.problems);
    too_new.extend(scan.too_new);
    let left_out = scan.left_out;
    for pack in scan.packs {
        offer(pack, false, &mut games, &mut problems, &mut too_new);
    }
    Catalogue { games: Box::leak(games.into_boxed_slice()), problems, too_new, left_out }
}

/// The DLLs a dev list names: one per line, a file name beside the launcher or a full path;
/// blank lines and lines starting with `#` skipped. No list, no DLLs.
fn dev_list(list: &Path, binaries: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(list) else { return Vec::new() };
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| binaries.join(l)).collect()
}

/// "a local build (path)" or "the game pack <folder>", for messages.
fn offered_by(game: &Game) -> String {
    let pack = game.pack();
    match game.origin {
        Origin::Local(_) => format!("a local build ({})", pack.dll.display()),
        Origin::Pack(_) => format!("the game pack {}", pack.folder().display()),
    }
}

/// The list [`list`] and [`game`] answer from, once read.
static CURRENT: Mutex<Option<&'static [Game]>> = Mutex::new(None);

/// The games on offer for this launcher ([`catalogue`] with [`Paths::locate`]), read once.
pub fn list() -> &'static [Game] {
    let mut current = CURRENT.locked();
    current.get_or_insert_with(|| catalogue(&Paths::locate()).games)
}

/// Reads the games again (after a pack is added or removed) and returns them with the problems.
pub fn rescan(paths: &Paths) -> Catalogue {
    let catalogue = catalogue(paths);
    *CURRENT.locked() = Some(catalogue.games);
    catalogue
}

pub fn game(id: &str) -> Option<&'static Game> {
    list().iter().find(|g| g.id.eq_ignore_ascii_case(id))
}

/// The names of every game on offer from the pack folder `folder` (one folder may hold several
/// producer DLLs), for messages.
pub fn names_in(folder: &Path) -> String {
    list().iter().filter(|g| g.removable() && g.pack().folder() == folder).map(|g| g.name).collect::<Vec<_>>().join(", ")
}

/// The games a pack would clash with if added: those on offer from a local build or another pack
/// (an update of the same pack, `replacing`, clashes with nothing of its own).
pub fn clashes<'a>(offered: impl Iterator<Item = &'a Manifest>, replacing: Option<&Path>) -> Vec<String> {
    offered
        .filter_map(|m| {
            let other = list().iter().find(|g| g.id.eq_ignore_ascii_case(m.id))?;
            if matches!(other.origin, Origin::Pack(p) if Some(p.folder()) == replacing) {
                return None;
            }
            Some(format!("{} is already offered by {}; remove that first.", m.name, offered_by(other)))
        })
        .collect()
}

/// The eye size a run of `game` asks for: a size of the player's own (`eye_size`), else what
/// `render_size` says, else the game's own.
pub fn eye_size(values: &Values, game: &Manifest) -> EyeSize {
    if let Some((w, h)) = values.size(game, "eye_size") {
        return EyeSize::Fixed(w, h);
    }
    if game.option("render_size").is_none() {
        return EyeSize::GameOwn;
    }
    match values.text(game, "render_size").as_str() {
        "headset" => EyeSize::Headset,
        "game" => EyeSize::GameOwn,
        size => monaka_contract::manifest::parse_size(size).map_or(EyeSize::Headset, |(w, h)| EyeSize::Fixed(w, h)),
    }
}

/// What a run of `game` with `values` writes and runs: the options file from the manifest (a
/// switch whose needs are not met written off, with why), the eye size the launcher settled, the
/// folders of files other games bring, the licences, the viewer's arguments and the notes.
pub fn plan(values: &Values, game: &Manifest, eye: Option<(u32, u32)>) -> Plan {
    plan_with(values, game, eye, find_steam_files)
}

/// What the launcher makes of `args` (`id=value`, as `monaka_play` takes them) for `game`, each
/// checked against its options: for a game crate's launch tests (a bad argument panics).
pub fn values_of(game: &Manifest, args: &[&str]) -> Values {
    let mut values = Values::default();
    for arg in args {
        let (id, value) = arg.split_once('=').unwrap_or_else(|| panic!("{arg} is not id=value"));
        values.set_checked(game, id, value).unwrap_or_else(|why| panic!("{why}"));
    }
    values
}

/// The options file a run of `game` with `values` writes (no eye size settled): for a game
/// crate's launch tests.
pub fn options_file(game: &Manifest, values: &Values) -> String {
    let plan = plan(values, game, None);
    let (name, text) = plan.files.into_iter().next().unwrap_or_else(|| panic!("{} writes no options file", game.id));
    assert_eq!(name, game.options_file);
    text
}

/// The viewer's arguments for a run of `game` with `args` but the options' lines (the rings and
/// switches): for a game crate's launch tests.
pub fn viewer_args_of(game: &Manifest, args: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut words = plan(&values_of(game, args), game, None).viewer_args.into_iter();
    while let Some(word) = words.next() {
        if word == OPTION_ARG {
            words.next();
        } else {
            out.push(word);
        }
    }
    out
}

/// The `id=value` lines a run of `game` with `args` sends the viewer from the game's own options
/// (Monaka's own, the same in every game, left out): for a game crate's launch tests.
pub fn viewer_lines_of(game: &Manifest, args: &[&str]) -> Vec<String> {
    viewer_lines(game, &values_of(game, args))
}

/// Panics unless every line the defaults of each of `manifests` write is one of its options: for
/// a game crate's launch tests.
pub fn defaults_are_options(manifests: &[Manifest]) {
    for game in manifests {
        let defaults = options_file(game, &Values::default());
        assert!(defaults.lines().all(|l| l.split_once('=').is_some_and(|(k, _)| game.option(k).is_some())), "{}: {defaults}", game.id);
    }
}

/// [`plan`] with `find` in place of looking through the Steam libraries for another game's files
/// (for tests, the game crates' own among them).
pub fn plan_with(values: &Values, game: &Manifest, eye: Option<(u32, u32)>, find: impl Fn(&SteamFiles) -> Option<std::path::PathBuf>) -> Plan {
    // Developer options hidden from the player are at their defaults.
    let mut values = values.for_run(&[game, &MONAKA]);
    let mut plan = Plan { licences: game.licences.to_vec(), ..Plan::default() };
    let mut found = Vec::new();
    for opt in game.options.iter().filter(|o| matches!(o.kind, Kind::Switch { .. })) {
        if !values.switch(game, opt.id) {
            continue;
        }
        if opt.needs_eye_size && eye.is_none() {
            values.set(opt.id, "0");
            plan.hints.push(format!("{} is off: it needs the headset resolution.", opt.label));
            continue;
        }
        if let Some(files) = opt.needs_files {
            match find(&files) {
                Some(folder) => found.push(format!("{}={}", files.line, folder.display())),
                None => {
                    values.set(opt.id, "0");
                    plan.hints.push(files.missing.to_owned());
                }
            }
        }
    }
    let mut lines = values.producer_lines(game);
    if let Some((w, h)) = eye {
        lines.push(format!("eye_size={w}x{h}"));
    }
    lines.extend(found);
    plan.viewer_args = reticles(game, &values);
    if values.switch(&MONAKA, DEBUG_MENU.id) {
        plan.viewer_args.push(DEBUG_MENU_ARG.to_owned());
    }
    for owner in [game, &MONAKA] {
        plan.viewer_args.extend(viewer_lines(owner, &values).into_iter().flat_map(|line| [OPTION_ARG.to_owned(), line]));
    }
    for opt in game.options {
        if let (Some(note), Some((w, h))) = (opt.eye_size_note, eye) {
            plan.hints.push(note.replace("{size}", &format!("{w}x{h}")));
        }
    }
    plan.files.push((game.options_file.to_owned(), lines.join("\n")));
    plan
}

/// The `id=value` lines `owner`'s options send the viewer with `values` (a game's, or Monaka's
/// own), each to go after [`OPTION_ARG`]: an option's own value ([`Opt::to_viewer`]) and the
/// lines it sends at a value ([`Opt::viewer_lines`]).
fn viewer_lines(owner: &Manifest, values: &Values) -> Vec<String> {
    let mut lines = Vec::new();
    for opt in owner.options.iter().filter(|opt| values.applies(owner, opt)) {
        let current = values.text(owner, opt.id);
        if opt.to_viewer {
            lines.push(format!("{}={current}", opt.id));
        }
        if let Some((value, sent)) = opt.viewer
            && (value == "*" || current == value)
        {
            lines.extend(sent.iter().map(|line| (*line).to_owned()));
        }
    }
    lines
}

/// The folder in some Steam library that holds `files`.
fn find_steam_files(files: &SteamFiles) -> Option<std::path::PathBuf> {
    crate::system::steam_libraries()
        .into_iter()
        .map(|library| library.join(r"steamapps\common").join(files.game_folder).join(files.path))
        .find(|folder| folder.join(files.file).exists())
}

/// The viewer's aim rings: on the aiming controller (the chosen one for a pointing controller,
/// else the right, from the palm with motion controls), and on the left controller for the left
/// hand's own aim where the game has one (`left_hand_aims`); a sword gets none (`weapon_ring`).
fn reticles(game: &Manifest, values: &Values) -> Vec<String> {
    let aim = values.text(game, "aim");
    // A game without an aiming-hand choice aims with the right.
    let hand = if aim == aims::CONTROLLER && values.text(game, "aim_hand") == "left" { "left" } else { "right" }.to_owned();
    let mut rings: Vec<&str> = Vec::new();
    if aim == aims::CONTROLLER || (aim == aims::HAND_RIG && game.weapon_ring) {
        rings.push(&hand);
    }
    if game.left_hand_aims && hand == "right" && (aim == aims::HAND_RIG || (aim == aims::CONTROLLER && !rings.is_empty())) {
        rings.push("left");
    }
    let mut args: Vec<String> = rings.iter().flat_map(|ring| ["--reticle".to_owned(), (*ring).to_owned()]).collect();
    if aim == aims::HAND_RIG && !args.is_empty() {
        args.extend(["--reticle-from".into(), "palm".into()]);
    }
    args
}

#[cfg(test)]
mod tests;
