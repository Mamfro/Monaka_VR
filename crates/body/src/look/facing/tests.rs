use super::*;
use crate::BoneAxes;
use crate::hand_rig::{Climbing, Holds, Method, Rig};
use crate::look::tests::serial;
use monaka_producer::game::{Adapter, Game};

/// A game whose character keeps its look targets in plain memory: a target yaw of `t` degrees looks
/// `-t` degrees in the world, pitch is degrees.
pub(super) struct Faced;

pub(super) static TARGETS: Mutex<(usize, f32, f32)> = Mutex::new((0, 0.0, 0.0));
pub(super) static CUTSCENE_NOW: AtomicBool = AtomicBool::new(false);
static WAS_CUTSCENE: AtomicBool = AtomicBool::new(false);
static HEAD_YAW: Mutex<f32> = Mutex::new(0.0);
/// A facing the player asks for at the next update (a saved spot).
pub(super) static SPOT: Mutex<Option<f32>> = Mutex::new(None);
static TRACED: AtomicU64 = AtomicU64::new(0);
static STEERED: AtomicU64 = AtomicU64::new(0);

pub(super) const HEAD_ONLY: AimSettings = AimSettings { head_aim: true, hand_aim: false, aim_hand: 1, hand_rig: false, melee_aim_head: true, tool_aim: false };

pub(super) const UNITS: TargetUnits = TargetUnits { convention: Convention { yaw_per_radian: -57.29578, pitch_per_radian: 57.29578, pitch_limit: 90.0 }, yaw_zero: 0.0, turn: 360.0 };

impl Game for Faced {
    const MANIFESTS: &'static [monaka_contract::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_contract::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}

impl HandRig for Faced {
    const RIG: Rig = Rig { method: Method::Carried { bones: BoneAxes::DYING_LIGHT_1, mirror: None, melee_holder: None }, holds: Holds::TwoHanded, climbing: Climbing::None, fingers: None };
}

pub(super) fn read_targets(_: usize) -> Option<f32> {
    Some(TARGETS.locked().1)
}

pub(super) fn write_targets(character: usize, yaw: f32, pitch: f32) {
    *TARGETS.locked() = (character, yaw, pitch);
}

fn spot(_now: u64, _character: usize, _facing: f32) -> Option<f32> {
    SPOT.locked().take()
}

pub(super) fn cutscene_asked() -> StandDown {
    let now = CUTSCENE_NOW.load(Relaxed);
    StandDown { now, changed: WAS_CUTSCENE.swap(now, Relaxed) != now }
}

impl Looks for Faced {
    const DELIVERY: Delivery<Self> = Delivery::Facing(Facing { units: UNITS, read: read_targets, write: write_targets, command: spot });
    fn stand_down(_: usize) -> StandDown {
        cutscene_asked()
    }
}

/// The head yaw the eyes take out of the latest update's camera.
pub(super) fn baked() -> f32 {
    crate::look::baked::<Faced>(None).map_or(0.0, |share| share.yaw)
}

fn update(camera: f32) -> PlayerUpdate {
    let head = HeadPose::synthetic(*HEAD_YAW.locked(), 0.0, HeadPose::SYNTHETIC_FOV, 0.064);
    PlayerUpdate { character: 0x1000, yaw: TARGETS.locked().1, pitch: 0.0, camera_yaw: camera.to_radians(), head }
}

#[test]
fn the_view_faces_the_camera_then_turns_with_the_head_alone() {
    let _serial = serial();
    *STATE.locked() = State::new();
    *TARGETS.locked() = (0, 0.0, 0.0);
    CUTSCENE_NOW.store(false, Relaxed);
    WAS_CUTSCENE.store(false, Relaxed);
    configure(HEAD_ONLY, 120.0);
    observe(Observer { traced: |_| _ = TRACED.fetch_add(1, Relaxed), steered: |_| _ = STEERED.fetch_add(1, Relaxed) });
    let (traced, steered) = (TRACED.load(Relaxed), STEERED.load(Relaxed));
    *HEAD_YAW.locked() = 0.0;
    // The first update: the facing is where the game camera looks.
    steer::<Faced>(update(40.0));
    assert!(share_in_view() && live() && !cutscene());
    let (character, yaw, _) = *TARGETS.locked();
    assert_eq!(character, 0x1000);
    assert!((yaw + 40.0).abs() < 1e-3, "the character looks where the camera did: {yaw}");
    // The head turns 30 degrees left: the character follows; the game camera, built from the
    // targets before, carries nothing of the head yet.
    *HEAD_YAW.locked() = 30.0;
    steer::<Faced>(update(40.0));
    assert!((TARGETS.locked().1 + 70.0).abs() < 1e-3);
    assert!(baked().abs() < 1e-6);
    // Its camera now carries the head's 30 degrees, which the view turns back out.
    steer::<Faced>(update(70.0));
    assert!((baked().to_degrees() - 30.0).abs() < 1e-3);
    // A cutscene takes the camera: nothing is written, the view is its camera plus the head.
    CUTSCENE_NOW.store(true, Relaxed);
    let before = *TARGETS.locked();
    steer::<Faced>(update(-120.0));
    assert!(cutscene() && !live());
    assert_eq!(*TARGETS.locked(), before);
    assert!((baked().to_degrees() - 30.0).abs() < 1e-3, "the head's yaw as the cutscene began");
    // It lets go: the facing goes on from its last framing.
    CUTSCENE_NOW.store(false, Relaxed);
    steer::<Faced>(update(-100.0));
    assert!((TARGETS.locked().1 - 100.0).abs() < 1e-3, "-100 - 30 + 30 head: {:?}", *TARGETS.locked());
    // The observer saw every update, and each steering (not the cutscene's).
    assert_eq!((TRACED.load(Relaxed) - traced, STEERED.load(Relaxed) - steered), (5, 4));
    // The end of the run: the facing, level, without the head.
    release::<Faced>();
    let (_, yaw, pitch) = *TARGETS.locked();
    assert!((yaw - 130.0).abs() < 1e-3 && pitch == 0.0, "{yaw} {pitch}");
}

#[test]
fn the_head_pushes_the_stick_by_how_far_apart_it_is() {
    assert_eq!(push_for(0.04), 0.0);
    assert_eq!(push_for(-PUSH_FULL), PUSH_DEAD_ZONE + (1.0 - PUSH_DEAD_ZONE));
    let near = push_for(0.06);
    assert!(near < -PUSH_DEAD_ZONE + 1e-6 && near > -PUSH_DEAD_ZONE - 0.02, "{near}");
}

#[test]
fn target_yaws_stay_nearest_the_characters() {
    // A look a turn and a bit away lands within half a turn of the character's own yaw.
    let target = UNITS.yaw(700.0, 0.1);
    assert!((target - 700.0).abs() <= 180.0 && ((target - (-0.1f32.to_degrees())).rem_euclid(360.0)).abs() < 1e-3, "{target}");
    assert_eq!(UNITS.pitch(3.0), 90.0, "within the limit");
}
