use super::*;
use monaka_contract::manifest::Kind;

#[test]
fn a_games_size_keeps_the_distance() {
    let [size, distance] = options(0.55);
    assert_eq!((size.id, distance.id), ("hud_scale", "hud_distance"));
    assert!(matches!(size.kind, Kind::Number { default, .. } if default == 0.55));
    assert!(matches!(OPTIONS[0].kind, Kind::Number { default, .. } if default == 0.5));
}
