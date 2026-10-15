use super::*;

/// Each bone's pose against its parent, from its pose in the skeleton's space (`model`; a
/// root's stays as given).
fn rests_against_parents(parents: &[i32], model: &[Mat34]) -> Vec<Mat34> {
    (0..model.len())
        .map(|i| match usize::try_from(parents[i]).ok().and_then(|p| model.get(p)) {
            Some(parent) => camera::compose(&camera::rigid_inverse(parent), &model[i]),
            None => model[i],
        })
        .collect()
}

/// Two hands of five fingers, `joints` curled bones each (an optional metacarpal before them),
/// with a relaxed rest curl, a watch on the left wrist and a nub on each index tip.
struct Fixture {
    parents: Vec<i32>,
    rest: Vec<Mat34>,
    world: Vec<Mat34>,
    chains: Chains,
    tip: [usize; 2],
    watch: usize,
}

fn push(f: &mut Fixture, parent: usize, rest: Mat34) -> usize {
    f.parents.push(parent as i32);
    f.rest.push(rest);
    f.parents.len() - 1
}

fn fixture(joints: usize, metacarpals: bool) -> Fixture {
    curled_fixture(joints, metacarpals, -20.0)
}

/// As [`fixture`], each knuckle curled `degrees` in the reference pose.
fn curled_fixture(joints: usize, metacarpals: bool, degrees: f32) -> Fixture {
    let mut f = Fixture { parents: vec![-1], rest: vec![camera::translation([0.0; 3])], world: Vec::new(), chains: Chains::default(), tip: [0; 2], watch: 0 };
    for side in 0..2 {
        let wrist = push(&mut f, 0, camera::translation([if side == 0 { -0.2 } else { 0.2 }, 0.0, 0.0]));
        f.chains.wrists[side] = wrist;
        for finger in 0..5 {
            let x = [0.06, 0.04, 0.0, -0.02, -0.04][finger] * if side == 0 { 1.0 } else { -1.0 };
            let mut parent = wrist;
            if metacarpals {
                parent = push(&mut f, wrist, camera::translation([x * 0.2, 0.0, -0.005]));
            }
            for k in 0..joints {
                let offset = if k == 0 { [if metacarpals { x * 0.8 } else { x }, 0.0, -0.08] } else { [0.0, 0.0, -0.04] };
                // A relaxed reference curl; a tracked zero takes it out.
                let a = degrees.to_radians();
                let rest = camera::apply_head(&camera::translation(offset), [(a / 2.0).sin(), 0.0, 0.0, (a / 2.0).cos()], [0.0; 3]);
                let bone = push(&mut f, parent, rest);
                f.chains.fingers[side][finger].push(bone);
                parent = bone;
            }
            if finger == 1 {
                f.tip[side] = parent;
                push(&mut f, parent, camera::translation([0.0, 0.0, -0.02]));
            }
        }
    }
    let left_wrist = f.chains.wrists[0];
    f.watch = push(&mut f, left_wrist, camera::translation([0.0, 0.02, 0.0]));
    for i in 0..f.rest.len() {
        let world = if f.parents[i] < 0 { f.rest[i] } else { camera::compose(&f.world[f.parents[i] as usize], &f.rest[i]) };
        f.world.push(world);
    }
    f
}

fn open() -> FingerPose {
    FingerPose { valid: 1, ..FingerPose::default() }
}

#[test]
fn mirrored_fingers_curl_toward_palms_without_moving_wrists_watch_or_other_hand() {
    for (joints, metacarpals) in [(3, false), (3, true), (2, false)] {
        let f = fixture(joints, metacarpals);
        let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
        for side in 0..2 {
            let mut opened = f.world.clone();
            assert!(rig.apply(&mut opened, side, open()));
            let mut closed = f.world.clone();
            assert!(rig.apply(&mut closed, side, FingerPose { fingers: [[0.7, 0.8, 0.6, 0.0]; 5], ..open() }));
            let tip = f.tip[side];
            assert!(position(&closed[tip])[1] < position(&opened[tip])[1] - 0.02, "{joints} joints, metacarpals {metacarpals}");
            // The nub rides its knuckle.
            let (nub, knuckle) = (tip + 1, tip);
            let riding = camera::compose(&camera::rigid_inverse(&closed[knuckle]), &closed[nub]);
            assert!(camera::distance(&riding, &f.rest[nub]) < 1e-4);
            let wrist = f.chains.wrists[side];
            assert_eq!(opened[wrist], f.world[wrist]);
            assert_eq!(closed[f.watch], f.world[f.watch]);
            let other = f.chains.wrists[1 - side];
            assert_eq!(closed[other], f.world[other]);
            // Segments keep their reference lengths.
            let chain = &f.chains.fingers[side][1];
            for w in chain.windows(2) {
                let (a, b) = (position(&closed[w[1]]), position(&closed[w[0]]));
                let length = (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt();
                assert!((length - 0.04).abs() < 1e-5);
            }
        }
    }
}

#[test]
fn invalid_tracking_and_wrong_hierarchies_preserve_animation() {
    let mut f = fixture(3, false);
    let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
    let mut frames = f.world.clone();
    assert!(!rig.apply(&mut frames, 0, FingerPose::default()));
    let mut bad = open();
    bad.fingers[1][1] = f32::NAN;
    assert!(!rig.apply(&mut frames, 0, bad));
    assert_eq!(frames, f.world);
    let bone = f.chains.fingers[0][1][1];
    f.parents[bone] = 0;
    assert!(Rig::discover(&f.parents, f.rest.clone(), &f.chains).is_err());
    f.parents[bone] = bone as i32;
    assert!(Rig::discover(&f.parents, f.rest.clone(), &f.chains).is_err());
    let mut short = fixture(3, false);
    short.chains.fingers[0][2].truncate(1);
    assert!(Rig::discover(&short.parents, short.rest.clone(), &short.chains).is_err());
    // A hand without a thumb or ring finger still has a palm; one without a middle finger not.
    let mut fewer = fixture(3, false);
    fewer.chains.fingers[0][0].clear();
    fewer.chains.fingers[1][3].clear();
    assert!(Rig::discover(&fewer.parents, fewer.rest.clone(), &fewer.chains).is_ok());
    fewer.chains.fingers[0][2].clear();
    assert!(Rig::discover(&fewer.parents, fewer.rest.clone(), &fewer.chains).is_err());
}

#[test]
fn individual_fingers_and_two_thumb_bends_are_independent_and_bounded() {
    for joints in [3, 2] {
        let f = fixture(joints, false);
        let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
        let mut opened = f.world.clone();
        assert!(rig.apply(&mut opened, 0, open()));
        let mut pose = open();
        pose.fingers[1] = [0.8, 0.4, 0.3, 0.0];
        let mut index = f.world.clone();
        assert!(rig.apply(&mut index, 0, pose));
        for finger in [0, 2, 3, 4] {
            for &bone in &f.chains.fingers[0][finger] {
                assert_eq!(index[bone], opened[bone]);
            }
        }
        let thumb = &f.chains.fingers[0][0];
        let mut pose = open();
        pose.fingers[0] = [0.4, 0.9, 0.0, 0.0];
        let mut tracked = f.world.clone();
        assert!(rig.apply(&mut tracked, 0, pose));
        // A three-bone thumb keeps its first bone; its last two (and a two-bone thumb's both)
        // take the bends.
        if joints == 3 {
            assert_eq!(tracked[thumb[0]], opened[thumb[0]]);
        }
        assert_ne!(tracked[thumb[joints - 2]], opened[thumb[joints - 2]]);
        assert_ne!(tracked[thumb[joints - 1]], opened[thumb[joints - 1]]);
        let mut huge = open();
        huge.fingers[1] = [100.0; 4];
        let mut bounded = open();
        bounded.fingers[1] = [1.9; 4];
        let (mut a, mut b) = (f.world.clone(), f.world.clone());
        assert!(rig.apply(&mut a, 0, huge));
        assert!(rig.apply(&mut b, 0, bounded));
        assert_eq!(a, b);
    }
}

#[test]
fn applying_twice_from_the_same_hand_gives_the_same_fingers() {
    let f = fixture(3, true);
    let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
    let pose = FingerPose { fingers: [[0.5, 0.6, 0.4, 0.0]; 5], ..open() };
    let mut once = f.world.clone();
    assert!(rig.apply(&mut once, 1, pose));
    let mut twice = once.clone();
    assert!(rig.apply(&mut twice, 1, pose));
    for (a, b) in once.iter().zip(&twice) {
        assert!(camera::distance(a, b) < 1e-5);
    }
}

#[test]
fn a_curled_reference_pose_gives_an_open_ones_axes() {
    let f = fixture(3, true);
    let open = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
    // Each knuckle 80 degrees in: the fingers point into the palm, where the bone times the
    // palm's normal says nothing.
    let f = curled_fixture(3, true, -80.0);
    let curled = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
    for side in 0..2 {
        for ((_, _, a), (_, _, b)) in open.hands[side].iter().zip(&curled.hands[side]) {
            assert!(distance(a.curl, b.curl) < 1e-4, "side {side}: {:?} against {:?}", a.curl, b.curl);
        }
    }
}

#[test]
fn chains_by_name() {
    let f = fixture(3, false);
    const NAMES: FingerNames = FingerNames { wrists: ["l_hand", "r_hand"], sides: ["Left", "Right"], fingers: ["Thumb", "Index", "Middle", "Ring", "Pinky"], bone: "{side}Hand{finger}{n}" };
    let name = |bone: usize| -> Option<String> {
        for side in 0..2 {
            if f.chains.wrists[side] == bone {
                return Some(NAMES.wrists[side].to_owned());
            }
            for finger in 0..5 {
                if let Some(n) = f.chains.fingers[side][finger].iter().position(|&b| b == bone) {
                    return Some(NAMES.name(side, finger, n + 1));
                }
            }
        }
        None
    };
    let named: Vec<Option<String>> = (0..f.parents.len()).map(name).collect();
    let chains = Chains::named(&NAMES, |wanted| named.iter().position(|n| n.as_deref() == Some(wanted))).unwrap();
    assert_eq!(chains, f.chains);
    assert_eq!(NAMES.name(1, 3, 2), "RightHandRing2");
    assert!(Chains::named(&NAMES, |_| None).unwrap_err().contains("l_hand"));
}

#[test]
fn chains_by_shape() {
    // A right hand: wrist 0 (forearm 1), a three-bone thumb (2-4) on the index side (+y),
    // four-bone fingers index 5-8, middle 9-12, ring 13-16, little 17-20.
    let mut parents = vec![-1, 0];
    let mut model = vec![camera::translation([0.0; 3]), camera::translation([-5.0, 0.0, 0.0])];
    for (len, y) in [(3, 3.0), (4, 1.5), (4, 0.5), (4, -0.5), (4, -1.5)] {
        for k in 0..len {
            parents.push(if k == 0 { 0 } else { parents.len() as i32 - 1 });
            model.push(camera::translation([2.0 + 2.0 * k as f32, y, 0.0]));
        }
    }
    let chains = Chains::by_shape(&parents, &model, [0, 0], [1, 1]).unwrap();
    assert_eq!(chains.fingers[1], [vec![2, 3, 4], vec![6, 7, 8], vec![10, 11, 12], vec![14, 15, 16], vec![18, 19, 20]]);
    // The forearm taken for a finger: six.
    assert!(Chains::by_shape(&parents, &model, [0, 0], [usize::MAX; 2]).unwrap_err().contains("6 fingers"));
}

#[test]
fn one_hand_and_model_space_rests() {
    let f = fixture(3, true);
    let rest = rests_against_parents(&f.parents, &f.world);
    assert!(rest.iter().zip(&f.rest).all(|(a, b)| camera::distance(a, b) < 1e-5));
    let mut chains = f.chains.clone();
    chains.fingers[1] = Default::default();
    let rig = Rig::discover(&f.parents, rest, &chains).unwrap();
    let mut frames = f.world.clone();
    assert!(rig.apply(&mut frames, 1, FingerPose { fingers: [[0.7; 4]; 5], ..open() }));
    assert_eq!(frames, f.world, "no right hand: nothing moves");
    assert!(rig.apply(&mut frames, 0, FingerPose { fingers: [[0.7; 4]; 5], ..open() }));
    assert_ne!(frames, f.world);
    // A scaled bone the hands do not use (the watch) is no reason to refuse.
    let mut scaled = f.rest.clone();
    scaled[f.watch][0] = 2.0;
    let rig = Rig::discover(&f.parents, scaled, &f.chains).unwrap();
    let mut frames = f.world.clone();
    frames[f.watch][0] = 2.0;
    assert!(rig.apply(&mut frames, 0, FingerPose { fingers: [[0.7; 4]; 5], ..open() }));
}
