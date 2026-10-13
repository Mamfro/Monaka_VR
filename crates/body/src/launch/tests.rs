use super::*;

#[test]
fn a_shot_keeps_the_games_speed_unless_it_says_its_own() {
    let game = Flight { from: [1.0, 2.0, 3.0], velocity: [0.0, 0.0, -40.0] };
    let along = Shot { from: [0.0; 3], direction: [1.0, 0.0, 0.0], speed: None };
    assert_eq!(along.flight(&game), Some(Flight { from: [0.0; 3], velocity: [40.0, 0.0, 0.0] }));
    let faster = Shot { speed: Some(60.0), ..along };
    assert_eq!(faster.flight(&game).map(|f| f.velocity), Some([60.0, 0.0, 0.0]));
    assert!(Shot { speed: Some(f32::NAN), ..along }.flight(&game).is_none());
}

#[test]
fn without_an_answer_the_game_launches_its_own_way() {
    let fired = Fired { weapon: Weapon::Gun, game: Flight { from: [0.0; 3], velocity: [0.0, 0.0, -1.0] }, aim: None, side: None };
    let leaving = Leaving { thrown: Thrown::Throwable, way: Way::Given, hand: HandThrow { from: None, velocity: [0.0; 3] }, game: None };
    aim_shots(Weapon::Gun, None);
    throw_by(None);
    draw_by(None);
    assert!(shot(&fired).is_none() && draw().is_none());
    assert!(let_go(Thrown::Weapon).is_none() && start(&leaving).is_none() && velocity(&leaving).is_none());
    aim_shots(Weapon::Gun, Some(|f| Some(Shot { from: f.game.from, direction: [0.0, 1.0, 0.0], speed: None })));
    assert_eq!(shot(&fired).map(|s| s.direction), Some([0.0, 1.0, 0.0]));
    assert!(shot(&Fired { weapon: Weapon::Bow, ..fired }).is_none(), "each weapon its own answer");
    draw_by(Some(|| Some(0.5)));
    assert_eq!(draw(), Some(0.5));
    draw_by(None);
    throw_by(Some(Thrower { let_go: |_| Some(HandThrow { from: Some([1.0; 3]), velocity: [2.0; 3] }), start: |l| l.hand.from, velocity: |l| Some(l.hand.velocity), throwing: || true }));
    let thrown = let_go(Thrown::Throwable).unwrap();
    assert_eq!(start(&Leaving { hand: thrown, ..leaving }), Some([1.0; 3]));
    assert_eq!(velocity(&Leaving { hand: thrown, ..leaving }), Some([2.0; 3]));
    aim_shots(Weapon::Gun, None);
    throw_by(None);
}
