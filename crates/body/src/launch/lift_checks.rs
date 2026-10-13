//! What aims itself against Dying Light 1's own answer, which it replaced.

use super::*;
use crate::held::Kind;

/// Dying Light 1's: a bow held by hand; a gun held while shooting from the barrel with the hand rig;
/// a throw under way while throwing by hand with the hand rig and the left controller aiming the
/// tool.
fn before(bow: bool, shoot_from_barrel: bool, throw_by_hand: bool, rig: bool, held: Option<Kind>, tool: bool, throwing: bool) -> bool {
    let gun_aims_itself = shoot_from_barrel && rig && held == Some(Kind::Gun);
    let thrown_by_hand = tool && throw_by_hand && rig && throwing;
    bow || gun_aims_itself || thrown_by_hand
}

#[test]
fn what_aims_itself_is_what_dying_light_1_said() {
    let helds = [None, Some(Kind::Nothing), Some(Kind::Gun), Some(Kind::Bow)];
    for bits in 0..64u32 {
        let [bow, shoot_from_barrel, throw_by_hand, rig, tool, throwing] = std::array::from_fn(|i| bits & (1 << i) != 0);
        for held in helds {
            // The gun and throw ready-mades are on with their switch and the hand rig.
            let (gun_on, throw_on) = (shoot_from_barrel && rig, throw_by_hand && rig);
            let now = aims_itself_with(bow, gun_on, held, tool, || throw_on && throwing);
            assert_eq!(now, before(bow, shoot_from_barrel, throw_by_hand, rig, held, tool, throwing), "{bits:06b} {held:?}");
        }
    }
}
