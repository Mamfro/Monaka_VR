use super::*;

#[test]
fn the_share_is_swapped_not_accumulated() {
    let head = Share { yaw: 30f32.to_radians(), pitch: 20f32.to_radians() };
    // The game's own accumulator (with its unknown offset): yaw 215, pitch 240.
    let (pitch, yaw) = reshare(240.0, 215.0, Share::default(), head);
    assert!((yaw - 245.0).abs() < 1e-3 && (pitch - 220.0).abs() < 1e-3, "head left and up: {yaw} {pitch}");
    // The mouse adds 5 degrees of yaw; the head holds still: only the mouse's turn is new.
    let (pitch, yaw) = reshare(pitch, yaw + 5.0, head, head);
    assert!((yaw - 250.0).abs() < 1e-3 && (pitch - 220.0).abs() < 1e-3);
    // Released: the game's own angles, with the mouse's turn kept.
    let (pitch, yaw) = reshare(pitch, yaw, head, Share::default());
    assert!((yaw - 220.0).abs() < 1e-3 && (pitch - 240.0).abs() < 1e-3);
}

#[test]
fn a_tilt_is_levelled_a_step_at_a_time() {
    assert_eq!(LEVEL.step(15.0), -0.25);
    assert_eq!(LEVEL.step(-15.0), 0.25);
    assert_eq!(LEVEL.step(0.2), -0.2);
    assert_eq!(LEVEL.step(0.01), 0.0, "level enough");
    // A head looking 40 down is past the range: no measure.
    assert!(own_pitch(-40.0, Share { yaw: 0.0, pitch: -40f32.to_radians() }).is_nan());
    // A head 10 up with the camera 25 up: the player's own pitch is 15 up (-15 positive down).
    assert!((own_pitch(25.0, Share { yaw: 0.0, pitch: 10f32.to_radians() }) + 15.0).abs() < 1e-4);
}

#[test]
fn a_pause_holds_the_share_and_another_camera_takes_none() {
    let held = Share { yaw: 0.2, pitch: 0.1 };
    let head = Share { yaw: 0.5, pitch: -0.1 };
    assert_eq!(next_share(true, true, false, held, Some(head)), held, "resuming after a pause");
    assert_eq!(next_share(true, false, false, held, Some(head)), head);
    assert_eq!(next_share(true, false, false, held, None), held, "no head pose: the share stays");
    assert_eq!(next_share(true, false, true, held, Some(head)), Share::default(), "another camera has the view");
    assert_eq!(next_share(false, false, false, held, Some(head)), Share::default(), "handed back");
}

#[test]
fn head_share_reads_yaw_and_pitch() {
    let (y, p) = (30f32.to_radians(), 20f32.to_radians());
    let (sy, cy) = (y * 0.5).sin_cos();
    let (sp, cp) = (p * 0.5).sin_cos();
    let share = head_share([cy * sp, sy * cp, -sy * sp, cy * cp]);
    assert!((share.yaw - y).abs() < 1e-4 && (share.pitch - p).abs() < 1e-4, "{share:?}");
}

#[test]
fn a_camera_that_turns_by_the_angles_keeps_the_share_as_it_was() {
    let _serial = crate::look::tests::serial();
    let held = Share { yaw: 60f32.to_radians(), pitch: 10f32.to_radians() };
    let run = |holds: fn() -> bool| {
        *GAME.locked() = Some(GameFns { camera_taken: || true, head: || Some(HeadPose::synthetic(5.0, 0.0, HeadPose::SYNTHETIC_FOV, 0.064)), holds, pitch: Pitch::Own });
        PAUSE_HOLD.store(false, Relaxed);
        AIMING.store(true, Relaxed);
        *STATE.locked() = State { held, commands: 0, aimed: 0, last_command: None, holding_after_pause: 0, pauses: 0 };
        let angles = built(240.0, 215.0);
        // Left as the other tests expect it: nothing aiming or held.
        AIMING.store(false, Relaxed);
        HOLDING.store(false, Release);
        *GAME.locked() = None;
        (angles, std::mem::take(&mut STATE.locked().held))
    };
    // A mounted gun: the angles stay, so the gun sees no turn from the head.
    assert_eq!(run(|| true), (Some((240.0, 215.0)), held));
    // A cutscene's camera: the share goes out.
    assert_eq!(run(|| false), (Some(reshare(240.0, 215.0, held, Share::default())), Share::default()));
}

#[test]
fn a_menu_leaving_the_camera_alone_keeps_the_share_and_reports_it_baked() {
    let _serial = crate::look::tests::serial();
    let held = Share { yaw: 30f32.to_radians(), pitch: 5f32.to_radians() };
    *GAME.locked() = Some(GameFns { camera_taken: || false, head: || Some(HeadPose::synthetic(5.0, 0.0, HeadPose::SYNTHETIC_FOV, 0.064)), holds: || true, pitch: Pitch::Own });
    PAUSE_HOLD.store(false, Relaxed);
    AIMING.store(true, Relaxed);
    *STATE.locked() = State { held, commands: 0, aimed: 0, last_command: None, holding_after_pause: 0, pauses: 0 };
    // The head has turned elsewhere, but the angles keep the share the camera has.
    let angles = built(240.0, 215.0);
    let baked = baked();
    AIMING.store(false, Relaxed);
    HOLDING.store(false, Release);
    *GAME.locked() = None;
    let kept = std::mem::take(&mut STATE.locked().held);
    assert_eq!((angles, kept), (Some((240.0, 215.0)), held));
    // The eye site turns that share back out of the camera, so the head looks around.
    assert_eq!(baked, Some(held));
}

#[test]
fn head_alone_drops_the_games_own_pitch_and_keeps_its_turns() {
    let _serial = crate::look::tests::serial();
    let held = Share { yaw: 2f32.to_radians(), pitch: 3f32.to_radians() };
    *GAME.locked() = Some(GameFns { camera_taken: || false, head: || Some(HeadPose::synthetic(5.0, 8.0, HeadPose::SYNTHETIC_FOV, 0.064)), holds: || false, pitch: Pitch::HeadAlone });
    PAUSE_HOLD.store(false, Relaxed);
    AIMING.store(true, Relaxed);
    *STATE.locked() = State { held, commands: 0, aimed: 0, last_command: None, holding_after_pause: 0, pauses: 0 };
    // The held share's pitch (3 degrees up: -3 in the command's degrees) and 20 more down from the
    // game's own input since; the yaw with the game's own turning in it.
    let angles = built(17.0, 100.0);
    AIMING.store(false, Relaxed);
    HOLDING.store(false, Release);
    *GAME.locked() = None;
    let next = std::mem::take(&mut STATE.locked().held);
    let head = head_share(HeadPose::synthetic(5.0, 8.0, HeadPose::SYNTHETIC_FOV, 0.064).orientation);
    assert_eq!(next, head);
    // The pitch is the head's alone; the yaw keeps the game's own turn, the share swapped.
    assert_eq!(angles, Some((degrees(head).1, reshare(17.0, 100.0, held, head).1)));
}
