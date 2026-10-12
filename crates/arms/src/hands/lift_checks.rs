//! The hands module against the per-feature hand items it replaced: each feature reads the same
//! point, at the same moment, in the same space as before.

use super::*;
use crate::BoneAxes;
use crate::hand_rig::{self, Climbing, Hands, Holds, Method, Moment, Palms, Rig};
use crate::hand_trail::{HandTrails, Lost, TrailRules};
use monaka_core::camera::IDENTITY_MATRIX;
use monaka_core::protocol::{HeadPose, LEFT_HAND, RIGHT_HAND};
use monaka_producer::game::{Adapter, Game};

const RULES: TrailRules = TrailRules { kept: 0.8, span: 0.06, lost: Lost::Kept };
static NOTED: HandTrails = HandTrails::new(RULES);
static SILENT: HandTrails = HandTrails::new(RULES);

/// A game whose rig notes its trail (as DL2 and The Beast's).
struct Noted;
/// A game that notes its trail itself (as Dying Light 1).
struct Silent;

impl Game for Noted {
    const MANIFESTS: &'static [monaka_core::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_core::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}

impl Game for Silent {
    const MANIFESTS: &'static [monaka_core::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_core::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}

const RIG: Rig = Rig { method: Method::Carried { bones: BoneAxes::DYING_LIGHT_1, mirror: None, melee_holder: None }, holds: Holds::TwoHanded, climbing: Climbing::None, fingers: None };

impl HandRig for Noted {
    const RIG: Rig = RIG;
    fn trail() -> Option<&'static HandTrails> {
        Some(&NOTED)
    }
}

impl HandRig for Silent {
    const RIG: Rig = RIG;
    const NOTES_TRAIL: bool = false;
    fn trail() -> Option<&'static HandTrails> {
        Some(&SILENT)
    }
}

fn pose(x: f32, valid: u32) -> HandPose {
    HandPose { orientation: [0.1 * x, 0.2, -0.3, 0.9], position: [x, 1.3 - x, -0.4 * x], valid, received_tick: 0 }
}

/// The frame sample: Dying Light 1's `pending_hand_of` (the aim pose read at the present) and
/// `pending_palm_of` (the palm made from it and the grip), and its look's choice between them.
#[test]
fn the_frame_sample_is_dying_light_1s_pending_hands() {
    let cases = [
        ([Some(pose(0.3, 1)), Some(pose(-0.2, 1))], [Some(pose(0.5, 1)), None]),
        ([None, Some(pose(0.7, 0))], [Some(pose(0.1, 1)), Some(pose(0.9, 0))]),
        ([None, None], [None, None]),
    ];
    for (aims, grips) in cases {
        *FRAME.locked() = Frame { aims, grips };
        for side in [LEFT_HAND, RIGHT_HAND] {
            let (old_hand, old_palm) = (aims[side], HandPose::palm(aims[side], grips[side]));
            assert_eq!(super::pose::<Noted>(side, Point::Controller, At::Frame, In::Tracking), old_hand);
            assert_eq!(super::pose::<Noted>(side, Point::Palm, At::Frame, In::Tracking), old_palm);
            for rig in [true, false] {
                let old = if rig { old_palm } else { old_hand };
                let point = if rig { Point::Palm } else { Point::Controller };
                assert_eq!(super::pose::<Noted>(side, point, At::Frame, In::Tracking), old, "the look's hand, rig {rig}");
            }
        }
    }
}

/// At the latest: the lockpick's default palm (`controllers::palm`, tracked only).
#[test]
fn the_latest_palm_is_the_lockpicks_default() {
    for (aim, grip) in [(Some(pose(0.3, 1)), Some(pose(0.5, 1))), (Some(pose(0.3, 0)), None), (None, Some(pose(0.2, 1))), (None, None)] {
        let old = HandPose::palm(aim, grip).filter(|p| p.valid != 0);
        assert_eq!(palm(At::Latest, aim, grip), old);
    }
}

/// In the world: the throw's default hand and the bow's default string hand were the palm noted
/// on the rig's trail within 0.1 s, as its matrix's position.
#[test]
fn the_world_palm_is_the_throws_and_bows_default() {
    let origin = [0.9, -0.1, 0.3, 4.5, 0.2, 0.95, -0.1, 1.25, -0.3, 0.1, 0.9, -7.75];
    NOTED.note(&origin, &[Some(pose(0.31, 1)), Some(pose(-0.17, 1))]);
    for side in [LEFT_HAND, RIGHT_HAND] {
        let old = NOTED.palm(side, 0.1).map(|palm| [palm[3], palm[7], palm[11]]);
        assert!(old.is_some());
        assert_eq!(hand_rig::posed_hand::<Noted>(side).map(|p| p.map(f32::to_bits)), old.map(|p| p.map(f32::to_bits)), "the throw's hand");
    }
    let old = NOTED.palm(RIGHT_HAND, 0.1).map(|palm| [palm[3], palm[7], palm[11]]);
    let new = <Noted as HandRig>::string_point();
    assert_eq!(new, old, "the bow's string hand");
    assert_eq!(super::pose::<Noted>(LEFT_HAND, Point::Controller, At::Latest, In::World), None);
}

/// The rig noted its trail at each moment; a game that notes its own (Dying Light 1, whose rig had
/// no trail) is not noted twice.
#[test]
fn the_rig_notes_only_a_trail_it_owns() {
    let palms = Palms { game: IDENTITY_MATRIX, origin: IDENTITY_MATRIX, head: HeadPose::default(), palms: [Some(pose(0.2, 1)), Some(pose(0.4, 1))], aim_side: RIGHT_HAND, hold: [0.0; 3], bones: None };
    let moment = Moment { hands: Hands::Palms(palms), fingers: false };
    let _ = hand_rig::capture::<Silent>(moment);
    assert_eq!(SILENT.held(), [0, 0]);
    let before = NOTED.held();
    let _ = hand_rig::capture::<Noted>(moment);
    assert!(NOTED.held().iter().zip(before).all(|(now, was)| *now >= was.min(1)) && NOTED.palm(LEFT_HAND, 0.1).is_some());
}
