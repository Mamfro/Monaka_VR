use super::prune_runs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

fn folder(runs: &Path, name: &str) {
    std::fs::create_dir_all(runs.join(name)).unwrap();
    std::fs::write(runs.join(name).join("monaka_t.dll"), b"copy").unwrap();
}

fn names(runs: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(runs).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    names
}

#[test]
fn only_the_newest_runs_of_that_game_are_kept() {
    let runs = std::env::temp_dir().join(format!("monaka-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&runs);
    for name in ["t-20261001T0000000000000-aaaaaaaa", "t-20261003T0000000000000-cccccccc", "t-20261002T0000000000000-bbbbbbbb", "t-check-20261001T0000000000000-dddddddd", "tt-20261001T0000000000000-eeeeeeee", "u-20261001T0000000000000-ffffffff"] {
        folder(&runs, name);
    }
    prune_runs(&runs, "t", 2);
    assert_eq!(
        names(&runs),
        ["t-20261002T0000000000000-bbbbbbbb", "t-20261003T0000000000000-cccccccc", "t-check-20261001T0000000000000-dddddddd", "tt-20261001T0000000000000-eeeeeeee", "u-20261001T0000000000000-ffffffff"],
        "the oldest of t's runs goes; check runs and other games stay"
    );
    // A folder whose DLL is in use (here: open unshared, as a loaded DLL is) stays whole.
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(runs.join("t-20261002T0000000000000-bbbbbbbb").join("monaka_t.dll")).unwrap();
    prune_runs(&runs, "t", 1);
    drop(held);
    assert!(runs.join("t-20261002T0000000000000-bbbbbbbb").join("monaka_t.dll").exists());
    let _ = std::fs::remove_dir_all(&runs);
}
