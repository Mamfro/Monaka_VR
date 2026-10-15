use super::*;

#[test]
fn a_report_holds_for_its_window_then_is_not_known() {
    let mut s = Situations::new();
    assert_eq!(s.state(Situation::CameraTaken, 0), None, "never reported");
    assert_eq!(s.report(Situation::CameraTaken, true, 100, 50), Some(true));
    assert_eq!(s.report(Situation::CameraTaken, true, 120, 50), None, "renewed, no change");
    assert!(s.active(Situation::CameraTaken, 170));
    assert_eq!(s.state(Situation::CameraTaken, 171), None, "no longer renewed");
    assert_eq!(s.report(Situation::CameraTaken, true, 200, 50), Some(true), "back after expiring: a change");
    assert_eq!(s.report(Situation::CameraTaken, false, 210, 50), Some(false));
    assert_eq!(s.state(Situation::CameraTaken, 220), Some(false), "reported not happening");
}

#[test]
fn a_long_report_is_not_cut_short_by_a_brief_one() {
    let mut s = Situations::new();
    s.report(Situation::Fighting, true, 0, 8000);
    s.report(Situation::Fighting, true, 100, 50);
    assert!(s.active(Situation::Fighting, 7000));
    assert_eq!(s.active_for(Situation::Fighting, 7000), Some(7000));
}

#[test]
fn features_react_to_the_situations_that_concern_them() {
    let mut s = Situations::new();
    s.report(Situation::Traversal, true, 0, 100);
    assert!(s.arms_to_game(10) && s.hands_held_back(10) && !s.head_aim_stands_down(10));
    s.report(Situation::Fighting, true, 0, 100);
    assert!(!s.arms_to_game(10), "a fight's raised hands stay on the controllers");
    s.report(Situation::Video, true, 0, 100);
    assert!(s.flat(10) && s.head_aim_stands_down(10));
    assert_eq!(s.flat_reason(10), Some(Situation::Video));
    assert!(!s.flat(200), "the reports expired");
    assert_eq!(s.summary(), "a video plays 1, climbing 1, fighting 1");
}
