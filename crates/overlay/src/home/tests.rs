use super::*;
use monaka_contract::manifest::{CONTROLLER_SMOOTHING, HUD_SIZE, Status, choice, line, opt};
use monaka_contract::pack::Needs;
use monaka_core::home::{GAMES_FILE, MANIFEST_FILE, VALUES_FILE, games_text, values_text};

const TRIAL: Opt = opt("trial", "Trial", "For testers.", Kind::Switch { default: false }).dev();
const GAME: Manifest = Manifest {
    id: "g",
    name: "The Game",
    status: Status::Beta,
    executables: &["g.exe"],
    producer: "monaka_g",
    log: "monaka_g.log",
    options_file: "g-options.txt",
    run_prefix: "g",
    options: &[
        HUD_SIZE,
        opt("mode", "Mode", "", choice(&[("one", "One"), ("two", "Two")], "one")).on(Shown::Rendering),
        opt("only", "Only", "", choice(&[("one", "One")], "one")),
        line("extra_line", "Line", "", Kind::Text),
        TRIAL,
    ],
    ..Manifest::GAME
};

/// A run folder as the launcher leaves it, with `saved` values.
pub(crate) fn run_folder(name: &str, saved: &Values) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("monaka-home-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(MANIFEST_FILE), pack::text(std::slice::from_ref(&GAME), "Someone", "", "1.2.3", &Needs::NONE)).unwrap();
    std::fs::write(folder.join(VALUES_FILE), values_text(&[&GAME, &MONAKA], saved)).unwrap();
    let games = [
        Listed { id: "g".into(), name: "The Game".into(), status: "Beta".into(), current: true, playing: true },
        Listed { id: "o".into(), name: "Other".into(), status: "Alpha".into(), current: false, playing: false },
    ];
    std::fs::write(folder.join(GAMES_FILE), games_text(&games)).unwrap();
    folder
}

fn ids(home: &Home) -> Vec<&'static str> {
    home.settings().iter().map(|s| s.opt.id).collect()
}

#[test]
fn the_launchers_files_make_the_home() {
    let folder = run_folder("load", &Values::default());
    let home = Home::load(Some(&folder));
    assert_eq!(home.game.map(|g| g.name), Some("The Game"));
    assert_eq!((home.about.author.as_str(), home.about.version.as_str()), ("Someone", "1.2.3"));
    assert_eq!(home.games.len(), 2);
    assert!(home.games[0].current && !home.games[1].current);
    assert_eq!(PAGES[home.front], Page::Settings, "Settings in front, Games left, Log right");
    // The game's settings, then Monaka's own; not a command-line one, not a choice of one, not a
    // tester's outside developer mode, not the debug menu's switch while it is off.
    let shown = ids(&home);
    assert!(shown.starts_with(&["hud_scale", "mode"]), "{shown:?}");
    assert!(shown.contains(&CONTROLLER_SMOOTHING.id) && shown.contains(&DEV_MODE.id));
    for hidden in ["extra_line", "only", "trial", DEBUG_MENU.id] {
        assert!(!shown.contains(&hidden), "{hidden} shown: {shown:?}");
    }
    assert_eq!(home.tabs(), [Shown::Hud, Shown::Rendering, Shown::Monaka]);
    assert_eq!(home.tab, Shown::Hud);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn developer_mode_and_the_debug_menu_bring_theirs_in() {
    let mut saved = Values::default();
    saved.set(DEV_MODE.id, "1");
    saved.set(DEBUG_MENU.id, "1");
    let folder = run_folder("dev", &saved);
    let shown = ids(&Home::load(Some(&folder)));
    assert!(shown.contains(&"trial") && shown.contains(&DEBUG_MENU.id), "{shown:?}");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_change_is_kept_and_told_once() {
    let folder = run_folder("change", &Values::default());
    let mut home = Home::load(Some(&folder));
    let hud = home.settings().into_iter().find(|s| s.opt.id == "hud_scale").unwrap();
    assert!(!hud.live());
    home.set(&hud, "0.7".into());
    home.set(&hud, "0.7".into());
    assert_eq!(home.value(&hud), "0.7");
    assert_eq!(home.take_changes(), [("hud_scale", "0.7".to_owned())]);
    assert!(home.take_changes().is_empty());
    let smoothing = home.settings().into_iter().find(|s| s.opt.id == CONTROLLER_SMOOTHING.id).unwrap();
    assert!(smoothing.live() && smoothing.editable());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_ring_turns_round() {
    let mut home = Home::load(None);
    home.turn(1);
    assert_eq!(PAGES[home.front], Page::Log);
    home.turn(1);
    assert_eq!(PAGES[home.front], Page::Games);
    home.turn(-1);
    assert_eq!(PAGES[home.front], Page::Log);
    assert!((0..3).all(|page| home.take_stale(page)), "every panel drawn again");
}

#[test]
fn without_a_run_folder_there_is_only_monakas_own() {
    let home = Home::load(None);
    assert!(home.game.is_none() && home.games.is_empty());
    assert!(home.settings().iter().all(|s| s.owner.id == MONAKA.id));
}

#[test]
fn the_log_tail_keeps_the_newest_lines() {
    let folder = std::env::temp_dir().join(format!("monaka-home-test-log-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
    std::fs::write(folder.join("monaka.log"), text).unwrap();
    let mut log = LogTail::new(Some(&folder), Some("game.log"));
    let now = Instant::now();
    assert!(log.refresh(now));
    assert_eq!(log.lines.len(), TAIL_LINES);
    assert_eq!(log.lines.last().map(String::as_str), Some("line 999"));
    assert!(!log.refresh(now), "read again only after a while");
    log.show(LogSource::Game);
    assert!(log.refresh(now));
    assert!(log.lines[0].contains("not there yet"), "{:?}", log.lines);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_change_that_waits_for_a_restart_is_applied_on_asking() {
    let folder = run_folder("apply", &Values::default());
    let mut home = Home::load(Some(&folder));
    let hud = home.settings().into_iter().find(|s| s.opt.id == "hud_scale").unwrap();
    let smoothing = home.settings().into_iter().find(|s| s.opt.id == CONTROLLER_SMOOTHING.id).unwrap();
    home.set(&hud, "0.7".into());
    home.set(&smoothing, "5".into());
    assert_eq!(home.pending, ["hud_scale"], "only what the game takes on a restart waits");
    home.ask_apply();
    assert!(home.take_apply() && home.applying && !home.take_apply());
    home.applied(Some("slow".into()));
    assert!(home.applying && home.pending.len() == 1, "still waiting");
    home.applied(None);
    assert!(!home.applying && home.pending.is_empty() && home.problem.is_none());
    // Without the launcher there is nothing to restart.
    let mut alone = Home::load(None);
    alone.ask_apply();
    assert!(!alone.take_apply());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_home_takes_what_the_window_changes() {
    let folder = run_folder("sync", &Values::default());
    let mut home = Home::load(Some(&folder));
    assert!(!home.sync(&folder), "nothing changed");
    assert!(home.can_apply(), "its game plays");
    // The window picked another game, with nothing playing: its settings, none to apply.
    const OTHER: Manifest = Manifest { id: "o", name: "Other", options: &[opt("shade", "Shade", "", Kind::Switch { default: false }).on(Shown::Rendering)], ..GAME };
    std::fs::write(folder.join(MANIFEST_FILE), pack::text(std::slice::from_ref(&OTHER), "", "", "", &Needs::NONE)).unwrap();
    let mut saved = Values::default();
    saved.set("shade", "1");
    std::fs::write(folder.join(VALUES_FILE), values_text(&[&OTHER, &MONAKA], &saved)).unwrap();
    let games = [
        Listed { id: "g".into(), name: "The Game".into(), status: "Beta".into(), ..Listed::default() },
        Listed { id: "o".into(), name: "Other".into(), status: "Alpha".into(), current: true, ..Listed::default() },
    ];
    std::fs::write(folder.join(GAMES_FILE), games_text(&games)).unwrap();
    home.tab = Shown::Hud;
    assert!(home.sync(&folder));
    assert_eq!(home.game.map(|g| g.id), Some("o"));
    assert!(home.values.switch(&OTHER, "shade"));
    assert_eq!(home.tab, Shown::Rendering, "a tab the new game lacks falls back to its first");
    assert!(home.playing().is_none() && !home.can_apply());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_home_with_no_files_yet_is_empty_until_they_come() {
    let folder = std::env::temp_dir().join(format!("monaka-home-test-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let mut home = Home::load(Some(&folder));
    assert!(home.game.is_none() && home.games.is_empty());
    let filled = run_folder("empty-filled", &Values::default());
    for name in FILES {
        std::fs::copy(filled.join(name), folder.join(name)).unwrap();
    }
    assert!(home.sync(&folder));
    assert_eq!(home.game.map(|g| g.name), Some("The Game"));
    let _ = std::fs::remove_dir_all(&folder);
    let _ = std::fs::remove_dir_all(&filled);
}

#[test]
fn with_no_game_playing_a_change_waits_for_nothing() {
    let folder = run_folder("idle", &Values::default());
    let games = [Listed { id: "g".into(), name: "The Game".into(), status: "Beta".into(), current: true, playing: false }];
    std::fs::write(folder.join(GAMES_FILE), games_text(&games)).unwrap();
    let mut home = Home::load(Some(&folder));
    let hud = home.settings().into_iter().find(|s| s.opt.id == HUD_SIZE.id).unwrap();
    home.set(&hud, "0.7".to_owned());
    assert!(home.pending.is_empty(), "it applies when the game starts");
    assert_eq!(home.take_changes(), [(HUD_SIZE.id, "0.7".to_owned())], "still saved by the window");
    let _ = std::fs::remove_dir_all(&folder);
}
