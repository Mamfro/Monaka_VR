use super::*;

#[test]
fn only_input_back_toward_level_passes() {
    let down = Some(-0.4);
    assert_eq!(passed(0.8, 0.0, true, true, down), 0.8, "looking down, up passes");
    assert_eq!(passed(0.8, 0.0, false, true, down), 0.0, "looking down, further down is held");
    assert_eq!(passed(0.8, 1.0, false, true, Some(0.4)), 0.8, "looking up, down passes (an inverted binding)");
    assert_eq!(passed(0.8, 0.0, true, true, Some(0.01)), 0.0, "level: nothing passes");
    assert_eq!(passed(0.8, 0.0, true, true, None), 0.0, "the pitch not known: held");
    assert_eq!(passed(0.8, 0.0, false, false, down), 0.8, "the lock not holding: as the game made it");
}
