use super::*;
use crate::hand_trail::{Lost, TrailRules};
use monaka_contract::protocol::HandPose;
use std::time::Duration;

const HAND_SPEED: Throwing = Throwing { given: 2.0, launched: 3.0, weapon_within: 0.5 };

fn leaving(thrown: Thrown, way: Way, velocity: Vec3, game: Option<Vec3>) -> Leaving {
    Leaving { thrown, way, hand: HandThrow { from: Some([1.0, 1.5, -2.0]), velocity }, game }
}

#[test]
fn the_hands_speed_is_scaled_by_the_way_the_item_leaves() {
    let hand = [0.0, 1.0, -3.0];
    assert_eq!(HAND_SPEED.velocity(&leaving(Thrown::Throwable, Way::Given, hand, None)), Some([0.0, 2.0, -6.0]));
    assert_eq!(HAND_SPEED.velocity(&leaving(Thrown::Throwable, Way::Launched, hand, None)), Some([0.0, 3.0, -9.0]));
    assert_eq!(HAND_SPEED.velocity(&leaving(Thrown::Weapon, Way::AtPoint, hand, None)), Some(hand), "at a point: the hand's own");
    assert_eq!(HAND_SPEED.start(&leaving(Thrown::Weapon, Way::AtPoint, hand, None)), Some([1.0, 1.5, -2.0]), "from the hand");
}

#[test]
fn the_games_own_speed_for_the_item_plays_no_part() {
    let hand = [0.5, 0.5, -4.0];
    let alone = HAND_SPEED.velocity(&leaving(Thrown::Throwable, Way::Given, hand, None));
    for game in [Some([0.0, 2.0, -30.0]), Some([0.0; 3])] {
        assert_eq!(HAND_SPEED.velocity(&leaving(Thrown::Throwable, Way::Given, hand, game)), alone, "{game:?}");
        assert_eq!(HAND_SPEED.velocity(&leaving(Thrown::Weapon, Way::Given, hand, game)), alone, "{game:?}");
    }
    assert_eq!(HAND_SPEED.velocity(&leaving(Thrown::Throwable, Way::Given, [0.0; 3], Some([0.0, 0.0, -20.0]))), Some([0.0; 3]), "a hand at rest throws at rest");
}

fn palm(at: Vec3) -> HandPose {
    HandPose { position: at, orientation: [0.0, 0.0, 0.0, 1.0], valid: 1, received_tick: 0 }
}

/// The Chrome games' way: the palms noted on the tracking origin (kept while a hand is lost), each
/// hand where its palm was noted within 0.1 s, every item given a velocity.
#[test]
fn a_throw_read_off_the_palms_leaves_from_the_left_palm_and_the_weapon_from_the_right() {
    let palms = HandTrails::new(TrailRules { kept: 0.8, span: 0.06, lost: Lost::Kept });
    let origin = [1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 5.0];
    let hand = |side: usize| palms.palm(side, 0.1).map(|p| [p[3], p[7], p[11]]);
    assert_eq!(HAND_SPEED.let_go(Thrown::Throwable, &palms, hand), None, "nothing noted: the game's throw");
    let start = std::time::Instant::now();
    for _ in 0..25 {
        let t = start.elapsed().as_secs_f32().min(0.2);
        // The left palm flung forward (-z) at 4 m/s for 0.2 s, then still; the right swung sideways
        // at 3 m/s and back at 1 m/s.
        let right = if t < 0.1 { 3.0 * t } else { 0.3 - (t - 0.1) };
        palms.note(&origin, &[Some(palm([-0.2, 1.2, -4.0 * t])), Some(palm([0.3 + right, 1.1, -0.4]))]);
        std::thread::sleep(Duration::from_millis(12));
    }
    let left = palms.palm(LEFT_HAND, 0.1).map(|p| [p[3], p[7], p[11]]).unwrap();
    let throwable = HAND_SPEED.let_go(Thrown::Throwable, &palms, hand).unwrap();
    assert_eq!(throwable.from, Some(left), "from the left palm, on the tracking origin");
    assert!((left[0] - 9.8).abs() < 1e-5 && (left[2] - 4.2).abs() < 1e-3, "{left:?}");
    assert!(throwable.velocity[2] < -3.0 && throwable.velocity[0].abs() < 0.5, "the fling's speed and way: {:?}", throwable.velocity);
    let given = Leaving { thrown: Thrown::Throwable, way: Way::Given, hand: throwable, game: Some([0.0, 0.0, -25.0]) };
    assert_eq!(HAND_SPEED.start(&given), Some(left));
    assert_eq!(HAND_SPEED.velocity(&given), Some(throwable.velocity.map(|c| c * 2.0)));
    let weapon = HAND_SPEED.let_go(Thrown::Weapon, &palms, hand).unwrap();
    assert_eq!(weapon.from, hand(RIGHT_HAND), "from the right palm");
    assert!(weapon.velocity[0] > 2.0, "the swing's fastest, not its end: {:?}", weapon.velocity);
    assert_eq!(HAND_SPEED.let_go(Thrown::Weapon, &palms, |_| None), None, "no hand noted lately: the game's throw");
}
