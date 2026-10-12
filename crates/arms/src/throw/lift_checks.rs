//! The ready-made against Dying Light 1's throwing by hand before it, moment by moment.

use super::*;
use crate::hand_trail::{Lost, TrailRules};
use std::time::Duration;

/// Dying Light 1's rules: the hand's speed times 1.6 given, 3 launched; a weapon's swing over 0.5 s.
const DL1: Throwing = Throwing { given: 1.6, launched: 3.0, weapon_within: 0.5 };

fn throw_rules(span: f64) -> ReleaseRules {
    ReleaseRules { span, ..ReleaseRules::THROW }
}

/// Dying Light 1's let-go before: the left controller's release from the left hand; the right
/// controller's fastest over half a second from the right hand.
fn old_dl1_let_go(thrown: Thrown, controllers: &HandTrails, hands: &HandTrails) -> Option<HandThrow> {
    match thrown {
        Thrown::Throwable => {
            let throw = controllers.release(LEFT_HAND, &throw_rules(0.07))?;
            hands.position(LEFT_HAND).map(|position| HandThrow { from: Some(position), velocity: throw.velocity })
        }
        Thrown::Weapon => hands.position(RIGHT_HAND).zip(controllers.fastest(RIGHT_HAND, 0.5)).map(|(at, velocity)| HandThrow { from: Some(at), velocity }),
    }
}

fn old_dl1_start(leaving: &Leaving) -> Option<Vec3> {
    leaving.hand.from
}

fn old_dl1_velocity(leaving: &Leaving) -> Option<Vec3> {
    let hand = leaving.hand.velocity;
    match leaving.way {
        Way::Given => ThrowSpeed::gain(1.6).thrown(hand),
        Way::Launched => ThrowSpeed::gain(3.0).thrown(hand),
        Way::AtPoint => Some(hand),
    }
}

/// Each leaving the game may ask about for `hand`, the game's own velocity `game`.
fn leavings(hand: HandThrow, game: Vec3) -> Vec<Leaving> {
    let mut all = Vec::new();
    for thrown in [Thrown::Throwable, Thrown::Weapon] {
        for way in [Way::Given, Way::Launched, Way::AtPoint] {
            for game in [None, Some(game), Some([0.0; 3])] {
                all.push(Leaving { thrown, way, hand, game });
            }
        }
    }
    all
}

#[test]
fn dying_light_1s_throws_are_the_ones_it_made_before() {
    // Its controllers' trail forgets a lost hand; its hands are the arms' hands read back.
    let controllers = HandTrails::new(TrailRules { kept: 0.6, span: 0.07, lost: Lost::Forgotten });
    let hands = HandTrails::new(TrailRules { kept: 0.6, span: 0.07, lost: Lost::Forgotten });
    let (mut compared, mut thrown) = (0, 0);
    for i in 0..160 {
        let t = i as f32 * 0.01;
        // The left hand winding up and flinging forward now and then, the right swinging; the left
        // lost for a while, the hands at rest for a while.
        let still = (100..125).contains(&i);
        let left = (!(60..70).contains(&i)).then(|| if still { [-0.2, 1.2, -0.3] } else { [-0.2 + 0.1 * t.sin(), 1.2 + 0.3 * (t * 7.0).sin(), -0.3 - 0.6 * (t * 5.0).sin().max(0.0)] });
        let right = if still { [0.3, 1.1, -0.4] } else { [0.3 + 0.5 * (t * 9.0).cos(), 1.1 + 0.2 * (t * 4.0).sin(), -0.4] };
        controllers.note_positions([left, Some(right)]);
        hands.note_positions([left.map(|p| [p[0] + 0.02, p[1], p[2]]), Some([right[0], right[1] - 0.03, right[2]])]);
        let game = [0.0, 3.0, -14.0 - t];
        for kind in [Thrown::Throwable, Thrown::Weapon] {
            let was = old_dl1_let_go(kind, &controllers, &hands);
            let now = DL1.let_go(kind, &controllers, |side| hands.position(side));
            assert_eq!(now, was, "Dying Light 1's let-go of a {kind:?} at {i}");
            if let Some(hand) = now {
                for leaving in leavings(hand, game) {
                    assert_eq!(DL1.start(&leaving), old_dl1_start(&leaving), "{leaving:?}");
                    assert_eq!(DL1.velocity(&leaving), old_dl1_velocity(&leaving), "{leaving:?}");
                    compared += 1;
                }
                thrown += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    assert!(thrown > 200 && compared > 3500, "{thrown} {compared}");
}
