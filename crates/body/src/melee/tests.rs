use super::*;
use monaka_core::pad::Gamepad;
use monaka_pad::actions::Binding;

/// The rules [`update`] chose inline before [`swing_rules`].
fn rules_before(held: Kind, side: usize, settings: &Settings, blade: Option<[f32; 3]>) -> Option<SwingRules> {
    let speed = settings.speed / SwingRules::WEAPON.start_speed;
    match held {
        Kind::Melee if side == settings.aim_side => Some(SwingRules::WEAPON.faster(speed)),
        Kind::Blade if side == settings.aim_side => {
            let blade = blade.unwrap_or(SwingRules::BLADE.blade);
            Some(SwingRules::BLADE.faster(speed).pointing(blade))
        }
        Kind::Nothing => Some(SwingRules::FIST.faster(speed)),
        Kind::Gun | Kind::Melee | Kind::Blade => None,
        _ => unreachable!("not a kind before"),
    }
}

#[test]
fn the_swing_rules_are_the_ones_chosen_before_and_new_kinds_never_swing() {
    for aim_side in [LEFT_HAND, RIGHT_HAND] {
        for speed in [0.5, 2.2, 7.0] {
            let settings = Settings { swing: true, speed, aim_side };
            for blade in [None, Some([0.0, 0.6, -0.8])] {
                for side in [LEFT_HAND, RIGHT_HAND] {
                    for held in [Kind::Gun, Kind::Melee, Kind::Blade, Kind::Nothing] {
                        assert_eq!(swing_rules(held, side, &settings, blade), rules_before(held, side, &settings, blade), "{held:?} {side} {aim_side} {speed}");
                    }
                    for held in [Kind::Bow, Kind::Throwable, Kind::Tool] {
                        assert!(swing_rules(held, side, &settings, blade).is_none(), "{held:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_swing_presses_the_right_trigger_as_before() {
    // No game's actions installed in this test run: the standard layout, the trigger a swing held.
    assert_eq!(actions::binding(Action::Attack), Binding::Pad(Gamepad { right_trigger: 255, ..Gamepad::default() }));
}
