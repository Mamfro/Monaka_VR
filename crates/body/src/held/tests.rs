use super::*;

#[test]
fn only_an_empty_hand_and_a_bow_hand_take_tracked_fingers() {
    // Every kind the hand rig knew before: only an empty hand was free.
    for kind in [Kind::Gun, Kind::Melee, Kind::Blade, Kind::Nothing] {
        assert_eq!(!kind.grips(), kind == Kind::Nothing, "{kind:?}");
    }
    assert!(!Kind::Bow.grips());
    assert!(Kind::Throwable.grips() && Kind::Tool.grips());
}

#[test]
fn a_new_kind_or_item_comes_to_hand_and_the_same_stays() {
    let mut draws = Draws::new();
    assert_eq!(draws.note(1, Kind::Nothing, None, 100), 100, "first asked: then");
    assert_eq!(draws.note(1, Kind::Nothing, None, 400), 100);
    assert_eq!(draws.note(1, Kind::Melee, Some(7), 500), 500, "drawn");
    assert_eq!(draws.note(1, Kind::Melee, Some(7), 900), 500);
    assert_eq!(draws.note(1, Kind::Melee, Some(8), 950), 950, "another weapon of the same kind: switched");
    assert_eq!(draws.note(0, Kind::Bow, Some(3), 960), 960, "each hand its own");
    assert_eq!(draws.note(1, Kind::Nothing, None, 1000), 1000, "put away");
    let held = HeldItem { kind: Kind::Melee, item: None, since: 500 };
    assert!(held.drawn_within(600, 200) && !held.drawn_within(700, 200));
    assert_eq!(held.grip(), Grip::Measured);
}

#[test]
fn the_aim_hand_is_wielded_as_melee_takes_it() {
    let hand = |kind| Some(HeldItem { kind, item: None, since: 0 });
    let right = RIGHT_HAND;
    assert_eq!(wielded_of([None, None], right), None, "nothing posed yet");
    assert_eq!(wielded_of([hand(Kind::Nothing), hand(Kind::Melee)], right), Some(Kind::Melee));
    assert_eq!(wielded_of([hand(Kind::Nothing), hand(Kind::Throwable)], right), Some(Kind::Melee), "a throwable as its melee weapon");
    assert_eq!(wielded_of([hand(Kind::Bow), hand(Kind::Nothing)], right), Some(Kind::Gun), "a bow swings nothing");
    assert_eq!(wielded_of([hand(Kind::Bow), hand(Kind::Nothing)], LEFT_HAND), Some(Kind::Gun));
    assert_eq!(wielded_of([hand(Kind::Gun), hand(Kind::Nothing)], LEFT_HAND), Some(Kind::Gun));
    assert!(Kind::Nothing.melee() && Kind::Blade.melee() && !Kind::Gun.melee() && !Kind::Throwable.melee());
}

#[test]
fn each_posing_notes_both_hands_with_when_they_came_to_hand() {
    let mut latest = Latest { sides: [None; 2], draws: Draws::new() };
    let knife = Some(Item { key: 7, grip: Grip::One });
    latest.note([Kind::Nothing, Kind::Melee], [None, knife], 100);
    latest.note([Kind::Nothing, Kind::Melee], [None, knife], 300);
    assert_eq!(latest.sides[RIGHT_HAND], Some(HeldItem { kind: Kind::Melee, item: knife, since: 100 }));
    latest.note([Kind::Bow, Kind::Nothing], [None, None], 400);
    assert_eq!(latest.sides.map(|h| h.map(|h| (h.kind, h.since))), [Some((Kind::Bow, 400)), Some((Kind::Nothing, 400))]);
}

/// A holder turned a quarter about up (its x along the world's -z), scaled by 2, at (1, 2, 3).
const HOLDER: Mat34 = [0.0, 0.0, 2.0, 1.0, 0.0, 2.0, 0.0, 2.0, -2.0, 0.0, 0.0, 3.0];

#[test]
fn a_line_goes_into_the_world_on_its_holder_unscaled() {
    let blade = Line { from: [0.0, 0.1, 0.0], along: [1.0, 0.0, 0.0], length: Some(0.8), edge: Some([0.0, 0.0, 1.0]) };
    let world = blade.in_world(&HOLDER).unwrap();
    let close = |a: Vec3, b: Vec3| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
    assert!(close(world.from, [1.0, 2.1, 3.0]), "the hilt 0.1 m up the holder, not 0.2: {:?}", world.from);
    assert!(close(world.along, [0.0, 0.0, -1.0]), "{:?}", world.along);
    assert!(close(world.edge.unwrap(), [1.0, 0.0, 0.0]));
    assert!(close(world.end().unwrap(), [1.0, 2.1, 2.2]));
    assert!(close(world.at(0.5), [1.0, 2.1, 2.5]));
    let flat = [0.0; 12];
    assert!(blade.in_world(&flat).is_none(), "a holder with no axes has no line");
}

#[test]
fn a_line_starts_level_with_its_holders_origin() {
    // An arrow 0.2 m behind and 0.05 m beside the holder, running along x.
    let arrow = Line { from: [-0.2, 0.05, 0.0], along: [1.0, 0.0, 0.0], length: None, edge: None };
    assert_eq!(arrow.level_with_origin(), [0.0, 0.05, 0.0]);
}
