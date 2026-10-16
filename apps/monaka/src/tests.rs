//! The window driven as a player would, through its accessibility tree: every test has a folder
//! of its own for the settings, the games folder and the packs, and a stand-in PC
//! ([`FakePc`]), so nothing touches the player's files, Steam or the processes running.

use super::*;
use eframe::egui;
use egui_kittest::Harness;
use pc::Running;
use egui_kittest::kittest::{NodeT, Queryable};
use monaka_contract::manifest::{Kind, Manifest, opt};
use monaka_contract::pack::{self, CONTRACT, Contract};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// The game the test packs carry: started with the window (so no game process is waited for),
/// with one switch on its main tab.
const KIT: Manifest = Manifest {
    id: "kit",
    name: "Test Kit",
    executables: &["kit.exe"],
    producer: "monaka_kit",
    log: "kit.log",
    options_file: "kit-options.txt",
    run_prefix: "kit",
    before: "nothing to do.",
    launch: Launch::AtStart,
    options: &[opt("kit_switch", "Kit switch", "Turns the kit on.", Kind::Switch { default: false })],
    ..Manifest::GAME
};

/// What the stand-in PC answers, changed by the test as it goes.
#[derive(Default)]
struct PcState {
    steamvr: Cell<bool>,
    viewer: Cell<bool>,
    game: Cell<bool>,
    /// The file the player picks in the open dialog (none: they cancel).
    pick: RefCell<Option<PathBuf>>,
    opened: RefCell<Vec<String>>,
}

struct FakePc(Rc<PcState>);

impl Pc for FakePc {
    fn running(&self, _: &[&str], _: Option<u32>) -> Running {
        Running { steamvr: self.0.steamvr.get(), viewer: self.0.viewer.get(), game: self.0.game.get() }
    }
    fn processes(&self, executables: &[&str]) -> Vec<String> {
        if self.0.game.get() { executables.iter().map(|e| (*e).to_owned()).collect() } else { Vec::new() }
    }
    fn steam_libraries(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn steam_installed(&self, _: &[PathBuf], _: u32) -> bool {
        true
    }
    fn pick_pack_file(&self) -> Option<PathBuf> {
        self.0.pick.borrow().clone()
    }
    fn launch_steam_game(&self, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn open(&self, target: &str) -> Result<(), String> {
        self.0.opened.borrow_mut().push(target.to_owned());
        Ok(())
    }
}

/// A test's own folder, emptied.
fn folder(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("monaka-window-test-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn paths_in(dir: &Path) -> Paths {
    Paths { binaries: dir.join("bin"), runs: dir.join("runs"), licences: dir.join("licenses"), packs: dir.join("games"), dev_games: None }
}

/// One window at a time: the games on offer are the process's (as in the launcher, which has one
/// window), so tests in parallel would see each other's packs.
static ONE_WINDOW: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_window() -> std::sync::MutexGuard<'static, ()> {
    ONE_WINDOW.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The window over `dir`'s settings and games, asking `pc`.
fn window(dir: &Path, pc: &Rc<PcState>) -> Harness<'static, App> {
    let (paths, settings, pc) = (paths_in(dir), Settings::load_from(dir.join("settings.txt")), FakePc(pc.clone()));
    let mut harness = Harness::builder().with_size(egui::vec2(1100.0, 760.0)).build_eframe(move |cc| App::with(&cc.egui_ctx, None, paths, settings, Box::new(pc), None, false));
    harness.run();
    harness
}

fn disabled(harness: &Harness<'_, App>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

/// Whether the window shows `text` (a heading's text can be in the tree twice).
fn shows(harness: &Harness<'_, App>, text: &str) -> bool {
    harness.query_all_by_label(text).next().is_some()
}

/// A 64-bit DLL whose one section holds `exports`, each a named piece of data: what reads a pack
/// from its file needs (no code; it is never loaded).
fn dll_exporting(exports: &[(&str, Vec<u8>)]) -> Vec<u8> {
    const SECTION: u32 = 0x1000;
    let put32 = |bytes: &mut [u8], at: usize, value: u32| bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    let put16 = |bytes: &mut [u8], at: usize, value: u16| bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    let count = exports.len();
    // The export directory, its address, name and ordinal tables, then the names.
    let (functions, names, ordinals) = (40, 40 + 4 * count, 40 + 8 * count);
    let mut data = vec![0u8; ordinals + 2 * count];
    let mut name_rvas = Vec::new();
    for (name, _) in exports {
        name_rvas.push(SECTION + data.len() as u32);
        data.extend_from_slice(name.as_bytes());
        data.push(0);
    }
    let directory_size = data.len() as u32;
    // The data after the directory, so no export reads as forwarded.
    let mut data_rvas = Vec::new();
    for (_, bytes) in exports {
        data.resize(data.len().next_multiple_of(8), 0);
        data_rvas.push(SECTION + data.len() as u32);
        data.extend_from_slice(bytes);
    }
    put32(&mut data, 20, count as u32);
    put32(&mut data, 24, count as u32);
    put32(&mut data, 28, SECTION + functions as u32);
    put32(&mut data, 32, SECTION + names as u32);
    put32(&mut data, 36, SECTION + ordinals as u32);
    for i in 0..count {
        put32(&mut data, functions + 4 * i, data_rvas[i]);
        put32(&mut data, names + 4 * i, name_rvas[i]);
        put16(&mut data, ordinals + 2 * i, i as u16);
    }
    let virtual_size = data.len() as u32;
    data.resize(data.len().next_multiple_of(0x200), 0);
    let mut file = vec![0u8; SECTION as usize];
    file[0..2].copy_from_slice(b"MZ");
    put32(&mut file, 0x3c, 0x40);
    file[0x40..0x44].copy_from_slice(b"PE\0\0");
    put16(&mut file, 0x44, 0x8664); // x64
    put16(&mut file, 0x46, 1); // one section
    put16(&mut file, 0x54, 240); // the PE32+ optional header with 16 directories
    put16(&mut file, 0x56, 0x2022); // a DLL
    let optional = 0x58;
    put16(&mut file, optional, 0x20b);
    put32(&mut file, optional + 56, SECTION + virtual_size.next_multiple_of(0x1000));
    put32(&mut file, optional + 108, 16);
    put32(&mut file, optional + 112, SECTION);
    put32(&mut file, optional + 116, directory_size);
    let section = optional + 240;
    file[section..section + 6].copy_from_slice(b".rdata");
    put32(&mut file, section + 8, virtual_size);
    put32(&mut file, section + 12, SECTION);
    put32(&mut file, section + 16, data.len() as u32);
    put32(&mut file, section + 20, SECTION);
    put32(&mut file, section + 36, 0x4000_0040);
    file.extend_from_slice(&data);
    file
}

/// Writes a pack of [`KIT`] as `file` (`monaka_kit.dll`, as its producer is named).
fn write_kit(file: &Path) {
    let mut text = pack::text(&[KIT], "Tester", "https://example.invalid/kit", "1.0.0", &pack::Needs::NONE).into_bytes();
    text.push(0);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, dll_exporting(&[("MONAKA_CONTRACT", CONTRACT.packed().to_le_bytes().to_vec()), ("MONAKA_MANIFEST", text)])).unwrap();
}

/// [`KIT`] installed in `dir`'s games folder, as an added pack is.
fn install_kit(dir: &Path) {
    write_kit(&paths_in(dir).packs.join("monaka_kit").join("monaka_kit.dll"));
}

#[test]
fn the_test_pack_reads_as_a_pack() {
    let dir = folder("reads");
    let dll = dir.join("monaka_kit.dll");
    write_kit(&dll);
    let read = packs::read(&dll).unwrap();
    assert_eq!(read.manifests.iter().map(|m| m.id).collect::<Vec<_>>(), ["kit"]);
}

#[test]
fn with_no_game_only_a_pack_can_be_added() {
    let _one = one_window();
    let dir = folder("empty");
    let pc = Rc::new(PcState::default());
    pc.steamvr.set(true);
    let harness = window(&dir, &pc);
    assert!(shows(&harness, "No games yet"));
    assert!(!shows(&harness, "Play"), "nothing to play");
    assert!(!shows(&harness, "Remove pack"), "nothing to remove");
    assert!(!disabled(&harness, "Add game pack..."));
}

#[test]
fn a_pack_is_added_after_its_warning_and_removed_after_asking() {
    let _one = one_window();
    let dir = folder("add");
    let file = dir.join("download").join("monaka_kit.dll");
    write_kit(&file);
    let pc = Rc::new(PcState::default());
    let mut harness = window(&dir, &pc);

    // Cancelled in the open dialog: nothing asked.
    harness.get_by_label("Add game pack...").click();
    harness.run();
    assert!(harness.state().dialog.is_none());

    // Picked, then cancelled at the warning: not added.
    *pc.pick.borrow_mut() = Some(file.clone());
    harness.get_by_label("Add game pack...").click();
    harness.run();
    assert!(harness.state().dialog.is_some(), "no warning asked: {}", harness.state().state_line);
    harness.get_by_label_contains("Only add packs from people you trust");
    harness.get_by_label("Cancel").click();
    harness.run();
    assert_eq!(harness.state().state_line, "Game pack not added.");
    assert!(shows(&harness, "No games yet"));

    // Added.
    harness.get_by_label("Add game pack...").click();
    harness.run();
    harness.get_by_label("Add").click();
    harness.run();
    assert!(harness.state().state_line.starts_with("Added Test Kit"), "{}", harness.state().state_line);
    assert!(shows(&harness, "Test Kit"));
    assert!(paths_in(&dir).packs.join("monaka_kit").join("monaka_kit.dll").is_file());

    // Removed, after asking.
    harness.get_by_label("Remove pack").click();
    harness.run();
    harness.get_by_label_contains("Its folder is deleted");
    harness.get_by_label("Remove").click();
    harness.run();
    assert!(harness.state().state_line.starts_with("Removed"), "{}", harness.state().state_line);
    assert!(shows(&harness, "No games yet"));
    assert!(!paths_in(&dir).packs.join("monaka_kit").exists());
}

#[test]
fn play_waits_for_steamvr_and_for_no_other_session() {
    let _one = one_window();
    let dir = folder("start");
    install_kit(&dir);
    let pc = Rc::new(PcState::default());
    let mut harness = window(&dir, &pc);
    assert!(disabled(&harness, "Play"), "SteamVR is not running");
    pc.steamvr.set(true);
    harness.state_mut().refresh_checks();
    harness.run();
    assert!(!disabled(&harness, "Play"), "a game started with the window needs only SteamVR");
    pc.viewer.set(true);
    harness.state_mut().refresh_checks();
    harness.run();
    assert!(disabled(&harness, "Play"), "another session's viewer runs");
}

#[test]
fn an_option_changed_in_the_window_is_saved() {
    let _one = one_window();
    let dir = folder("option");
    install_kit(&dir);
    let pc = Rc::new(PcState::default());
    let mut harness = window(&dir, &pc);
    harness.get_by_label("Kit switch");
    harness.get_by_label("On").click();
    harness.run();
    let saved = Settings::load_from(dir.join("settings.txt"));
    assert!(saved.values.get("kit").is_some_and(|v| v.switch(&KIT, "kit_switch")), "{:?}", std::fs::read_to_string(dir.join("settings.txt")));
}

#[test]
fn changes_soon_after_a_save_are_saved_once_the_gap_has_passed() {
    let _one = one_window();
    let dir = folder("gap");
    install_kit(&dir);
    let pc = Rc::new(PcState::default());
    let mut harness = window(&dir, &pc);
    let saved = || Settings::load_from(dir.join("settings.txt")).values.get("kit").is_some_and(|v| v.switch(&KIT, "kit_switch"));
    harness.state_mut().set_from_headset("kit_switch", "0".to_owned());
    harness.state_mut().set_from_headset("kit_switch", "1".to_owned());
    assert!(!saved(), "the second change waits");
    harness.state_mut().save_pending(true);
    assert!(saved(), "and is saved");
}

#[test]
fn a_report_is_saved_from_the_window_and_its_folder_opened() {
    let _one = one_window();
    let dir = folder("report");
    install_kit(&dir);
    let pc = Rc::new(PcState::default());
    let mut harness = window(&dir, &pc);
    harness.get_by_label("Save report").click();
    harness.run();
    let reports = paths_in(&dir).runs.join("reports");
    let zips: Vec<_> = std::fs::read_dir(&reports).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "zip")).collect();
    assert_eq!(zips.len(), 1, "{}", harness.state().state_line);
    assert!(pc.opened.borrow().iter().any(|o| o.ends_with("reports")), "{:?}", pc.opened.borrow());
}

#[test]
fn a_setting_from_the_headset_is_saved_where_its_control_would_save_it() {
    let _one = one_window();
    let dir = folder("headset");
    install_kit(&dir);
    let pc = Rc::new(PcState::default());
    let mut harness = window(&dir, &pc);
    harness.state_mut().set_from_headset("kit_switch", "1".to_owned());
    harness.state_mut().set_from_headset(DIAGNOSTIC_LOGGING.id, "1".to_owned());
    // The second came soon after the first: saved once the gap has passed (or the window closes).
    harness.state_mut().save_pending(true);
    let saved = Settings::load_from(dir.join("settings.txt"));
    assert!(saved.values.get("kit").is_some_and(|v| v.switch(&KIT, "kit_switch")), "the game's own option");
    assert!(saved.monaka.switch(&MONAKA, DIAGNOSTIC_LOGGING.id), "Monaka's own, for every game");
}

#[test]
fn a_pack_that_cannot_run_is_listed_with_why_and_can_be_removed() {
    let _one = one_window();
    let dir = folder("left-out");
    let old = paths_in(&dir).packs.join("old_kit").join("monaka_kit.dll");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    let contract = Contract { major: CONTRACT.major - 1 }.packed().to_le_bytes().to_vec();
    std::fs::write(&old, dll_exporting(&[("MONAKA_CONTRACT", contract)])).unwrap();
    let pc = Rc::new(PcState::default());
    let mut harness = window(&dir, &pc);
    harness.get_by_label("old_kit").click();
    harness.run();
    assert!(shows(&harness, "Made for an older Monaka. Get a newer version of this game pack."));
    harness.get_by_label("Remove pack").click();
    harness.run();
    harness.get_by_label("Remove").click();
    harness.run();
    assert!(!old.parent().unwrap().exists());
    assert!(!shows(&harness, "old_kit"));
}
