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
