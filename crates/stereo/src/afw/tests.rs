use super::*;

const NEAR: f32 = 0.05;

/// A reverse-Z infinite projection, as The Beast's.
fn projection() -> Mat44 {
    let mut p = [0.0f32; 16];
    (p[0], p[5], p[11], p[14]) = (1.0, 1.0, NEAR, -1.0);
    p
}

/// Eye `eye` of a head at `x`, the eyes 1 apart.
fn rendered(x: f32, eye: usize) -> Rendered {
    let camera = [1.0, 0.0, 0.0, x + eye as f32 - 0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    Rendered::new(camera, projection(), NEAR).unwrap()
}

fn shot(x: f32, eye: usize) -> Shot {
    Shot { eye, pose: HeadPose { position: [x, 0.0, 0.0], valid: 1, ..HeadPose::default() }, rendered: rendered(x, eye), other: rendered(x, 1 - eye) }
}

/// A warp of `shot` as a backend runs one: begun, its frame kept, then drawn.
fn warp(history: &mut History, shot: &Shot) -> Warp {
    history.begin();
    history.keep(shot);
    history.warp(shot)
}

#[test]
fn the_gaps_come_from_the_other_eyes_own_newest_frame() {
    let mut history = History::new(Rules::WHOLE);
    let first = warp(&mut history, &shot(0.0, 0));
    assert_eq!((first.eye, first.primary, first.target, first.fill), (0, rendered(0.0, 0), rendered(0.0, 1), None), "nothing of the right eye yet");
    // The right eye next: its gaps from the left eye's frame, as that was rendered.
    let second = warp(&mut history, &shot(1.0, 1));
    assert_eq!(second.fill, Some(rendered(0.0, 0)));
    // The right eye again: still the left eye's frame, never its own.
    assert_eq!(warp(&mut history, &shot(2.0, 1)).fill, Some(rendered(0.0, 0)));
    // A lost copy fills nothing.
    history.lose(0);
    assert_eq!(warp(&mut history, &shot(3.0, 1)).fill, None);
}

#[test]
fn an_old_frame_fills_only_without_an_age_limit() {
    for (rules, fills) in [(Rules::WHOLE, false), (Rules::WARPED_ONLY, true)] {
        let mut history = History::new(rules);
        warp(&mut history, &shot(0.0, 0));
        // The left eye's frame is four warps old at the fourth right-eye warp, five at the fifth.
        for k in 1..=4 {
            assert!(warp(&mut history, &shot(k as f32, 1)).fill.is_some(), "{rules:?} warp {k}");
        }
        assert_eq!(warp(&mut history, &shot(5.0, 1)).fill.is_some(), fills, "{rules:?}");
        history.forget();
        assert_eq!(warp(&mut history, &shot(6.0, 0)).fill, None, "nothing after a cut");
    }
}

#[test]
fn the_shown_eye_is_the_one_its_cameras_were_recorded_for() {
    let schedule = Schedule { latency: 2 };
    let cameras = |x: f32| EyeCameras {
        camera: [rendered(x, 0).camera, rendered(x, 1).camera],
        projection: [projection(); 2],
        mapping: [rendered(x, 0).mapping; 2],
        pose: HeadPose { position: [x, 0.0, 0.0], ..HeadPose::default() },
    };
    let mut ring = PresentRing::new();
    // The left eye chosen after present 10 shows at present 12.
    ring.put(10, cameras(10.0));
    let shot = Shot::scheduled(schedule, 12, &ring).unwrap();
    assert_eq!((shot.eye, shot.rendered, shot.other, shot.pose.position[0]), (0, rendered(10.0, 0), rendered(10.0, 1), 10.0));
    assert_eq!(shot.views(), [rendered(10.0, 0), rendered(10.0, 1)]);
    // Nothing recorded for present 13's frame; with the latency changed to 3, present 13 shows
    // what was recorded after present 10.
    assert!(Shot::scheduled(schedule, 13, &ring).is_none());
    assert_eq!(Shot::scheduled(Schedule { latency: 3 }, 13, &ring).map(|s| s.pose.position[0]), Some(10.0));
}

#[test]
fn a_labelled_frames_other_eye_is_a_separation_along_its_right_axis() {
    let label = Pending { eye: 1, pose: HeadPose::default() };
    let frustum = Frustum::centred([1.2, 0.6]);
    let shot = Shot::labelled(label, rendered(0.0, 1), 1.0, frustum);
    assert_eq!(shot.other.camera, rendered(0.0, 0).camera, "the left eye one to the left");
    assert!((shot.other.projection[0] - 1.0 / 1.2).abs() < 1e-6, "its own frustum");
    assert_eq!(shot.views()[1], rendered(0.0, 1));
}

#[test]
fn menus_go_flat_and_a_lone_unlabelled_frame_waits() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut presents = Presents::new(Rules::WHOLE);
    presents.generated(Previous { pose: HeadPose::default(), eye: 0, present: 1 });
    assert_eq!(presents.out(false, true, true, at(0)), Out::Warped);
    assert_eq!(presents.out(false, false, false, at(100)), Out::Nothing, "a moment after a labelled frame");
    assert_eq!(presents.generation(1, 2, false), (Some(Previous { pose: HeadPose::default(), eye: 0, present: 1 }), true), "nothing forgotten yet");
    let out = presents.out(false, false, false, at(300));
    assert!(out == Out::BothEyes && out.forgets());
    assert_eq!(presents.out(true, true, true, at(310)), Out::Flat);
    assert_eq!(presents.out(false, true, false, at(320)), Out::Plain);
    // Without the rules, a frame is warped or nothing.
    let mut warped_only = Presents::new(Rules::WARPED_ONLY);
    assert_eq!(warped_only.out(true, false, true, at(0)), Out::Warped);
    assert_eq!(warped_only.out(true, false, false, at(1000)), Out::Nothing);
}

#[test]
fn a_plain_pair_needs_the_other_eyes_frame_just_before() {
    let mut presents = Presents::new(Rules::WHOLE);
    let pose = |x: f32| HeadPose { position: [x, 0.0, 0.0], ..HeadPose::default() };
    assert_eq!(presents.plain(0, pose(1.0), 10), None);
    assert_eq!(presents.plain(1, pose(2.0), 11), Some(pose(1.0)));
    assert_eq!(presents.plain(0, pose(3.0), 13), None, "the right eye's is two presents old");
    presents.lose_plain(0);
    assert_eq!(presents.plain(1, pose(4.0), 14), None, "the left eye's copy was being replaced");
}

#[test]
fn frame_generations_history_runs_on_only_from_the_other_eye_just_before() {
    let mut presents = Presents::new(Rules::WHOLE);
    let last = Previous { pose: HeadPose::default(), eye: 0, present: 7 };
    for (eye, present, reset, runs_on) in [(1, 8, false, true), (0, 8, false, false), (1, 9, false, false), (1, 8, true, false)] {
        presents.generated(last);
        assert_eq!(presents.generation(eye, present, reset), (Some(last), runs_on), "{eye} {present} {reset}");
        assert_eq!(presents.generation(eye, present, reset).0, None, "taken");
    }
    presents.generated(last);
    presents.forget();
    assert_eq!(presents.generation(1, 8, false), (None, false));
}
