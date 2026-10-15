use super::*;

#[test]
fn reads_a_command_line() {
    let own = command_line(std::process::id()).expect("our own command line");
    let exe = std::env::current_exe().unwrap();
    let stem = exe.file_stem().unwrap().to_string_lossy().to_ascii_lowercase();
    assert!(own.to_ascii_lowercase().contains(&stem), "{own}");
    assert_eq!(command_line(u32::MAX - 3), None, "no such process");
}

#[test]
fn finds_documents() {
    assert!(documents().is_some_and(|d| d.is_absolute()));
}

#[test]
fn reads_the_windows_build() {
    let build = windows_build();
    let number = build.split("build ").nth(1).and_then(|b| b.split('.').next()).and_then(|b| b.parse::<u32>().ok());
    assert!(number.is_some_and(|n| n >= 10_240), "{build}");
}
