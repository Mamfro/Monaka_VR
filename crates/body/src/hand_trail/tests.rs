use super::*;
use monaka_core::camera::from_back_up;
use std::time::Duration;

/// A palm at `x` metres along tracking x, level.
fn palm(x: f32) -> HandPose {
    HandPose { position: [x, 1.2, -0.3], orientation: [0.0, 0.0, 0.0, 1.0], valid: 1, received_tick: 0 }
}

#[test]
fn a_lost_hand_keeps_or_forgets_its_trail_as_the_rules_say() {
    let origin = from_back_up([0.3, 0.0, 0.95], [0.0, 1.0, 0.0], [4.0, 1.0, -2.0]).unwrap();
    for lost in [Lost::Kept, Lost::Forgotten] {
        let trails = HandTrails::new(TrailRules { kept: 0.8, span: 0.06, lost });
        for i in 0..20 {
            trails.note(&origin, &[Some(palm(0.05 * i as f32)), Some(palm(0.2))]);
            std::thread::sleep(Duration::from_millis(5));
        }
        // The left hand lost for a note; the right one still tracked.
        trails.note(&origin, &[None, Some(palm(0.2))]);
        let hands = trails.hands.lock().unwrap();
        assert_eq!(hands[0].trail.is_empty(), lost == Lost::Forgotten, "{lost:?}");
        assert!(!hands[1].trail.is_empty(), "{lost:?}: the tracked hand keeps its trail");
    }
}

#[test]
fn a_swing_reads_its_peak_stroke_and_release() {
    let trails = HandTrails::new(TrailRules { kept: 1.0, span: 0.03, lost: Lost::Kept });
    for i in 0..12 {
        trails.note_positions([Some([0.0, 0.0, -0.04 * i as f32]), None]);
        std::thread::sleep(Duration::from_millis(10));
    }
    let fastest = trails.fastest(0, 1.0).unwrap();
    assert!(fastest[2] < -1.0, "{fastest:?}");
    assert!(trails.stroke(0, 1.0, 0.1, 0.04).unwrap()[2] < -0.99);
    let release = trails.release(0, &ReleaseRules { span: 0.03, ..ReleaseRules::THROW }).unwrap();
    assert_eq!(release, Release::from_peak(fastest, trails.stroke(0, 0.5, 0.1, 0.04)));
    assert!(trails.palm(0, 1.0).is_none(), "places alone give no palm");
    assert_eq!(trails.held()[1], 0);
}
