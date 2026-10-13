use super::*;
use crate::look::tests::serial;

/// The look targets at 0 (yaw) and 4 (pitch) in a character, degrees with the yaw turning right.
pub(super) const FIELDS: LookFields = LookFields { yaw: 0, pitch: 4, convention: Convention::DEGREES_YAW_RIGHT };

/// A character of plain memory: the look targets at 0 (yaw) and 4 (pitch), degrees.
pub(super) fn character(yaw: f32, pitch: f32) -> Box<[f32; 4]> {
    Box::new([yaw, pitch, 0.0, 0.0])
}

pub(super) fn head(yaw: f32, pitch: f32) -> HeadPose {
    HeadPose::synthetic(yaw, pitch, HeadPose::SYNTHETIC_FOV, 0.064)
}

pub(super) const HEAD_ONLY: AimSettings = AimSettings { head_aim: true, hand_aim: false, aim_hand: 1, hand_rig: false, melee_aim_head: true, tool_aim: false };

#[test]
fn the_head_steers_and_its_share_goes_back() {
    let _serial = serial();
    crate::look::configure(HEAD_ONLY);
    RELEASING.store(false, Relaxed);
    let body = character(10.0, 0.0);
    let at = body.as_ptr() as usize;
    let steered = steer(FIELDS, at, Some(head(30.0, 0.0)), Some(1), StandDown::default(), || false).unwrap();
    assert_eq!(steered.baked, 0.0, "the first update's camera carries nothing yet");
    assert!(crate::look::live());
    assert!(STEERING.locked().share(at) != 0.0, "the head's yaw is in the targets");
    assert_eq!(baked_at(Some(1)), 0.0);
    let next = steer(FIELDS, at, Some(head(30.0, 0.0)), Some(2), StandDown::default(), || false).unwrap();
    assert!((baked_at(Some(2)) - next.baked).abs() < 1e-6 && next.baked != 0.0, "the next camera carries the share");
    // Another camera takes the player: the share goes back, nothing is steered.
    assert!(steer(FIELDS, at, Some(head(30.0, 0.0)), Some(3), StandDown { now: true, changed: true }, || false).is_none());
    assert_eq!(STEERING.locked().share(at), 0.0);
    assert_eq!(body[0], 10.0, "the character's own yaw again");
}

/// Writes a still look of the game's own over the character, as a scripted moment does each frame.
fn hold(body: &mut [f32; 4]) {
    body[0] = -180.0;
    body[1] = 0.0;
}

#[test]
fn a_look_the_game_holds_is_left_to_the_game() {
    let _serial = serial();
    crate::look::configure(HEAD_ONLY);
    RELEASING.store(false, Relaxed);
    let mut body = character(10.0, 0.0);
    let at = body.as_ptr() as usize;
    assert!(steer(FIELDS, at, Some(head(30.0, 10.0)), Some(1), StandDown::default(), || false).is_some());
    // Far from what head aim wrote: once could be a nudge, so head aim steers on; the same look left
    // again is the game's own.
    hold(&mut body);
    assert!(steer(FIELDS, at, Some(head(30.0, 10.0)), Some(2), StandDown::default(), || false).is_some(), "once");
    hold(&mut body);
    assert!(steer(FIELDS, at, Some(head(30.0, 10.0)), Some(3), StandDown::default(), || false).is_none(), "standing down");
    assert_eq!(STEERING.locked().share(at), 0.0, "none of the head's share is in the game's look");
    assert_eq!(body[0], -180.0, "the game's yaw left as it is");
    assert!(body[1].abs() <= PROBE_PITCH, "only a probe too small to see on the pitch");
    // While it keeps writing the same still look over each probe, head aim keeps off.
    for update in 4..40 {
        hold(&mut body);
        assert!(steer(FIELDS, at, Some(head(30.0, 10.0)), Some(update), StandDown::default(), || false).is_none(), "update {update}");
        assert_eq!(body[0], -180.0);
    }
}

#[test]
fn head_aim_steers_again_once_the_game_lets_go() {
    let _serial = serial();
    crate::look::configure(HEAD_ONLY);
    RELEASING.store(false, Relaxed);
    let mut body = character(10.0, 0.0);
    let at = body.as_ptr() as usize;
    assert!(steer(FIELDS, at, Some(head(30.0, 10.0)), Some(1), StandDown::default(), || false).is_some());
    for update in 2..=3 {
        hold(&mut body);
        steer(FIELDS, at, Some(head(30.0, 10.0)), Some(update), StandDown::default(), || false);
    }
    assert!(STEERING.locked().taken.is_some(), "standing down");
    // The game stops writing: its probes stay, and after a few head aim steers again.
    let mut update = 4;
    while steer(FIELDS, at, Some(head(30.0, 10.0)), Some(update), StandDown::default(), || false).is_none() {
        update += 1;
        assert!(update < 4 + 2 * PROBES_KEPT as u64, "still standing down at update {update}");
    }
    assert!(body[0] != -180.0, "steering from the game's last look: {body:?}");
}

#[test]
fn aim_assist_moving_the_look_on_is_not_the_game_holding_it() {
    let _serial = serial();
    crate::look::configure(HEAD_ONLY);
    RELEASING.store(false, Relaxed);
    let mut body = character(10.0, 0.0);
    let at = body.as_ptr() as usize;
    let mut steered = steer(FIELDS, at, Some(head(30.0, 10.0)), Some(1), StandDown::default(), || false).unwrap();
    // Each update of an attack the game turns the look on from the one written toward its target,
    // yaw and pitch (and a fast stick turn moves the yaw as far).
    for update in 2..40 {
        body[0] = steered.written.yaw + 6.0 + update as f32 * 0.1;
        body[1] = steered.written.pitch - 1.5;
        steered = steer(FIELDS, at, Some(head(30.0, 10.0)), Some(update), StandDown::default(), || false).expect("head aim keeps steering");
    }
    assert!(STEERING.locked().share(at) != 0.0, "the head's share kept: no jump");
}

#[test]
fn turning_by_the_stick_is_not_the_game_writing() {
    let _serial = serial();
    crate::look::configure(HEAD_ONLY);
    RELEASING.store(false, Relaxed);
    let mut body = character(10.0, 0.0);
    let at = body.as_ptr() as usize;
    let first = steer(FIELDS, at, Some(head(30.0, 0.0)), Some(1), StandDown::default(), || false).unwrap();
    // The player's stick turns the yaw a little between updates; the pitch stays (held neutral).
    body[0] = first.written.yaw + 2.0;
    assert!(steer(FIELDS, at, Some(head(30.0, 0.0)), Some(2), StandDown::default(), || false).is_some());
    // Across the half turn, the yaw wraps: still a small turn.
    let at_wrap = steer(FIELDS, at, Some(head(30.0, 0.0)), Some(3), StandDown::default(), || false).unwrap();
    body[0] = at_wrap.written.yaw - 358.0;
    assert!(steer(FIELDS, at, Some(head(30.0, 0.0)), Some(4), StandDown::default(), || false).is_some());
}
