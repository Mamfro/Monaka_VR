use super::*;
use monaka_contract::gamepad::button;

#[test]
fn turning_has_a_deadzone_and_eases_in() {
    let turn = StickTurn::smooth(150.0);
    assert_eq!(turn.share(0.1), 0.0);
    assert_eq!(turn.share(-0.15), 0.0);
    assert!((turn.share(1.0) - 1.0).abs() < 1e-6 && (turn.share(-1.0) + 1.0).abs() < 1e-6);
    let half = turn.share(0.575);
    assert!(half > 0.3 && half < 0.4, "eased: {half}");
}

#[test]
fn a_snap_turns_once_per_push_and_a_straight_smooth_turn_is_linear() {
    // As eng_unr's first person turns: a straight smooth turn past 0.2, and a 30 degree snap.
    let straight = StickTurn::Smooth { speed: 120.0, dead_zone: 0.2, ease: 1.0 };
    let mut latch = TurnLatch::new();
    assert_eq!(straight.turn(0.15, 0.1, &mut latch), 0.0);
    assert!((straight.turn(0.6, 0.1, &mut latch) - 0.5 * 120.0 * 0.1).abs() < 1e-5);
    assert!((straight.turn(-1.0, 0.1, &mut latch) + 12.0).abs() < 1e-5);
    let snap = StickTurn::SNAP;
    let pushes = [0.0, 0.8, 0.9, 0.5, 0.8, 0.2, -0.75, -0.8, 0.0, 0.71];
    let turned: Vec<f32> = pushes.iter().map(|&x| snap.turn(x, 0.011, &mut latch)).collect();
    assert_eq!(turned, [0.0, 30.0, 0.0, 0.0, 0.0, 0.0, -30.0, 0.0, 0.0, 30.0], "back within 0.3 before the next");
    assert!(snap.pushed(0.75) && !snap.pushed(0.6) && snap.share(1.0) == 0.0);
}

#[test]
fn the_turn_option_picks_smooth_or_snap() {
    let manifest = Manifest { options: &[monaka_contract::manifest::TURN], ..Manifest::GAME };
    let pick = |text: &str| StickTurn::from_options(&Options::parse(text), &manifest, StickTurn::smooth(150.0), StickTurn::SNAP);
    assert_eq!(pick(""), StickTurn::smooth(150.0));
    assert_eq!(pick("turn=snap"), StickTurn::SNAP);
    assert_eq!(pick("turn=Smooth"), StickTurn::smooth(150.0));
}

#[test]
fn axes_convert_with_a_deadzone() {
    assert_eq!(stick(0.1), 0, "drift stays centred");
    assert_eq!(stick(1.0), 32767);
    assert_eq!(stick(-1.0), -32767);
    assert!(stick(0.56) > 16000 && stick(0.56) < 16800, "half way past the deadzone is about half");
    assert_eq!(stick(f32::NAN), 0);
    assert_eq!((trigger(0.0), trigger(1.0), trigger(2.0), trigger(-1.0)), (0, 255, 255, 0));
}

#[test]
fn dither_moves_only_held_sticks() {
    let pad = Gamepad { buttons: button::A, thumb_ly: 32767, thumb_rx: -20000, ..Gamepad::default() };
    assert_eq!(dither(pad, pad, false), pad);
    let odd = dither(pad, pad, true);
    assert_eq!((odd.thumb_ly, odd.thumb_rx, odd.thumb_lx, odd.buttons), (32766, -19999, 0, button::A));
    let drift = Gamepad { thumb_lx: 817, ..pad };
    assert_eq!(dither(drift, pad, true).thumb_lx, 817, "a real pad's drift is not jiggled");
}

#[test]
fn pads_merge() {
    let real = Gamepad { buttons: button::A, left_trigger: 10, thumb_lx: 30000, ..Gamepad::default() };
    let vr = Gamepad { buttons: button::B, left_trigger: 200, thumb_lx: 10000, thumb_ry: -500, ..Gamepad::default() };
    let both = merge(real, vr);
    assert_eq!(both.buttons, button::A | button::B);
    assert_eq!((both.left_trigger, both.thumb_lx, both.thumb_ry), (200, i16::MAX, -500));
    assert_eq!(merge(Gamepad::default(), vr), vr);
}
