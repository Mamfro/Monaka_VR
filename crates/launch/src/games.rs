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

pub use monaka_core::manifest::{Kind, Launch, Manifest, Opt, Shown, Status, SteamFiles, TABS, Values, describe, number_text};
use crate::Paths;
use crate::packs::{self, Pack};
use monaka_core::manifest::aims;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
/// a game comes from the manifest, so a game is what its pack declares (`monaka_core::pack`).
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
    let mut problems = Vec::new();
    let offer = |pack: Pack, local: bool, games: &mut Vec<Game>, problems: &mut Vec<String>| {
        let pack: &'static Pack = Box::leak(Box::new(pack));
        let name = if local {
            format!("Local build {}", pack.dll.display())
        } else {
            format!("Game pack {}", pack.folder().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        };
        if let Some(why) = pack.unusable() {
            problems.push(format!("{name} ({}): {why}", pack.names()));
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
                Ok(pack) => offer(pack, true, &mut games, &mut problems),
                Err(why) => problems.push(format!("Dev list {}: {why}", list.display())),
            }
        }
    }
    let scan = scan(&paths.packs);
    problems.extend(scan.problems);
    for pack in scan.packs {
        offer(pack, false, &mut games, &mut problems);
    }
    Catalogue { games: Box::leak(games.into_boxed_slice()), problems }
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
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    current.get_or_insert_with(|| catalogue(&Paths::locate()).games)
}

/// Reads the games again (after a pack is added or removed) and returns them with the problems.
pub fn rescan(paths: &Paths) -> Catalogue {
    let catalogue = catalogue(paths);
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(catalogue.games);
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
        size => monaka_core::options::parse_size(size).map_or(EyeSize::Headset, |(w, h)| EyeSize::Fixed(w, h)),
    }
}

/// What a run of `game` with `values` writes and runs: the options file from the manifest (a
/// switch whose needs are not met written off, with why), the eye size the launcher settled, the
/// folders of files other games bring, the licences, the viewer's arguments and the notes.
pub fn plan(values: &Values, game: &Manifest, eye: Option<(u32, u32)>) -> Plan {
    plan_with(values, game, eye, find_steam_files)
}

/// [`plan`] with `find` in place of looking through the Steam libraries for another game's files
/// (for tests, the game crates' own among them).
pub fn plan_with(values: &Values, game: &Manifest, eye: Option<(u32, u32)>, find: impl Fn(&SteamFiles) -> Option<std::path::PathBuf>) -> Plan {
    let mut values = values.clone();
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
    for opt in game.options {
        if let Some((value, args)) = opt.viewer
            && values.applies(game, opt)
        {
            let current = values.text(game, opt.id);
            if value == "*" && current != opt.default_text() {
                plan.viewer_args.extend(args.iter().map(|a| (*a).to_owned()).chain([current]));
            } else if current == value {
                plan.viewer_args.extend(args.iter().map(|a| (*a).to_owned()));
            }
        }
        if let (Some(note), Some((w, h))) = (opt.eye_size_note, eye) {
            plan.hints.push(note.replace("{size}", &format!("{w}x{h}")));
        }
    }
    plan.files.push((game.options_file.to_owned(), lines.join("\n")));
    plan
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
    let hand = if aim == aims::CONTROLLER { values.text(game, "aim_hand") } else { "right".to_owned() };
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
mod tests {
    use super::*;
    use monaka_core::manifest::*;
    use monaka_core::pack::{About, CONTRACT};

    /// A game with one of each thing the plan reads: the aim and its hand, a render size, a
    /// switch that needs the eye size and another game's files, a choice that passes viewer
    /// arguments, a note shown with the eye size, and a command-line-only extra.
    const GAME: Manifest = Manifest {
        id: "t",
        name: "Test",
        status: Status::Experimental,
        executables: &["t.exe"],
        steam_app: 1,
        producer: "monaka_t",
        log: "t.log",
        options_file: "t-options.txt",
        run_prefix: "t",
        launch: Launch::Manual,
        refused_arguments: &[],
        before: "",
        options: &[
            aim(&[AIM_MOTION, AIM_LOOK, AIM_POINT, AIM_MOUSE], aims::HAND_RIG),
            AIM_HAND,
            render_size("Size."),
            opt("upscale", "Upscale", "Needs both.", Kind::Switch { default: false }).on(Shown::Engine).needs_eye_size().needs_steam_files(SteamFiles {
                game_folder: "Other Game",
                path: r"bin\x64",
                file: "a.dll",
                line: "a_dlls",
                missing: "No A.",
            }),
            opt("mode", "Mode", "", choice(&[("plain", "Plain"), ("deep", "Deep")], "plain")).viewer_args("deep", &["--panel-nearest", "0.5"]).eye_size_note("Rendering at {size}."),
            EXTRA,
            OTHER_BUILDS,
        ],
        licences: &["T-LICENSE.txt"],
        left_hand_aims: true,
        weapon_ring: true,
    };

    fn parsed(args: &[&str]) -> Values {
        let mut values = Values::default();
        for arg in args {
            let (id, value) = arg.split_once('=').unwrap();
            values.set_checked(&GAME, id, value).unwrap_or_else(|why| panic!("{why}"));
        }
        values
    }

    #[test]
    fn the_plan_writes_what_the_manifest_declares() {
        GAME.check().unwrap();
        let plan = plan(&Values::default(), &GAME, Some((2000, 2000)));
        assert_eq!(plan.files, [("t-options.txt".to_owned(), "aim=hand_rig\nupscale=0\nmode=plain\nother_builds=0\neye_size=2000x2000".to_owned())]);
        assert_eq!(plan.licences, ["T-LICENSE.txt"]);
        assert_eq!(plan.hints, ["Rendering at 2000x2000."]);
        // Extra pairs go last, junk dropped.
        let text = &super::plan(&parsed(&["extra=probe=1 junk x=0,1"]), &GAME, None).files[0].1;
        assert!(text.ends_with("other_builds=0\nprobe=1\nx=0,1"), "{text}");
    }

    #[test]
    fn a_switch_is_written_off_when_its_needs_are_not_met() {
        let on = parsed(&["upscale=on"]);
        let found = |_: &SteamFiles| Some(PathBuf::from(r"D:\Other Game\bin\x64"));
        let both = plan_with(&on, &GAME, Some((2000, 2000)), found);
        assert!(both.files[0].1.contains("upscale=1") && both.files[0].1.ends_with(r"a_dlls=D:\Other Game\bin\x64"), "{}", both.files[0].1);
        let missing = plan_with(&on, &GAME, Some((2000, 2000)), |_| None);
        assert!(missing.files[0].1.contains("upscale=0") && missing.hints.contains(&"No A.".to_owned()), "{:?}", missing);
        let no_eye = plan_with(&on, &GAME, None, found);
        assert!(no_eye.files[0].1.contains("upscale=0") && no_eye.hints[0].contains("headset resolution"), "{:?}", no_eye.hints);
    }

    #[test]
    fn rings_follow_the_aim() {
        let args = |a: &[&str]| plan(&parsed(a), &GAME, None).viewer_args;
        assert_eq!(args(&["aim=hand_rig"]), ["--reticle", "right", "--reticle", "left", "--reticle-from", "palm"]);
        assert_eq!(args(&["aim=controller"]), ["--reticle", "right", "--reticle", "left"]);
        assert_eq!(args(&["aim=controller", "aim_hand=left"]), ["--reticle", "left"]);
        assert!(args(&["aim=head"]).is_empty() && args(&["aim=off"]).is_empty());
        // A choice's viewer arguments.
        assert_eq!(args(&["aim=head", "mode=deep"]), ["--panel-nearest", "0.5"]);
    }

    #[test]
    fn the_eye_size_is_the_players_or_the_render_sizes() {
        assert_eq!(eye_size(&Values::default(), &GAME), EyeSize::Headset);
        assert_eq!(eye_size(&parsed(&["render_size=game"]), &GAME), EyeSize::GameOwn);
        assert_eq!(eye_size(&parsed(&["render_size=2160x2160"]), &GAME), EyeSize::Fixed(2160, 2160));
        let without = Manifest { options: &[], ..GAME };
        assert_eq!(eye_size(&Values::default(), &without), EyeSize::GameOwn);
    }

    /// Stands in for reading a DLL: a file whose text is `pack` is a pack of [`GAME`]; anything
    /// else is unreadable.
    fn fake(dll: &Path) -> Result<Pack, String> {
        match std::fs::read_to_string(dll).map_err(|e| e.to_string())?.as_str() {
            "pack" => Ok(Pack { dll: dll.to_owned(), contract: CONTRACT, about: About { author: String::new(), source: "https://x".into() }, manifests: vec![GAME] }),
            _ => Err(format!("{} is not a game pack", dll.display())),
        }
    }

    #[test]
    fn local_builds_come_from_the_dev_list_and_packs_from_the_games_folder() {
        let dir = std::env::temp_dir().join(format!("monaka-catalogue-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let list = dir.join(r"local\dev-games.txt");
        let paths = Paths { binaries: dir.join("bin"), runs: dir.join("runs"), licences: dir.join("licenses"), packs: dir.join("games"), dev_games: Some(list.clone()) };
        std::fs::create_dir_all(&paths.binaries).unwrap();
        std::fs::create_dir_all(list.parent().unwrap()).unwrap();
        let scan_none = |_: &Path| packs::Scan::default();
        // No list, no games.
        assert!(catalogue_with(&paths, fake, scan_none).games.is_empty());
        std::fs::write(paths.binaries.join("monaka_t.dll"), "pack").unwrap();
        std::fs::write(paths.binaries.join("junk.dll"), "junk").unwrap();
        std::fs::write(&list, "# my builds\nmonaka_t.dll\n\njunk.dll\n").unwrap();
        let catalogue = catalogue_with(&paths, fake, scan_none);
        assert_eq!(catalogue.games.iter().map(|g| g.id).collect::<Vec<_>>(), ["t"]);
        assert_eq!(catalogue.problems.len(), 1, "{:?}", catalogue.problems);
        assert!(catalogue.problems[0].contains("junk.dll is not a game pack"));
        let t = &catalogue.games[0];
        assert!(!t.removable() && t.dll(&paths) == paths.binaries.join("monaka_t.dll") && t.licence_folder(&paths) == paths.licences);
        assert!(t.credit().starts_with("Built on this PC"), "{}", t.credit());
        // The same game from a pack is left out while the local build offers it.
        let pack_dir = paths.packs.join("monaka_t");
        std::fs::create_dir_all(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("monaka_t.dll"), "pack").unwrap();
        let scan_one = |_: &Path| packs::Scan { packs: vec![fake(&pack_dir.join("monaka_t.dll")).unwrap()], problems: Vec::new() };
        let both = catalogue_with(&paths, fake, scan_one);
        assert_eq!(both.games.len(), 1);
        assert!(both.problems.iter().any(|p| p.contains("already offered by a local build")), "{:?}", both.problems);
        // Without the local build, the pack's game is offered, and can be removed.
        std::fs::write(&list, "").unwrap();
        let packed = catalogue_with(&paths, fake, scan_one);
        assert!(packed.games.len() == 1 && packed.games[0].removable() && packed.games[0].licence_folder(&paths) == pack_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
