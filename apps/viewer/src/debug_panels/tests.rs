use super::*;

const MENU: Place = Place { orientation: [0.0, 0.0, 0.0, 1.0], centre: [0.0, 1.5, -1.0], size: [0.7, 0.7] };

fn close(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
}

#[test]
fn a_panel_goes_right_of_the_menu_then_left_then_further_out() {
    let size = [0.5, 0.7];
    let first = free_spot(&MENU, size, &[]);
    assert!(close(first.centre, [0.63, 1.5, -1.0]), "{:?}", first.centre);
    let second = free_spot(&MENU, size, &[first]);
    assert!(close(second.centre, [-0.63, 1.5, -1.0]), "{:?}", second.centre);
    let third = free_spot(&MENU, size, &[first, second]);
    assert!(close(third.centre, [1.16, 1.5, -1.0]), "{:?}", third.centre);
    assert_eq!(third.orientation, MENU.orientation, "facing the way the menu faces");
}

#[test]
fn a_shorter_panel_is_level_with_the_menus_top() {
    let place = free_spot(&MENU, [0.5, 0.5], &[]);
    assert!((place.centre[1] + place.size[1] * 0.5 - (MENU.centre[1] + MENU.size[1] * 0.5)).abs() < 1e-4);
}

#[test]
fn the_spots_follow_a_turned_menu() {
    // The menu turned 90 degrees left (about up): its right is the room's -z.
    let (s, c) = (std::f32::consts::FRAC_PI_4.sin(), std::f32::consts::FRAC_PI_4.cos());
    let menu = Place { orientation: [0.0, s, 0.0, c], centre: [-1.0, 1.5, 0.0], size: [0.7, 0.7] };
    let place = free_spot(&menu, [0.5, 0.7], &[]);
    assert!(close(place.centre, [-1.0, 1.5, -0.63]), "{:?}", place.centre);
}
