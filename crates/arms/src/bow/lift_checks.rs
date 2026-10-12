//! The ready-made against Dying Light 1's bow by hand before it, posing by posing. The geometry
//! helpers moved over unchanged (`in_frame`, `out_of_frame`, `arrow_line`, `aligned`); what is
//! compared is how the posings, the draw and the shots use them.

use super::from_nock::{self, Hang, aligned, arrow_line, in_frame, out_of_frame};
use super::tests::{Model, at, bow_model};
use super::*;
use monaka_core::gesture::Pull;
use monaka_core::math::sub;

const DL1: Bowing = Bowing { pull: PullRules::BOW, most_turn: 30.0, nock: "Arrow_Pose", nock_like: None, string: "bone_string", bow: "Bone_Bow" };

/// Dying Light 1's bow state before: what was known of the model, and the bow.
#[derive(Default)]
struct OldDl1 {
    model: usize,
    string: Option<i32>,
    nock: Option<i32>,
    bow: Option<i32>,
    arrow: Option<(usize, f32)>,
    bow_at_rest: Option<Mat34>,
    start: Option<(Option<Mat34>, Option<Mat34>)>,
    shot: Option<(Vec3, Vec3, f32, Instant)>,
}

/// Dying Light 1's `place_string` before, its clock passed in.
fn old_place_string(old: &mut OldDl1, model: &Model, reticle: Option<Vec3>, drawn: bool, fingers: Option<Vec3>, now: Instant) {
    let Some(mut root) = model.world(0) else { return };
    if old.model != model.id() {
        let nock = find(model, DL1.nock);
        *old = OldDl1 { model: model.id(), string: find(model, DL1.string), nock, bow: find(model, DL1.bow), arrow: nock.and_then(|nock| nock_axis(model, nock)), ..OldDl1::default() };
    }
    let Some(arrow) = old.arrow else { return };
    let nock_now = old.nock.and_then(|e| model.world(e)).and_then(|m| in_frame(&root, &m));
    let string_now = old.string.and_then(|e| model.world(e)).and_then(|m| in_frame(&root, &m));
    let nock_line = old.start.and_then(|s| s.0).or(nock_now);
    if let Some(turned) = reticle.and_then(|r| nock_line.and_then(|local| arrow_line(&root, &local, arrow)).and_then(|(at, way)| aligned(&root, way, sub(r, at), 30.0))) {
        model.set_world(0, &turned);
        root = turned;
    }
    let grip = [root[3], root[7], root[11]];
    if !drawn {
        old.start = None;
        old.bow_at_rest = old.bow.and_then(|e| model.world(e)).and_then(|m| in_frame(&root, &m));
        let axis = nock_now.and_then(|local| arrow_line(&root, &local, arrow)).map(|(_, way)| way);
        let kept = old.shot.filter(|b| now.duration_since(b.3) < DRAW_FRESH);
        old.shot = kept.or_else(|| axis.map(|axis| (grip, axis, 0.0, now)));
        return;
    }
    if old.start.is_none() {
        old.start = Some((nock_now, string_now));
    }
    let Some(start) = old.start else { return };
    let Some(local) = start.0 else { return };
    let nock = out_of_frame(&root, &local);
    let Some((from, axis)) = arrow_line(&root, &local, arrow) else { return };
    let pull = fingers.map_or(0.0, |f| Pull::back(from, axis, f, &PullRules::BOW));
    old.shot = Some((from, axis, pull, now));
    if let (Some(e), Some(local)) = (old.bow, old.bow_at_rest) {
        model.set_world(e, &out_of_frame(&root, &local));
    }
    let back = |m: Mat34| {
        let mut m = m;
        for k in 0..3 {
            m[k * 4 + 3] -= axis[k] * pull;
        }
        m
    };
    if let Some(e) = old.nock {
        model.set_world(e, &back(nock));
    }
    if let (Some(e), Some(local)) = (old.string, start.1) {
        model.set_world(e, &back(out_of_frame(&root, &local)));
    }
}

/// The game's own animation of the bow at step `i`: the root carried and turned with the hand,
/// the draw pulling its nock and string back.
fn animate(model: &Model, i: usize, drawn: bool) {
    let t = i as f32 * 0.05;
    let (s, c) = ((0.3 * t.sin()).sin(), (0.3 * t.sin()).cos());
    let root = [c, 0.0, s, 1.0 + 0.1 * t.cos(), 0.0, 1.0, 0.0, 1.5, -s, 0.0, c, -2.0 + 0.05 * t];
    let place = |offset: Vec3| [0, 1, 2].map(|k| root[3 + 4 * k] + root[4 * k] * offset[0] + root[4 * k + 1] * offset[1] + root[4 * k + 2] * offset[2]);
    let draw = if drawn { 0.3 } else { 0.0 };
    let mut worlds = model.worlds.borrow_mut();
    worlds[0] = root;
    worlds[1] = at(root, place([0.0; 3]));
    worlds[2] = at(root, place([-0.2 - draw, 0.0, 0.0]));
    worlds[3] = at(root, place([-0.2 - draw, 0.05, 0.0]));
}

#[test]
fn dying_light_1s_bow_is_posed_as_it_was_before() {
    let (old_model, new_model) = (bow_model(7), bow_model(7));
    let (mut old, mut new) = (OldDl1::default(), from_nock::State::new());
    let start = Instant::now();
    let mut shots = 0;
    for i in 0..300 {
        let now = start + Duration::from_millis(16 * i as u64);
        // Draws come and go; the posings stop a while (the bow put away: its pull goes stale).
        if (150..170).contains(&i) {
            continue;
        }
        let drawn = (i / 40) % 2 == 1;
        animate(&old_model, i, drawn);
        animate(&new_model, i, drawn);
        let reticle = (i % 7 != 0).then(|| [1.0 + 10.0 * (i as f32 * 0.01).cos(), 1.5 + 0.5 * (i as f32 * 0.03).sin(), -2.0 + 10.0 * (i as f32 * 0.01).sin()]);
        let fingers = (i % 11 != 0).then(|| [0.9 - 0.004 * (i % 40) as f32, 1.52, -2.0]);
        old_place_string(&mut old, &old_model, reticle, drawn, fingers, now);
        from_nock::place(&mut new, &DL1, &new_model, &Hang::World, reticle, drawn, fingers, true, now);
        assert_eq!(*new_model.worlds.borrow(), *old_model.worlds.borrow(), "the model at {i}");
        // As the bow looses: its draw and its shot.
        let loose = now + Duration::from_millis(5 * (i % 3) as u64);
        let was = old.shot.filter(|b| loose.duration_since(b.3) < DRAW_FRESH);
        let fresh = new.fresh(loose);
        assert_eq!(fresh.map(|b| (b.grip, b.axis, b.pull)), was.map(|b| (b.0, b.1, b.2)), "the shot at {i}");
        assert_eq!(fresh.and_then(|b| Pull::share(b.pull, &DL1.pull)), was.and_then(|b| Pull::share(b.2, &PullRules::BOW)), "the draw at {i}");
        shots += fresh.is_some() as usize;
    }
    assert!(shots > 200, "{shots}");
}

/// Dying Light 1's line before: the nock's axis most along the root's x, forward along +x.
fn along_root_x(root: &Mat34, nock: &Mat34) -> (usize, f32) {
    let x = monaka_core::camera::apply_3x3(root, [1.0, 0.0, 0.0]);
    let x = x.map(|c| c / math::length(x));
    let along = |c: usize| {
        let column = [nock[c], nock[4 + c], nock[8 + c]];
        math::dot(column, x) / math::length(column)
    };
    let axis = (0..3).max_by(|&a, &b| along(a).abs().total_cmp(&along(b).abs())).unwrap();
    (axis, along(axis).signum())
}

#[test]
fn the_models_line_is_the_one_along_the_roots_x() {
    const ROOT: Mat34 = [0.744, 0.668, 0.030, 407.438, -0.350, 0.427, -0.834, 17.538, -0.569, 0.610, 0.551, 287.791];
    let turns: [[f32; 9]; 6] = [
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        [-1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        [0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0],
    ];
    let bow = monaka_core::camera::compose(&ROOT, &[1.0, 0.0, 0.0, -0.001, 0.0, 1.0, 0.0, 0.001, 0.0, 0.0, 1.0, 0.0]);
    for r in turns {
        let local = [r[0], r[1], r[2], -0.2, r[3], r[4], r[5], -0.001, r[6], r[7], r[8], -0.001];
        let nock = monaka_core::camera::compose(&ROOT, &local);
        assert_eq!(arrow_axis(&nock, &bow), Some(along_root_x(&ROOT, &nock)), "{r:?}");
    }
}
