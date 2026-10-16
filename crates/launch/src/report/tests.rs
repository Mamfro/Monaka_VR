use super::*;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("monaka-report-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A run folder as a run leaves it: its logs, an options file, a marker, the DLL copy and a
/// capture.
fn run_folder(runs: &Path) -> PathBuf {
    let run = runs.join("wolf-20261015T2133414349371-8d3a5abc");
    std::fs::create_dir_all(run.join("captures")).unwrap();
    std::fs::write(run.join("monaka.log"), "state: Running\nrun folder: D:\\Home\\Bob\\runs\\wolf\n").unwrap();
    std::fs::write(run.join("monaka_wolf.log"), "view builds\n").unwrap();
    std::fs::write(run.join("wolf-options.txt"), "aim=hand_rig\n").unwrap();
    std::fs::write(run.join("diagnostic-logging.on"), "").unwrap();
    std::fs::write(run.join("monaka_wolf_tno-1.dll"), [0u8; 64]).unwrap();
    std::fs::write(run.join("captures").join("pair-1.png"), [0u8; 64]).unwrap();
    run
}

#[test]
fn a_report_holds_the_runs_text_scrubbed_and_nothing_else() {
    let dir = temp("gather");
    let run = run_folder(&dir.join("runs"));
    let settings = dir.join("settings.txt");
    std::fs::write(&settings, "monaka.dev_mode=1\nwolf_tno.fsr_dlls=D:\\Home\\Bob\\fsr\n").unwrap();
    let staging = dir.join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    let report = Report { run: Some(&run), settings: Some(&settings), details: "Starting...\nAttached from D:\\Home\\Bob\n" };
    gather(&report, &staging, &Scrub::for_profile(Some(r"D:\Home\Bob"))).unwrap();
    let read = |name: &str| std::fs::read_to_string(staging.join(name)).unwrap();
    assert!(read("about.txt").starts_with("Monaka "));
    assert!(read("details.txt").contains(r"Attached from %USERPROFILE%"), "{}", read("details.txt"));
    assert!(read("settings.txt").contains(r"%USERPROFILE%\fsr"));
    let name = run.file_name().unwrap().to_string_lossy().into_owned();
    let mut files: Vec<String> = std::fs::read_dir(staging.join(&name)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    assert_eq!(files, ["diagnostic-logging.on", "monaka.log", "monaka_wolf.log", "wolf-options.txt"], "no DLL, no captures");
    assert!(read(&format!("{name}/monaka.log")).contains(r"%USERPROFILE%\runs"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_report_is_one_zip_and_leaves_nothing_else() {
    let dir = temp("zip");
    let run = run_folder(&dir.join("runs"));
    let reports = dir.join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    let zip = save(&Report { run: Some(&run), settings: None, details: "" }, &reports).unwrap();
    assert!(zip.is_file() && std::fs::metadata(&zip).unwrap().len() > 100, "{}", zip.display());
    assert_eq!(std::fs::read(&zip).unwrap()[..2], *b"PK", "a zip");
    assert_eq!(std::fs::read_dir(&reports).unwrap().count(), 1, "the staging folder gone");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_latest_run_is_the_newest_of_that_game() {
    let runs = temp("latest");
    for name in ["wolf-20261015T1", "wolf-20261015T3", "wolf_tob-20261016T1", "beast-20261017T1"] {
        std::fs::create_dir_all(runs.join(name)).unwrap();
    }
    assert_eq!(latest_run(&runs, "wolf"), Some(runs.join("wolf-20261015T3")));
    assert_eq!(latest_run(&runs, "dl2"), None);
    let _ = std::fs::remove_dir_all(&runs);
}
