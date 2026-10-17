use super::*;

#[test]
fn a_viewer_setting_line_is_a_setting() {
    assert!(matches!(viewer_line(format!("{SETTING_LINE} menu_reach_depth=20")), Event::Setting(id, value) if id == "menu_reach_depth" && value == "20"));
    assert!(matches!(viewer_line("GPU: something".into()), Event::Detail(_)));
}

#[test]
fn dates_are_civil() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(20_367), (2025, 10, 6));
    assert_eq!(civil_from_days(11_016), (2000, 2, 29));
}

#[test]
fn versions_sort_and_differ() {
    let (a, b) = (version(), version());
    assert_eq!(a.len(), "20261006T1122330000000-1a2b3c4d".len());
    assert_ne!(a, b);
}

#[test]
fn the_viewers_stated_failure_is_the_runs() {
    let said = format!("{FAILURE_LINE} the game's head pose block is version 4, this Monaka reads version 3");
    assert_eq!(failure(&said).as_deref(), Some("the game's head pose block is version 4, this Monaka reads version 3"));
    assert_eq!(failure("Live stereo; update cadence set by the producer."), None);
}

#[test]
fn a_refusal_tells_the_producers_reason() {
    let log = "    0.001 monaka_wolf_tnc 0.1.0\n    0.363 rejected: the game was started before Monaka; start it again while Monaka runs\n";
    assert_eq!(refusal_from(log, code::NOT_READY), "The game was started before Monaka; start it again while Monaka runs.");
    let host = "    0.400 |     0.363 rejected: cvar vr_enable is not where this build keeps it";
    assert!(refusal_from(host, code::REVISION_MISMATCH).contains("(cvar vr_enable is not where this build keeps it)"));
    assert_eq!(refusal_from("    0.001 started", 50), "The mod refused to start in this game (code 50); see the log.");
}
