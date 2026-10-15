use super::*;

/// Binary fingers clench and open with a gap between the two, and move there over
/// [`CLENCH_SECONDS`] rather than jumping. One test, as the state is the process's.
#[test]
fn binary_fingers_clench_with_a_gap_and_move_there() {
    set_binary(true);
    let pose = |bend: f32| FingerPose { valid: 1, fingers: [[bend, bend, bend, 0.2]; 5], ..FingerPose::default() };
    let start = Instant::now();
    let at = |seconds: f32| start + std::time::Duration::from_secs_f32(seconds);
    let fist = [FIST[1][0], FIST[1][1], FIST[1][2], 0.0];
    let open = [OPEN[1][0], OPEN[1][1], OPEN[1][2], 0.0];
    // First seen, the hand is where it is at once: open.
    assert_eq!(shape_at(1, pose(0.1), at(0.0)).fingers[1], open);
    // 0.55 rad per knuckle: 38% of the index's fist, past the 35% that clenches. A third of the way
    // through the clench the finger is part way, and by the end it is a fist.
    let part = shape_at(1, pose(0.55), at(CLENCH_SECONDS / 3.0)).fingers[1][1];
    assert!(part > OPEN[1][1] && part < FIST[1][1], "on its way: {part}");
    assert_eq!(shape_at(1, pose(0.55), at(CLENCH_SECONDS * 1.2)).fingers[1], fist, "clenched");
    // Easing off to 28% keeps it clenched; 14%, below a fifth, opens it, again over time.
    assert_eq!(shape_at(1, pose(0.4), at(0.3)).fingers[1], fist, "still clenched");
    let part = shape_at(1, pose(0.2), at(0.3 + CLENCH_SECONDS / 2.0)).fingers[1][1];
    assert!(part > OPEN[1][1] && part < FIST[1][1], "opening: {part}");
    assert_eq!(shape_at(1, pose(0.2), at(0.3 + CLENCH_SECONDS * 1.2)).fingers[1], open, "open");
    // The other hand keeps its own state.
    assert_eq!(shape_at(0, pose(0.4), at(1.0)).fingers[1], open, "the left hand never clenched");
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
