//! The targets delivery against the look by targets it replaced (`look_targets::TargetAim`), and
//! against the Chrome Engine's own head aim, which that was lifted from.

use super::tests::{FIELDS, HEAD_ONLY, character, head};
use super::*;
use crate::look::tests::serial;
use crate::look::{Carried, Delivery, baked};
use monaka_core::aim::BakedYaw;
use monaka_core::camera::head_yaw_pitch;
use monaka_producer::game::{Adapter, Game};

/// The look targets' steering as it was (`aim::LookSteer`).
struct Steer {
    applied: HeadAim,
}

impl Steer {
    fn share(&self, character: usize) -> f32 {
        if character == self.applied.owner() { self.applied.yaw_share_radians(&FIELDS.convention) } else { 0.0 }
    }

    fn steer(&mut self, character: usize, yaw: f32, pitch: f32) -> Option<(Look, Look)> {
        if !yaw.is_finite() || !pitch.is_finite() {
            return None;
        }
        let stored = FIELDS.read(character)?;
        let (write, own) = self.applied.apply(character, stored, yaw, pitch, PitchMode::HeadOnly, &FIELDS.convention)?;
        FIELDS.write(character, write);
        Some((write, own))
    }

    fn release(&mut self, character: usize) -> bool {
        if !self.applied.holds_share() {
            return false;
        }
        let Some(stored) = FIELDS.read(character) else { return false };
        match self.applied.release(character, stored) {
            Some(own) => {
                FIELDS.write(character, own);
                true
            }
            None => false,
        }
    }
}

/// `TargetAim` as it was: its own steering, settings, release, tick and baked yaws.
struct Before {
    steer: Steer,
    settings: AimSettings,
    releasing: bool,
    tick: u64,
    updates: u64,
    baked: BakedYaw,
    latest_baked: f32,
}

impl Before {
    fn new() -> Self {
        Self { steer: Steer { applied: HeadAim::NONE }, settings: HEAD_ONLY, releasing: false, tick: 0, updates: 0, baked: BakedYaw::EMPTY, latest_baked: 0.0 }
    }

    fn live(&self) -> bool {
        monaka_channel::tick().saturating_sub(self.tick) < 500
    }

    fn steer(&mut self, character: usize, head: Option<HeadPose>, counter: Option<u64>, stand_down: StandDown, aims_itself: bool) -> Option<(Look, Look, f32, u64, bool)> {
        let settings = self.settings;
        let baked = self.steer.share(character);
        self.latest_baked = baked;
        if let Some(counter) = counter {
            self.baked.record(counter, baked);
        }
        let head = head.filter(|h| h.valid != 0 && settings.head_aim && !self.releasing && !stand_down.now);
        if let Some(head) = head {
            let mut aim = settings.aim_at(monaka_channel::tick(), head.orientation, crate::controllers::palm);
            if settings.hand_rig && aims_itself {
                (aim.yaw, aim.pitch) = head_yaw_pitch(head.orientation);
                aim.by_hand = false;
                aim.tool = false;
            }
            if let Some((written, own)) = self.steer.steer(character, aim.yaw, aim.pitch) {
                let update = self.updates;
                self.updates += 1;
                self.tick = monaka_channel::tick();
                return Some((written, own, baked, update, aim.by_hand));
            }
        }
        self.steer.release(character);
        None
    }

    fn baked_at(&self, counter: Option<u64>) -> f32 {
        match counter {
            Some(counter) => self.baked.lookup(counter).0,
            None => self.latest_baked,
        }
    }

    fn baked_in_view(&self, counter: Option<u64>) -> f32 {
        if self.live() { self.baked_at(counter) } else { 0.0 }
    }

    fn current_share(&self) -> Option<f32> {
        (self.live() && self.steer.applied.holds_share()).then(|| self.steer.share(self.steer.applied.owner()))
    }
}

/// The Chrome Engine's head aim as it was, its own state: the steering, the baked yaws, the release.
struct Chrome {
    steer: Steer,
    baked: BakedYaw,
}

impl Chrome {
    fn steer(&mut self, settings: AimSettings, character: usize, head: Option<HeadPose>, counter: Option<u64>, cutscene: bool, head_aims: bool) -> Option<(Look, Look, f32)> {
        let baked = self.steer.share(character);
        if let Some(counter) = counter {
            self.baked.record(counter, baked);
        }
        let head = head.filter(|h| h.valid != 0 && settings.head_aim && !cutscene);
        if let Some(head) = head {
            let mut aim = settings.aim_at(monaka_channel::tick(), head.orientation, crate::controllers::palm);
            if settings.hand_rig && head_aims {
                (aim.yaw, aim.pitch) = head_yaw_pitch(head.orientation);
                aim.by_hand = false;
            }
            if let Some((written, own)) = self.steer.steer(character, aim.yaw, aim.pitch) {
                return Some((written, own, baked));
            }
        }
        self.steer.release(character);
        None
    }
}

/// A game building its camera from the targets as the update began (DL2).
struct AtUpdate;
/// A game building its camera after the update steers (The Beast).
struct Applied;

impl Game for AtUpdate {
    const MANIFESTS: &'static [monaka_core::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_core::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}

impl Looks for AtUpdate {
    const DELIVERY: Delivery<Self> = Delivery::Targets { carried: Carried::AtUpdate };
}

impl Game for Applied {
    const MANIFESTS: &'static [monaka_core::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_core::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}

impl Looks for Applied {
    const DELIVERY: Delivery<Self> = Delivery::Targets { carried: Carried::Applied };
}

#[test]
fn each_update_steers_and_bakes_as_before() {
    let _serial = serial();
    let (body_now, body_before, body_chrome) = (character(-35.0, 4.0), character(-35.0, 4.0), character(-35.0, 4.0));
    let (now_at, before_at, chrome_at) = (body_now.as_ptr() as usize, body_before.as_ptr() as usize, body_chrome.as_ptr() as usize);
    let mut before = Before::new();
    let mut chrome = Chrome { steer: Steer { applied: HeadAim::NONE }, baked: BakedYaw::EMPTY };
    let rig = AimSettings { hand_rig: true, ..HEAD_ONLY };
    RELEASING.store(false, Relaxed);
    *STEERING.locked() = Steering { fields: FIELDS, applied: HeadAim::NONE, written: None, left: None, taken: None };
    // Updates: (head yaw and pitch, or no head; the game's frame; a cutscene has the player; the
    // settings; what is in hand aims itself; the stop has begun).
    let updates = [
        (Some((20.0, 10.0)), Some(1), false, HEAD_ONLY, false, false),
        (Some((25.0, -5.0)), Some(2), false, HEAD_ONLY, false, false),
        (Some((25.0, -5.0)), Some(3), true, HEAD_ONLY, false, false),
        (Some((-60.0, 30.0)), Some(4), false, HEAD_ONLY, false, false),
        (Some((-60.0, 30.0)), None, false, rig, true, false),
        (Some((170.0, -80.0)), Some(6), false, rig, false, false),
        (None, Some(7), false, rig, false, false),
        (Some((-170.0, 15.0)), Some(8), false, rig, false, false),
        (Some((-170.0, 15.0)), Some(9), false, rig, false, true),
        (Some((10.0, 0.0)), None, false, rig, false, true),
    ];
    let mut cutscene_before = false;
    for (i, (pose, counter, cutscene, settings, aims_itself, releasing)) in updates.into_iter().enumerate() {
        crate::look::configure(settings);
        before.settings = settings;
        RELEASING.store(releasing, Relaxed);
        before.releasing = releasing;
        let stand_down = StandDown { now: cutscene, changed: cutscene != cutscene_before };
        cutscene_before = cutscene;
        let head = pose.map(|(yaw, pitch)| head(yaw, pitch));
        let a = steer(FIELDS, now_at, head, counter, stand_down, || aims_itself).map(|s| (s.written, s.own, s.baked, s.update, s.by_hand));
        let b = before.steer(before_at, head, counter, stand_down, aims_itself);
        // The update numbers count this run's steering from the first.
        assert_eq!(a.map(|s| (s.0, s.1, s.2.to_bits(), s.4)), b.map(|s| (s.0, s.1, s.2.to_bits(), s.4)), "update {i}");
        assert_eq!(body_now[..2], body_before[..2], "the targets written at update {i}");
        if !releasing {
            let c = chrome.steer(settings, chrome_at, head, counter, cutscene, aims_itself);
            assert_eq!(a.map(|s| (s.0, s.1, s.2.to_bits())), c.map(|s| (s.0, s.1, s.2.to_bits())), "update {i} as Chrome's");
            assert_eq!(body_now[..2], body_chrome[..2]);
        }
        // What the eyes and the arms read of it.
        for frame in [counter, Some(2), Some(40), None] {
            assert_eq!(baked_at(frame).to_bits(), before.baked_at(frame).to_bits(), "baked at {frame:?} after update {i}");
            let at_update = baked::<AtUpdate>(frame).map_or(0.0, |share| share.yaw);
            assert_eq!(at_update.to_bits(), before.baked_in_view(frame).to_bits(), "the eyes' share of frame {frame:?} after update {i}");
        }
        assert_eq!(baked::<Applied>(None).map(|share| share.yaw.to_bits()), before.current_share().map(f32::to_bits), "the share applied after update {i}");
        assert_eq!(current_share().map(f32::to_bits), before.current_share().map(f32::to_bits));
        assert_eq!(STEERING.locked().share(now_at).to_bits(), before.steer.share(before_at).to_bits());
    }
    RELEASING.store(false, Relaxed);
}
