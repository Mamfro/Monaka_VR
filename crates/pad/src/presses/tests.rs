use super::*;
use crate::TEST_PRESSES;
use crate::vr_pad::stop;

#[test]
fn a_hold_stays_until_let_go_or_the_stop() {
    let _serial = TEST_PRESSES.locked();
    hold("guard", Gamepad { buttons: monaka_contract::gamepad::button::RIGHT_SHOULDER, ..Gamepad::default() });
    press("melee", Gamepad { right_trigger: 255, ..Gamepad::default() }, 60_000);
    assert_eq!((pressed().buttons, pressed().right_trigger), (monaka_contract::gamepad::button::RIGHT_SHOULDER, 255));
    let_go("guard");
    assert_eq!(pressed().buttons, 0, "let go of the hold alone");
    hold("guard", Gamepad { buttons: monaka_contract::gamepad::button::RIGHT_SHOULDER, ..Gamepad::default() });
    stop();
    assert_eq!(pressed(), Gamepad { right_trigger: 255, ..Gamepad::default() }, "the stop lets go of holds, a timed press runs out");
    let_go("melee");
    press("long", Gamepad { buttons: 1, ..Gamepad::default() }, u64::MAX);
    assert_eq!(pressed().buttons, 1, "a press for ever does not overflow");
    let_go("long");
}

#[test]
fn each_source_presses_and_lets_go_on_its_own() {
    let _serial = TEST_PRESSES.locked();
    press("melee", Gamepad { right_trigger: 255, ..Gamepad::default() }, 60_000);
    press("climb", Gamepad { thumb_rx: 12_000, ..Gamepad::default() }, 60_000);
    let both = pressed();
    assert_eq!((both.right_trigger, both.thumb_rx), (255, 12_000), "two sources held together");
    let_go("climb");
    let melee = pressed();
    assert_eq!((melee.right_trigger, melee.thumb_rx), (255, 0), "the climb's let-go leaves the melee press");
    press("melee", Gamepad::default(), 0);
    assert_eq!(pressed(), Gamepad::default(), "a source's new press replaces its own, and an expired one is gone");
}
