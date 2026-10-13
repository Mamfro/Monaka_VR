use super::*;
use crate::launch::Flight;

fn fired(velocity: Vec3, aim: Option<Vec3>) -> Fired {
    Fired { weapon: Weapon::Gun, game: Flight { from: [0.0; 3], velocity }, aim, side: None }
}

#[test]
fn a_pellet_keeps_its_spread_around_the_barrel() {
    let barrel = math::unit([0.2, 0.1, -1.0]).unwrap();
    let muzzle = [1.0, 1.5, -0.5];
    // A projectile straight along the aim leaves straight along the barrel.
    let straight = from_barrel(&fired([0.0, 0.0, -300.0], Some([0.0, 0.0, -1.0])), muzzle, barrel).unwrap();
    assert!(straight.direction.iter().zip(barrel).all(|(a, b)| (a - b).abs() < 1e-6) && straight.from == muzzle);
    // A pellet a little off the aim is as far off the barrel.
    let off = math::unit([0.05, 0.0, -1.0]).unwrap();
    let pellet = from_barrel(&fired(off.map(|c| c * 300.0), Some([0.0, 0.0, -1.0])), muzzle, barrel).unwrap();
    let apart = |a: Vec3, b: Vec3| math::dot(a, b).clamp(-1.0, 1.0).acos();
    let (now, was) = (apart(pellet.direction, barrel), apart(off, [0.0, 0.0, -1.0]));
    assert!((now - was).abs() < 1e-3, "{now} {was}");
    assert_eq!(pellet.speed, None, "the game's speed kept");
}

#[test]
fn without_the_games_aim_or_a_velocity_the_game_shoots_its_own_way() {
    assert!(from_barrel(&fired([0.0, 0.0, -300.0], None), [0.0; 3], [0.0, 0.0, -1.0]).is_none());
    assert!(from_barrel(&fired([0.0; 3], Some([0.0, 0.0, -1.0])), [0.0; 3], [0.0, 0.0, -1.0]).is_none());
}
