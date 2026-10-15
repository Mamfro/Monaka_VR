use super::*;
use monaka_contract::manifest::{Launch, Status};

#[test]
fn a_dll_without_the_exports_is_not_a_pack() {
    let kernel = std::fs::read(r"C:\Windows\System32\kernel32.dll").unwrap();
    assert!(from_bytes(Path::new("kernel32.dll"), &kernel).unwrap().is_none());
    assert!(read(Path::new(r"C:\Windows\System32\kernel32.dll")).unwrap_err().contains("not a game pack"));
    assert!(from_bytes(Path::new("x.dll"), b"not a PE file").is_err());
}

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
    options: &[],
    licences: &[],
    left_hand_aims: false,
    weapon_ring: false,
    controls: &[],
};

/// Stands in for reading a DLL: a file whose text is `pack <major>`, then optionally `requires`
/// and host feature names, is a pack of [`GAME`] with that contract; anything else is no pack.
fn fake(dll: &Path) -> Result<Option<Pack>, String> {
    let text = std::fs::read_to_string(dll).map_err(|e| e.to_string())?;
    let Some(rest) = text.strip_prefix("pack ") else { return Ok(None) };
    let mut words = rest.split_whitespace();
    let contract = Contract { major: words.next().unwrap().parse().unwrap() };
    let requires = words.skip_while(|w| *w != "requires").skip(1).map(str::to_owned).collect();
    let about = About { author: "A:B".into(), source: "https://x".into(), ..About::default() };
    Ok(Some(Pack { dll: dll.to_owned(), contract, about, requires, uses: Vec::new(), manifests: vec![GAME] }))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("monaka-packs-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn scanning_lists_packs_and_says_what_is_wrong_with_the_rest() {
    let games = temp("scan");
    let current = format!("pack {}", pack::CONTRACT);
    write(&games.join(r"monaka_t\monaka_t.dll"), &current);
    write(&games.join(r"monaka_t\helper.dll"), "not a pack");
    write(&games.join(r"monaka_t\T-LICENSE.txt"), "licence");
    write(&games.join(r"old\monaka_t.dll"), "pack 3");
    write(&games.join(r"renamed\monaka_x.dll"), &current);
    write(&games.join(r"empty\readme.txt"), "");
    write(&games.join(r".adding-1-2\monaka_t.dll"), &current);
    let scan = scan_with(&games, fake);
    assert_eq!(scan.packs.len(), 1);
    assert!(scan.packs[0].dll.ends_with(r"monaka_t\monaka_t.dll"));
    assert_eq!(scan.packs[0].folder(), games.join("monaka_t"));
    assert_eq!(scan.problems.len(), 3, "{:?}", scan.problems);
    assert!(scan.problems[0].starts_with("Game pack empty: no game pack DLL"), "{:?}", scan.problems);
    assert!(scan.problems[1].starts_with("Game pack old (Test)") && scan.problems[1].contains("older Monaka"), "{:?}", scan.problems);
    assert!(scan.problems[2].contains("names its producer monaka_t, but the DLL is monaka_x.dll"), "{:?}", scan.problems);
    // The window lists each folder left out once, by its games where they can be read.
    let left: Vec<(&str, &str)> = scan.left_out.iter().map(|l| (l.name.as_str(), l.why.as_str())).collect();
    assert_eq!(left.len(), 3, "{left:?}");
    assert_eq!(left[0], ("empty", "There is no game pack in this folder."));
    assert_eq!(left[1], ("Test", "Made for an older Monaka. Get a newer version of this game pack."));
    assert!(left[2].0 == "Test" && left[2].1.contains("names its producer"), "{left:?}");
    assert_eq!(scan.left_out[1].folder, games.join("old"));
    let _ = std::fs::remove_dir_all(&games);
}

#[test]
fn a_folder_of_several_packs_that_cannot_run_is_one_entry() {
    let games = temp("several");
    write(&games.join(r"games-vr\monaka_a.dll"), "pack 3");
    write(&games.join(r"games-vr\monaka_b.dll"), "pack 3");
    let scan = scan_with(&games, fake);
    assert_eq!(scan.left_out.len(), 1, "{:?}", scan.left_out);
    assert_eq!(scan.left_out[0].name, "Test, Test");
    let _ = std::fs::remove_dir_all(&games);
}

#[test]
fn a_pack_for_a_newer_monaka_asks_for_an_update() {
    let games = temp("newer");
    let newer = format!("pack {}", pack::CONTRACT.major + 1);
    write(&games.join(r"newer\monaka_t.dll"), &newer);
    write(&games.join(r"old\monaka_t.dll"), "pack 3");
    let scan = scan_with(&games, fake);
    assert!(scan.packs.is_empty());
    assert_eq!(scan.too_new, ["Test (game pack newer)"], "only the newer one asks for an update");
    assert_eq!(scan.problems.len(), 2, "{:?}", scan.problems);
    // Offered for adding, it is staged so the player can be asked, and never installed.
    let source = temp("newer-source").join("monaka_t.dll");
    write(&source, &newer);
    let staged = stage_with(&source, &games, fake).unwrap();
    assert_eq!(staged.too_new().as_deref(), Some("Test"));
    assert!(staged.install().unwrap_err().contains("update Monaka"));
    assert!(!games.join("monaka_t").exists());
    let _ = std::fs::remove_dir_all(&games);
    let _ = std::fs::remove_dir_all(source.parent().unwrap());
}

#[test]
fn a_pack_requiring_a_feature_this_monaka_lacks_asks_for_an_update() {
    let games = temp("requires");
    write(&games.join(r"needy\monaka_t.dll"), &format!("pack {} requires depth teleport", pack::CONTRACT));
    write(&games.join(r"fine\monaka_t.dll"), &format!("pack {} requires depth", pack::CONTRACT));
    let scan = scan_with(&games, fake);
    assert_eq!(scan.packs.len(), 1, "{:?}", scan.problems);
    assert!(scan.packs[0].dll.ends_with(r"fine\monaka_t.dll"));
    assert_eq!(scan.too_new, ["Test (game pack needy)"]);
    assert!(scan.problems.iter().any(|p| p.contains("one with teleport: update Monaka")), "{:?}", scan.problems);
    let _ = std::fs::remove_dir_all(&games);
}

#[test]
fn a_dll_is_staged_shown_and_installed_or_discarded() {
    let games = temp("dll");
    let source = temp("dll-source").join("monaka_t.dll");
    write(&source, &format!("pack {}", pack::CONTRACT));
    let staged = stage_with(&source, &games, fake).unwrap();
    assert_eq!(staged.name(), "monaka_t");
    assert_eq!(staged.names(), "Test");
    assert_eq!(staged.credit(), "by A, B, from https://x");
    assert!(staged.replaces().is_none());
    // Discarded: nothing is left behind.
    drop(staged);
    assert_eq!(std::fs::read_dir(&games).unwrap().count(), 0);
    // Installed, then again as an update of itself.
    let folder = stage_with(&source, &games, fake).unwrap().install().unwrap();
    assert_eq!(folder, games.join("monaka_t"));
    assert!(folder.join("monaka_t.dll").is_file());
    let again = stage_with(&source, &games, fake).unwrap();
    assert_eq!(again.replaces(), Some(folder.clone()));
    again.install().unwrap();
    assert_eq!(std::fs::read_dir(&games).unwrap().count(), 1, "the staging folder is gone");
    assert_eq!(scan_with(&games, fake).packs.len(), 1);
    // Removed, only from inside the games folder.
    assert!(remove(&source, &games).is_err());
    remove(&folder, &games).unwrap();
    assert!(!folder.exists());
    let _ = std::fs::remove_dir_all(&games);
    let _ = std::fs::remove_dir_all(source.parent().unwrap());
}

#[test]
fn what_cannot_be_used_is_refused_and_cleaned_up() {
    let games = temp("refuse");
    let source = temp("refuse-source");
    let old = source.join("monaka_t.dll");
    write(&old, "pack 0");
    assert!(stage_with(&old, &games, fake).unwrap_err().contains("older Monaka"));
    let none = source.join("x.dll");
    write(&none, "nothing");
    assert!(stage_with(&none, &games, fake).unwrap_err().contains("holds no game pack"));
    let text = source.join("notes.txt");
    write(&text, "pack 1");
    assert!(stage_with(&text, &games, fake).unwrap_err().contains("expected a .zip or a .dll"));
    assert_eq!(std::fs::read_dir(&games).unwrap().count(), 0, "no staging folder left behind");
    let _ = std::fs::remove_dir_all(&games);
    let _ = std::fs::remove_dir_all(&source);
}

#[test]
fn a_zip_with_a_top_folder_brings_its_licences() {
    let games = temp("zip");
    let source = temp("zip-source");
    write(&source.join(r"Test VR\monaka_t.dll"), &format!("pack {}", pack::CONTRACT));
    write(&source.join(r"Test VR\T-LICENSE.txt"), "licence");
    let zip = source.join("test-vr.zip");
    // Windows' bsdtar writes a zip for a .zip name (`-a`).
    let made = Command::new(r"C:\Windows\System32\tar.exe").arg("-a").arg("-cf").arg(&zip).arg("-C").arg(&source).arg("Test VR").status().unwrap();
    assert!(made.success());
    let folder = stage_with(&zip, &games, fake).unwrap().install().unwrap();
    assert_eq!(folder, games.join("Test VR"), "named after the zip's own folder");
    assert!(folder.join("T-LICENSE.txt").is_file());
    assert_eq!(std::fs::read_dir(&games).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&games);
    let _ = std::fs::remove_dir_all(&source);
}
