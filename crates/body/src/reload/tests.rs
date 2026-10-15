use super::*;

#[test]
fn each_games_rules_are_the_ones_it_had() {
    assert_eq!(Reloading::FRESH.touch, TouchAwayRules { near: 0.15, away: 0.25, within: 1.0, cooldown: 1.5 });
    assert_eq!((Reloading::FRESH.fresh, Reloading::LATEST.fresh), (Some(0.1), None));
    assert_eq!(Reloading::LATEST.touch, Reloading::FRESH.touch);
}

#[test]
fn an_empty_or_other_hand_feeds_nothing() {
    let mut gesture = TouchAway::new();
    let rules = TouchAwayRules::RELOAD;
    for held in [Kind::Nothing, Kind::Melee, Kind::Bow, Kind::Throwable] {
        assert!(!gestured(&mut gesture, 0.0, held, Some(0.1), &rules));
    }
    assert_eq!(gesture, TouchAway::new(), "no touch noted");
    assert!(!gestured(&mut gesture, 0.0, Kind::Gun, None, &rules), "a hand not placed");
    assert!(!gestured(&mut gesture, 0.1, Kind::Gun, Some(0.1), &rules));
    assert!(gestured(&mut gesture, 0.3, Kind::Gun, Some(0.3), &rules));
}
