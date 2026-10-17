use super::*;

#[test]
fn frosted_keeps_the_panels_colour_and_lets_the_game_through() {
    let glass = HOME.frosted();
    let [r, g, b, a] = glass.background.to_array();
    assert_eq!(a, GLASS);
    let solid = |c: u8| c as u32 * 255 / a as u32;
    let [hr, hg, hb, ha] = HOME.background.to_array();
    let home = |c: u8| c as u32 * 255 / ha as u32;
    for (frosted, own) in [(solid(r), home(hr)), (solid(g), home(hg)), (solid(b), home(hb))] {
        assert!(frosted.abs_diff(own) <= 2, "the same oxblood under the glass: {frosted} vs {own}");
    }
    assert!(glass.button.a() < 255);
    assert_eq!((glass.row, glass.accent, glass.text), (HOME.row, HOME.accent, HOME.text));
}

#[test]
fn shading_stays_within_the_colours_own_alpha() {
    assert_eq!(shade(Color32::from_gray(60), 10), Color32::from_gray(70));
    assert_eq!(shade(Color32::from_gray(60), -5), Color32::from_gray(55));
    let see_through = Color32::from_rgba_premultiplied(40, 30, 30, 50);
    let [r, g, b, a] = shade(see_through, 100).to_array();
    assert_eq!(a, 50);
    assert!(r <= a && g <= a && b <= a, "still premultiplied");
}
