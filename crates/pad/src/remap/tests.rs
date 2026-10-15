use super::*;
use crate::TEST_PRESSES;
use crate::actions::{self, Action};
use crate::presses::pressed;
use monaka_core::pad::merge;
use monaka_contract::gamepad::button;
use monaka_producer::game::Adapter;

/// The controllers' B as the d-pad's down.
const B_DOWN: &[Remap] = &[Remap { from: button::B, to: button::DPAD_DOWN }];

#[test]
fn a_remapped_button_reaches_the_game_as_its_target_and_the_rest_as_they_are() {
    let mut held = Held::NONE;
    let mut read = |buttons: u16| remapped(buttons, B_DOWN, || true, &mut held);
    assert_eq!(read(0), 0);
    assert_eq!(read(button::B), button::DPAD_DOWN);
    assert_eq!(read(button::B | button::A), button::DPAD_DOWN | button::A, "A as it is");
    assert_eq!(read(button::A), button::A, "B let go: the d-pad's down too");
    assert_eq!(read(button::B | button::DPAD_DOWN), button::DPAD_DOWN, "the controllers' own d-pad down merges with it");
}

#[test]
fn a_button_stays_as_it_went_down_until_let_go() {
    let mut held = Held::NONE;
    let mut read = |buttons: u16, applies: bool| remapped(buttons, B_DOWN, || applies, &mut held);
    assert_eq!(read(button::B, false), button::B, "pressed while the remap does not apply: B");
    assert_eq!(read(button::B, true), button::B, "it applies now, but B is still the press it was");
    assert_eq!(read(0, true), 0);
    assert_eq!(read(button::B, true), button::DPAD_DOWN, "the next press is remapped");
    assert_eq!(read(button::B, false), button::DPAD_DOWN, "and stays so while held");
    assert_eq!(read(0, false), 0);
    assert_eq!(read(button::B, false), button::B);
}

#[test]
fn applies_is_asked_only_as_a_remapped_button_goes_down() {
    let mut held = Held::NONE;
    let mut asked = 0;
    for buttons in [0, button::A, button::B, button::B, button::B | button::A, 0, button::B] {
        remapped(buttons, B_DOWN, || {
            asked += 1;
            true
        }, &mut held);
    }
    assert_eq!(asked, 2);
}

struct Layout;
impl Game for Layout {
    const MANIFESTS: &'static [monaka_contract::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_contract::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}
impl Remaps for Layout {
    const REMAPS: &'static [Remap] = B_DOWN;
}

#[test]
fn a_press_of_the_producers_own_reaches_the_game_as_pressed() {
    let _serial = TEST_PRESSES.locked();
    actions::release();
    install::<Layout>();
    let vr_b = Gamepad { buttons: button::B, ..Gamepad::default() };
    // The read: the controllers' pad remapped, merged with what the producer presses.
    let read = |vr: Gamepad| merge(controllers(vr), pressed());
    assert_eq!(read(vr_b).buttons, button::DPAD_DOWN, "the controllers' B is the flashlight");
    // The standard crouch is a toggle on B: a duck taps it.
    assert!(actions::hold(Action::Crouch, true));
    assert_eq!(read(Gamepad::default()).buttons, button::B, "the duck's B reaches the game as B");
    assert_eq!(read(vr_b).buttons, button::B | button::DPAD_DOWN, "both at once: each as it is meant");
    assert!(actions::hold(Action::Crouch, false));
    crate::let_go(Action::Crouch.source());
    actions::release();
    release();
    assert_eq!(controllers(vr_b), vr_b, "released: the controllers' buttons as they are");
}
