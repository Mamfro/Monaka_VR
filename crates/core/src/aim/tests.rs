use super::*;
use core::f32::consts::FRAC_PI_6;

#[test]
fn levelling_steps_and_gates_toward_level() {
    let rule = Levelling { dead_band: 0.05, step: 0.25 };
    assert_eq!((rule.step(15.0), rule.step(-15.0), rule.step(0.2), rule.step(0.01)), (-0.25, 0.25, -0.2, 0.0));
    assert!(rule.toward_level(-3.0, true) && !rule.toward_level(-3.0, false));
    assert!(rule.toward_level(3.0, false) && !rule.toward_level(3.0, true));
    assert!(!rule.toward_level(0.04, false) && !rule.toward_level(-0.04, true), "level enough: nothing passes");
}

const DL2: Convention = Convention::DEGREES_YAW_RIGHT;

#[test]
fn head_only_pitch_and_added_yaw() {
    let mut aim = HeadAim::default();
    // Mouse at yaw 100, pitch -2.2; head 30 degrees left and 20 down.
    let (write, own) = aim.apply(1, Look { yaw: 100.0, pitch: -2.2 }, FRAC_PI_6, -0.3491, PitchMode::HeadOnly, &DL2).unwrap();
    assert!((write.pitch + 20.0).abs() < 0.01 && (write.yaw - 70.0).abs() < 0.01, "{write:?}");
    assert_eq!(own, Look { yaw: 100.0, pitch: -2.2 });
    // Next update the game added 5 degrees of mouse yaw to what we wrote: it is kept.
    let (write, own) = aim.apply(1, Look { yaw: write.yaw + 5.0, pitch: write.pitch }, FRAC_PI_6, -0.3491, PitchMode::HeadOnly, &DL2).unwrap();
    assert!((own.yaw - 105.0).abs() < 0.01 && (write.yaw - 75.0).abs() < 0.01);
    assert!((aim.yaw_share_radians(&DL2) - FRAC_PI_6).abs() < 1e-4);
    let back = aim.release(1, write).unwrap();
    assert!((back.yaw - 105.0).abs() < 0.01 && (back.pitch + 2.2).abs() < 0.01);
    assert!(aim.release(1, write).is_none(), "released once");
}

#[test]
fn a_head_turning_past_the_half_turn_keeps_the_yaw_near() {
    let mut aim = HeadAim::default();
    let (first, _) = aim.apply(1, Look { yaw: 0.0, pitch: 0.0 }, 179f32.to_radians(), 0.0, PitchMode::HeadOnly, &DL2).unwrap();
    // The head goes on 2 degrees: its yaw reads -179.
    let (next, _) = aim.apply(1, first, (-179f32).to_radians(), 0.0, PitchMode::HeadOnly, &DL2).unwrap();
    assert!((next.yaw - first.yaw + 2.0).abs() < 0.01, "{first:?} then {next:?}");
    // And back.
    let (back, own) = aim.apply(1, next, 179f32.to_radians(), 0.0, PitchMode::HeadOnly, &DL2).unwrap();
    assert!((back.yaw - first.yaw).abs() < 0.01 && own.yaw.abs() < 0.01, "{back:?} {own:?}");
}

#[test]
fn added_pitch_is_clamped_and_new_owners_start_clean() {
    let mut aim = HeadAim::default();
    let (write, _) = aim.apply(1, Look { yaw: 0.0, pitch: 70.0 }, 0.0, 0.5, PitchMode::HeadPlusOwn, &DL2).unwrap();
    assert_eq!(write.pitch, 85.0);
    let (_, own) = aim.apply(2, Look { yaw: 10.0, pitch: 85.0 }, 0.0, 0.0, PitchMode::HeadPlusOwn, &DL2).unwrap();
    assert_eq!(own, Look { yaw: 10.0, pitch: 85.0 }, "another character's values are taken as its own");
    assert!(aim.apply(2, Look { yaw: f32::NAN, pitch: 0.0 }, 0.0, 0.0, PitchMode::HeadOnly, &DL2).is_none());
    assert!(!aim.holds_share());
}

#[test]
fn baked_yaw_by_frame() {
    let mut baked = BakedYaw::default();
    for frame in 1..=10 {
        baked.record(frame, frame as f32);
    }
    assert_eq!(baked.lookup(7), (7.0, true));
    assert_eq!(baked.lookup(1), (10.0, false), "evicted frames fall back to the latest");
}
