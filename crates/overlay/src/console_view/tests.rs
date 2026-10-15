use super::*;

#[test]
fn the_stick_picks_a_button_round_its_row_and_a_runs_it() {
    let mut nav = ConsoleNav::default();
    assert_eq!(nav.press(Press::Switch).as_deref(), Some("list"), "the first probe button to start with");
    nav.press(Press::Left);
    assert_eq!(nav.picked().1, "help", "left from the first goes round to the row's last");
    nav.press(Press::Down);
    assert_eq!(nav.picked().1, "cover undo", "down keeps the place in the row");
    nav.press(Press::Down);
    assert_eq!(nav.row, 0, "down from the last row goes round to the first");
    nav.press(Press::Up);
    for _ in 0..3 {
        nav.press(Press::Right);
    }
    assert_eq!(nav.press(Press::Switch).as_deref(), Some("cover start"), "the search's row goes round too");
}

#[test]
fn the_headset_buttons_start_with_the_windows() {
    for (headset, window) in HEADSET_PROBES.iter().zip(PROBE_BUTTONS) {
        assert_eq!((headset.0, headset.1), window);
    }
}

#[test]
fn the_log_scrolls_up_from_the_newest_and_stops_at_the_oldest() {
    let log: String = (0..100).map(|n| format!("line {n}\n")).collect();
    assert_eq!(headset_window(&log, 0, 40), (60, 100, 100), "the newest at the bottom");
    assert_eq!(headset_window(&log, 10, 40), (50, 90, 100));
    assert_eq!(headset_window(&log, 500, 40), (0, 40, 100), "no further than the oldest");
    assert_eq!(headset_window("a\nb\n", 5, 40), (0, 2, 2), "a short log, all of it");
    let mut nav = ConsoleNav::default();
    nav.scroll(-3);
    assert_eq!(nav.scroll, 0, "not below the newest");
    nav.scroll(5);
    assert_eq!(nav.scroll, 5);
}
