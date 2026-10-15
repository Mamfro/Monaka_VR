use super::*;

const MENU: Place = Place { orientation: [0.0, 0.0, 0.0, 1.0], centre: [0.0, 1.5, -1.0], size: [0.7, 0.7] };

fn close(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
}

#[test]
fn the_panels_follow_the_menu_in_a_row_with_it_where_it_opened() {
    let row = strip(&MENU, &[MENU.size, [0.5, 0.7], [0.5, 0.7]], 0);
    assert!(close(row[0].centre, MENU.centre), "the menu where it opened: {:?}", row[0].centre);
    assert!(close(row[1].centre, [0.63, 1.5, -1.0]), "{:?}", row[1].centre);
    assert!(close(row[2].centre, [1.16, 1.5, -1.0]), "{:?}", row[2].centre);
    assert_eq!(row[2].orientation, MENU.orientation, "facing the way the menu faces");
}

#[test]
fn the_row_slides_so_the_one_driven_is_where_the_menu_opened() {
    let row = strip(&MENU, &[MENU.size, [0.5, 0.7], [0.5, 0.7]], 2);
    assert!(close(row[2].centre, MENU.centre), "{:?}", row[2].centre);
    assert!(close(row[1].centre, [-0.53, 1.5, -1.0]), "{:?}", row[1].centre);
    assert!(close(row[0].centre, [-1.16, 1.5, -1.0]), "the menu off to the left: {:?}", row[0].centre);
    assert_eq!(strip(&MENU, &[MENU.size], 5).len(), 1, "past the end: the last");
}

#[test]
fn a_shorter_panel_is_level_with_the_menus_top() {
    let row = strip(&MENU, &[MENU.size, [0.5, 0.5]], 1);
    assert!((row[1].centre[1] + row[1].size[1] * 0.5 - (MENU.centre[1] + MENU.size[1] * 0.5)).abs() < 1e-4);
}

#[test]
fn the_row_follows_a_turned_menu() {
    // The menu turned 90 degrees left (about up): its right is the room's -z.
    let (s, c) = (std::f32::consts::FRAC_PI_4.sin(), std::f32::consts::FRAC_PI_4.cos());
    let menu = Place { orientation: [0.0, s, 0.0, c], centre: [-1.0, 1.5, 0.0], size: [0.7, 0.7] };
    let row = strip(&menu, &[menu.size, [0.5, 0.7]], 0);
    assert!(close(row[1].centre, [-1.0, 1.5, -0.63]), "{:?}", row[1].centre);
}
