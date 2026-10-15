use super::turned;

#[test]
fn the_stick_is_turned_back_to_the_players_forward() {
    const MAX: i16 = 32767;
    // The game's forward a quarter turn left of the player's (a gun pointed left): forward on
    // the stick goes to the game as its right, which is the player's forward.
    assert_eq!(turned(0, MAX, std::f32::consts::FRAC_PI_2), (MAX, 0));
    // Pointed behind (the rifle over the shoulder): forward goes as back.
    assert_eq!(turned(0, MAX, std::f32::consts::PI), (0, -MAX));
    // Right a quarter turn: forward goes as left; no turn leaves the stick alone.
    assert_eq!(turned(0, MAX, -std::f32::consts::FRAC_PI_2), (-MAX, 0));
    assert_eq!(turned(1200, -800, 0.0), (1200, -800));
    // A full diagonal stays within the axis.
    let (x, y) = turned(MAX, MAX, std::f32::consts::FRAC_PI_4);
    assert!(x == MAX && y.abs() <= 1, "{x} {y}");
}
