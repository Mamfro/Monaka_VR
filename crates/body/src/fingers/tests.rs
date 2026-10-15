use super::*;

#[test]
fn binary_fingers_snap_with_a_gap() {
    set_binary(true);
    let pose = |bend: f32| FingerPose { valid: 1, fingers: [[bend, bend, bend, 0.2]; 5], ..FingerPose::default() };
    // 0.55 rad per knuckle: 38% of the index's fist, past the 35% that clenches.
    let half = shape(1, pose(0.55));
    assert_eq!(half.fingers[1], [FIST[1][0], FIST[1][1], FIST[1][2], 0.0], "clenched");
    // Easing off to 28% keeps it clenched; 14%, below a fifth, opens it.
    assert_eq!(shape(1, pose(0.4)).fingers[1][0], FIST[1][0], "still clenched");
    assert_eq!(shape(1, pose(0.2)).fingers[1], [OPEN[1][0], OPEN[1][1], OPEN[1][2], 0.0], "open");
    // The other hand keeps its own state.
    assert_eq!(shape(0, pose(0.4)).fingers[1][0], OPEN[1][0], "the left hand never clenched");
    set_binary(false);
    assert_eq!(shape(1, pose(0.4)).fingers[1], [0.4, 0.4, 0.4, 0.2], "off: as tracked");
}

const IDENTITY: Mat34 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// A finger of three 4 cm bones along +x from the hand at the origin, its rest pose relaxed
/// (each knuckle 20 degrees about +y, the curl axis), plus an end joint on the last bone.
struct Test {
    worlds: Vec<Mat34>,
}

impl FingerJoints for Test {
    fn world(&self, joint: i32) -> Option<Mat34> {
        self.worlds.get(joint as usize).copied()
    }
    fn set_world(&mut self, joint: i32, matrix: &Mat34) {
        self.worlds[joint as usize] = *matrix;
    }
    fn rest_between(&self, from: i32, to: i32) -> Option<Mat34> {
        let mut rest = turn([0.0, 1.0, 0.0], 20f32.to_radians());
        // Each bone sits 4 cm along its parent's x (the end, 2 cm).
        rest[3] = if to == 4 { 0.02 } else { 0.04 };
        (to == from + 1).then_some(rest)
    }
}

fn hand() -> Hand {
    let index = Finger { base: 0, knuckles: vec![1, 2, 3], followers: vec![(4, 3)] };
    Hand { fingers: [None, Some(index), None, None, None], along: [1.0, 0.0, 0.0], curl: [0.0, 1.0, 0.0] }
}

fn tip(bend: f32) -> Vec3 {
    let mut test = Test { worlds: vec![IDENTITY; 5] };
    let mut tracked = FingerPose { valid: 1, ..FingerPose::default() };
    tracked.fingers[1] = [bend, bend, bend, 0.0];
    apply(&mut test, &hand(), &tracked);
    [test.worlds[4][3], test.worlds[4][7], test.worlds[4][11]]
}

#[test]
fn an_open_hand_lies_straight() {
    // No bend: every bone along the hand's x, the relaxed curl taken out.
    let t = tip(0.0);
    assert!((t[0] - 0.14).abs() < 1e-5 && t[1].abs() < 1e-5 && t[2].abs() < 1e-5, "{t:?}");
}

#[test]
fn a_curl_turns_about_the_curl_axis() {
    // A quarter turn at each knuckle about +y (x goes to -z): the bones run 4 cm out along x,
    // down along -z, back along -x, and the end 2 cm up along +z, under the hand.
    let t = tip(std::f32::consts::FRAC_PI_2);
    assert!(t[0].abs() < 1e-5 && t[1].abs() < 1e-5 && (t[2] + 0.02).abs() < 1e-5, "{t:?}");
}
