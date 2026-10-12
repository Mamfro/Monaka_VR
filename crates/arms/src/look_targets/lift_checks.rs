//! Head aim by look targets against the Chrome Engine's own head aim, which it was lifted from.

use super::tests::{HEAD_ONLY, character, head};
use super::*;

/// The Chrome Engine's head aim as it was, its own state: the steering, the baked yaws, the release.
struct Before {
    steer: LookSteer,
    baked: BakedYaw,
    releasing: bool,
    updates: u64,
}

impl Before {
    fn new() -> Self {
        Self { steer: LookSteer::new(LookFields { yaw: 0, pitch: 4, convention: Convention::DEGREES_YAW_RIGHT }), baked: BakedYaw::EMPTY, releasing: false, updates: 0 }
    }

    /// Its update: the share recorded, the head (or nothing while a cutscene has the player or it
    /// releases) steered, the share handed back otherwise.
    fn steer(&mut self, settings: AimSettings, character: usize, head: Option<HeadPose>, counter: Option<u64>, cutscene: bool, head_aims: bool) -> Option<(Look, Look, f32)> {
        let baked = self.steer.share(character);
        if let Some(counter) = counter {
            self.baked.record(counter, baked);
        }
        let head = head.filter(|h| h.valid != 0 && settings.head_aim && !self.releasing && !cutscene);
        if let Some(head) = head {
            let mut aim = settings.aim_at(monaka_channel::tick(), head.orientation, crate::controllers::palm);
            if settings.hand_rig && head_aims {
                (aim.yaw, aim.pitch) = head_yaw_pitch(head.orientation);
                aim.by_hand = false;
            }
            if let Some((written, own)) = self.steer.steer(character, aim.yaw, aim.pitch) {
                self.updates += 1;
                return Some((written, own, baked));
            }
        }
        self.steer.release(character);
        None
    }
}

#[test]
fn each_update_steers_as_chromes_did() {
    let (body_now, body_before) = (character(-35.0, 4.0), character(-35.0, 4.0));
    let (now_at, before_at) = (body_now.as_ptr() as usize, body_before.as_ptr() as usize);
    let now = TargetAim::new(0, 4);
    let mut before = Before::new();
    let rig = AimSettings { hand_rig: true, ..HEAD_ONLY };
    // Updates: (head yaw, pitch, a cutscene has the player, the settings, what is in hand aims itself).
    let updates = [
        (20.0, 10.0, false, HEAD_ONLY, false),
        (25.0, -5.0, false, HEAD_ONLY, false),
        (25.0, -5.0, true, HEAD_ONLY, false),
        (-60.0, 30.0, false, HEAD_ONLY, false),
        (-60.0, 30.0, false, rig, true),
        (170.0, -80.0, false, rig, false),
    ];
    let mut cutscene_before = false;
    for (i, (yaw, pitch, cutscene, settings, aims_itself)) in updates.into_iter().enumerate() {
        now.configure(settings);
        let counter = Some(i as u64);
        let stand_down = StandDown { now: cutscene, changed: cutscene != cutscene_before };
        cutscene_before = cutscene;
        let a = now.steer(now_at, Some(head(yaw, pitch)), counter, stand_down, || aims_itself).map(|s| (s.written, s.own, s.baked));
        let b = before.steer(settings, before_at, Some(head(yaw, pitch)), counter, cutscene, aims_itself);
        assert_eq!(a, b, "update {i}");
        assert_eq!(body_now[..2], body_before[..2], "the targets written at update {i}");
        assert_eq!(now.baked_at(counter), before.baked.lookup(i as u64).0);
    }
}
