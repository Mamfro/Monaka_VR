use super::*;

#[test]
fn the_layer_makes_a_dpad_and_back_and_then_lets_go_of_the_bumper() {
    let mut used = false;
    // Held alone (the grip as a bumper): nothing changes.
    let mut pad = Gamepad { buttons: button::LEFT_SHOULDER, thumb_lx: 3000, ..Default::default() };
    layer(&mut pad, true, true, &mut used);
    assert_eq!((pad.buttons, pad.thumb_lx, used), (button::LEFT_SHOULDER, 3000, false));
    // The stick pushed left: d-pad left, no movement, the bumper let go.
    let mut pad = Gamepad { buttons: button::LEFT_SHOULDER, thumb_lx: -30000, thumb_ly: 5000, ..Default::default() };
    layer(&mut pad, true, true, &mut used);
    assert_eq!((pad.buttons, pad.thumb_lx, pad.thumb_ly, used), (button::DPAD_LEFT, 0, 0, true));
    // Still the same hold: the bumper stays let go.
    let mut pad = Gamepad { buttons: button::LEFT_SHOULDER | button::START, thumb_ly: -30000, ..Default::default() };
    layer(&mut pad, true, true, &mut used);
    assert_eq!(pad.buttons, button::DPAD_DOWN | button::BACK);
    // With the thumb on the left thumbrest, the right stick: d-pad right, no turning.
    let mut pad = Gamepad { thumb_rx: 30000, thumb_lx: 2000, ..Default::default() };
    layer(&mut pad, true, true, &mut used);
    assert_eq!((pad.buttons, pad.thumb_rx, pad.thumb_lx), (button::DPAD_RIGHT, 0, 2000));
    // Let go of the layer: everything as it was, Start is Start again.
    let mut pad = Gamepad { buttons: button::LEFT_SHOULDER | button::START, thumb_ly: -30000, ..Default::default() };
    layer(&mut pad, false, true, &mut used);
    assert_eq!((pad.buttons, pad.thumb_ly, used), (button::LEFT_SHOULDER | button::START, -30000, false));
}

#[test]
fn a_proxy_hides_what_the_controller_lacks_and_the_grip_stands_in() {
    let all = DPAD | button::BACK | button::START | button::A | button::LEFT_THUMB;
    // As Quest controllers: no d-pad, no view button; the grip is the thumbrest (the layer).
    let mut pad = Gamepad { buttons: all, ..Default::default() };
    assert_eq!(proxied(&mut pad, Proxy::Touch, true), (true, false));
    assert_eq!(pad.buttons, button::START | button::A);
    // As Index controllers: the view button stays; the grip is the trackpad touch (the shift).
    let mut pad = Gamepad { buttons: all, ..Default::default() };
    assert_eq!(proxied(&mut pad, Proxy::Index, true), (false, true));
    assert_eq!(pad.buttons, button::BACK | button::START | button::A);
    // Not standing in: the stick click stays; no proxy: nothing changes.
    let mut pad = Gamepad { buttons: all, ..Default::default() };
    assert_eq!(proxied(&mut pad, Proxy::Index, false), (false, false));
    assert!(pad.buttons & button::LEFT_THUMB != 0);
    let mut pad = Gamepad { buttons: all, ..Default::default() };
    assert_eq!((proxied(&mut pad, Proxy::None, true), pad.buttons), ((false, false), all));
    // The shift makes the d-pad but leaves Start as Start.
    let mut used = false;
    let mut pad = Gamepad { buttons: button::START, thumb_rx: -30000, ..Default::default() };
    layer(&mut pad, true, false, &mut used);
    assert_eq!(pad.buttons, button::START | button::DPAD_LEFT);
    assert_eq!(Proxy::parse("quest"), Ok(Proxy::Touch));
    assert!(Proxy::parse("vive").is_err());
}

#[test]
fn a_held_choice_stays_open_through_the_pick() {
    let hold = dpad_directions("left, right").unwrap();
    let t = Instant::now();
    let ms = |n: u64| t + Duration::from_millis(n);
    let mut latch = Latch::default();
    let read = |latch: &mut Latch, buttons: u16, pushed: u16, at: Instant| {
        let mut pad = Gamepad { buttons, ..Default::default() };
        latch.step(&mut pad, pushed, hold, at);
        pad.buttons
    };
    // Pushed left (the tools): held, and stays held once the stick comes back.
    assert_eq!(read(&mut latch, button::DPAD_LEFT, button::DPAD_LEFT, ms(0)), button::DPAD_LEFT);
    assert_eq!(read(&mut latch, 0, 0, ms(100)), button::DPAD_LEFT);
    // X picks a tool with the d-pad still held; letting go of X lets go of the d-pad after.
    assert_eq!(read(&mut latch, button::X, 0, ms(200)), button::DPAD_LEFT | button::X);
    assert_eq!(read(&mut latch, 0, 0, ms(300)), button::DPAD_LEFT);
    assert_eq!(read(&mut latch, 0, 0, ms(400)), 0);
    // Up is not a held choice: only while pushed.
    assert_eq!(read(&mut latch, button::DPAD_UP, button::DPAD_UP, ms(500)), button::DPAD_UP);
    assert_eq!(read(&mut latch, 0, 0, ms(600)), 0);
    // Pushed right, then right again: let go. And on its own after a while.
    read(&mut latch, button::DPAD_RIGHT, button::DPAD_RIGHT, ms(700));
    read(&mut latch, 0, 0, ms(800));
    assert_eq!(read(&mut latch, button::DPAD_RIGHT, button::DPAD_RIGHT, ms(900)), button::DPAD_RIGHT, "the push itself still reads");
    assert_eq!(read(&mut latch, 0, 0, ms(1000)), 0);
    read(&mut latch, button::DPAD_RIGHT, button::DPAD_RIGHT, ms(1100));
    assert_eq!(read(&mut latch, 0, 0, ms(1200)), button::DPAD_RIGHT);
    assert_eq!(read(&mut latch, 0, 0, ms(1100 + 5000)), 0);
    assert!(dpad_directions("sideways").is_err());
}

#[test]
fn the_layers_apply_proxy_then_layer_then_latch() {
    let t = Instant::now();
    let mut layers = Layers::new(dpad_directions("left").unwrap(), Proxy::Touch);
    // As Quest controllers, the grip stands in for the thumbrest: the left stick pushed is d-pad
    // left, and it stays held once the stick and the grip are let go.
    let mut pad = Gamepad { thumb_lx: -30000, ..Default::default() };
    layers.apply(&mut pad, Held { rest: true, ..Held::default() }, t);
    assert_eq!(pad.buttons, button::DPAD_LEFT);
    let mut pad = Gamepad::default();
    layers.apply(&mut pad, Held::default(), t + Duration::from_millis(100));
    assert_eq!(pad.buttons, button::DPAD_LEFT, "latched");
}
