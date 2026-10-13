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
