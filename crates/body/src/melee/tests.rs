use super::*;
use monaka_contract::gamepad::Gamepad;
use monaka_pad::actions::Binding;

#[test]
fn a_weapon_swings_in_the_aiming_hand_a_fist_in_either_and_the_rest_never() {
    for aim_side in [LEFT_HAND, RIGHT_HAND] {
        let other = if aim_side == LEFT_HAND { RIGHT_HAND } else { LEFT_HAND };
        let settings = Settings { swing: true, speed: 2.2, aim_side };
        let swings = |held: Kind, side: usize| swing_rules(held, side, &settings, None).is_some();
        for held in [Kind::Melee, Kind::Blade] {
            assert!(swings(held, aim_side) && !swings(held, other), "{held:?} {aim_side}");
        }
        assert!(swings(Kind::Nothing, aim_side) && swings(Kind::Nothing, other));
        for held in [Kind::Gun, Kind::Bow, Kind::Throwable, Kind::Tool] {
            assert!(!swings(held, aim_side) && !swings(held, other), "{held:?} {aim_side}");
        }
    }
}

#[test]
fn a_swing_presses_the_right_trigger() {
    // No game's actions installed in this test run: the standard layout, the trigger a swing held.
    assert_eq!(actions::binding(Action::Attack), Binding::Pad(Gamepad { right_trigger: 255, ..Gamepad::default() }));
}
