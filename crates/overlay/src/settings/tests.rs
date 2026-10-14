use super::*;

fn menu() -> SettingsMenu {
    SettingsMenu::parse("controller_smoothing=3,menu_reach=same,unknown=1,menu_reach_depth=40,controller_proxy=off")
}

#[test]
fn the_lines_are_the_live_options_named() {
    let menu = menu();
    let ids: Vec<&str> = menu.lines.iter().map(|l| l.opt.id).collect();
    assert_eq!(ids, ["controller_smoothing", "menu_reach", "menu_reach_depth", "controller_proxy"]);
    assert_eq!(menu.lines[0].value, "3");
    assert!(SettingsMenu::parse("").lines.is_empty());
}

#[test]
fn values_step_within_their_kind() {
    let mut menu = menu();
    let changed = menu.press(Press::Right).cloned();
    assert_eq!(changed.map(|l| l.value), Some("4".to_owned()), "a number steps");
    menu.press(Press::Down);
    assert_eq!(menu.press(Press::Left).map(|l| l.value.clone()), Some("off".to_owned()), "a choice goes round");
    assert_eq!(menu.press(Press::Switch).map(|l| l.value.clone()), Some("same".to_owned()), "A moves a choice on");
    menu.press(Press::Down);
    assert!(menu.press(Press::Right).is_none(), "at its top: no change");
    assert!(menu.press(Press::Switch).is_none(), "A leaves a number");
    assert_eq!(menu.press(Press::Left).map(|l| l.value.clone()), Some("39".to_owned()));
    menu.press(Press::Down);
    menu.press(Press::Down);
    assert_eq!(menu.selected, 0, "round to the top");
    menu.open = true;
    menu.press(Press::Close);
    assert!(!menu.open);
}

#[test]
fn values_show_as_the_window_names_them() {
    let proxy = LIVE.iter().find(|o| o.id == "controller_proxy").unwrap();
    assert_eq!(shown(proxy, "touch"), "<  Quest controllers  >");
    let smoothing = LIVE.iter().find(|o| o.id == "controller_smoothing").unwrap();
    assert_eq!(shown(smoothing, "3"), "<  3  >");
}
