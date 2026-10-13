use super::*;
use monaka_core::math::sub;
use std::cell::RefCell;

/// A bow model: a root, the bow bone on it, the nock 0.2 m behind the bow bone along the root's
/// -x (its own x along the root's x), the string beside the nock.
pub(super) struct Model {
    pub(super) id: usize,
    pub(super) names: Vec<&'static str>,
    pub(super) parents: Vec<i32>,
    pub(super) worlds: RefCell<Vec<Mat34>>,
}

impl Skeleton for Model {
    fn id(&self) -> usize {
        self.id
    }
    fn count(&self) -> i32 {
        self.names.len() as i32
    }
    fn name(&self, element: i32) -> String {
        self.names[element as usize].to_string()
    }
    fn parent(&self, element: i32) -> i32 {
        self.parents[element as usize]
    }
    fn world(&self, element: i32) -> Option<Mat34> {
        self.worlds.borrow().get(usize::try_from(element).ok()?).copied()
    }
    fn set_world(&self, element: i32, matrix: &Mat34) {
        self.worlds.borrow_mut()[element as usize] = *matrix;
    }
    fn is_bone(&self, _element: i32) -> bool {
        true
    }
}

const IDENTITY: Mat34 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];

pub(super) fn at(m: Mat34, p: Vec3) -> Mat34 {
    let mut m = m;
    (m[3], m[7], m[11]) = (p[0], p[1], p[2]);
    m
}

pub(super) fn bow_model(id: usize) -> Model {
    Model {
        id,
        names: vec!["bone_root", "Bone_Bow", "Arrow_Pose", "bone_string"],
        parents: vec![-1, 0, 1, 1],
        worlds: RefCell::new(vec![at(IDENTITY, [1.0, 1.5, -2.0]), at(IDENTITY, [1.0, 1.5, -2.0]), at(IDENTITY, [0.8, 1.5, -2.0]), at(IDENTITY, [0.8, 1.55, -2.0])]),
    }
}

#[test]
fn the_arrow_runs_from_the_nock_toward_the_bow_it_hangs_from() {
    let model = bow_model(1);
    let nock = find_any_case(&model, "arrow_pose").unwrap();
    assert_eq!(nock, 2, "found without case");
    assert_eq!(nock_axis(&model, nock), Some((0, 1.0)), "the nock's x, forward");
    // Turned half a turn about y, the nock's x points back: forward is its -x.
    let back = [-1.0, 0.0, 0.0, 0.8, 0.0, 1.0, 0.0, 1.5, 0.0, 0.0, -1.0, -2.0];
    model.set_world(2, &back);
    assert_eq!(nock_axis(&model, nock), Some((0, -1.0)));
    // A nock at its bow (no depth) or across it has no line.
    assert_eq!(arrow_axis(&at(IDENTITY, [1.0, 1.5, -2.0]), &at(IDENTITY, [1.02, 1.5, -2.0])), None);
    let across = [0.0, 0.0, 1.0, 0.8, 0.0, 1.0, 0.0, 1.5, -1.0, 0.0, 0.0, -2.0];
    assert_eq!(arrow_axis(&[0.7, 0.7, 0.0, 0.8, -0.7, 0.7, 0.0, 1.5, 0.0, 0.0, 1.0, -2.0], &at(IDENTITY, [1.0, 1.5, -2.0])), None, "no axis close enough");
    assert!(arrow_axis(&across, &at(IDENTITY, [1.0, 1.5, -2.0])).is_some(), "its -z along the way");
    assert_eq!(find(&model, "arrow_pose"), None, "exact names only");
}

#[test]
fn the_line_is_kept_in_the_roots_frame() {
    // A root turned a quarter about y, the nock 0.2 m behind it along the root's own x.
    let root: Mat34 = [0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.5, -1.0, 0.0, 0.0, -2.0];
    let nock: Mat34 = [0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.55, -1.0, 0.0, 0.0, -1.8];
    let line = nock_line(&nock, &root, 0, 1.0);
    let close = |a: Vec3, b: Vec3| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
    assert!(close(line.from, [-0.2, 0.05, 0.0]), "{:?}", line.from);
    assert!(close(line.along, [1.0, 0.0, 0.0]), "{:?}", line.along);
    assert!(close(line.level_with_origin(), [0.0, 0.05, 0.0]));
}

#[test]
fn a_bow_is_turned_to_the_reticle_only_when_nearly_aimed_there() {
    let root = at(IDENTITY, [0.0; 3]);
    let ahead = from_nock::aligned(&root, [1.0, 0.0, 0.0], [10.0, 1.0, 0.0], 30.0).unwrap();
    let x = math::unit([ahead[0], ahead[4], ahead[8]]).unwrap();
    assert!((x[1] - (1.0f32).atan2(10.0).sin()).abs() < 1e-5, "{x:?}");
    assert!(from_nock::aligned(&root, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 30.0).is_none(), "a quarter turn away: not aimed");
}

/// The Chrome games' rules: Dying Light 1's, the bow model's names found without case and its nock
/// else by the arrow.
const CHROME: Bowing = Bowing { pull: PullRules::BOW, most_turn: 30.0, nock: "arrow_pose", nock_like: Some("arrow"), string: "bone_string", bow: "bone_bow" };

#[test]
fn the_elements_are_found_by_name_exactly_else_without_case_and_the_nock_else_by_the_arrow() {
    let model = bow_model(1);
    assert_eq!((CHROME.nock_in(&model), Bowing::element(&model, CHROME.bow), Bowing::element(&model, CHROME.string)), (Some(2), Some(1), Some(3)));
    let renamed = Model { names: vec!["ARROW_POSE", "Bone_Bow", "arrow_pose", "bone_string"], ..bow_model(2) };
    assert_eq!(Bowing::element(&renamed, "arrow_pose"), Some(2), "the exact name first");
    let named_for_the_arrow = Model { names: vec!["bone_root", "Bone_Bow", "Arrow_Nock", "bone_string"], ..bow_model(3) };
    assert_eq!(CHROME.nock_in(&named_for_the_arrow), Some(2));
    assert_eq!(Bowing { nock_like: None, ..CHROME }.nock_in(&named_for_the_arrow), None);
}

/// The left holder where the rig placed it: a quarter turn about y (its x along the world's -z),
/// at (0, 1, 0), away from where the game hangs the model.
const HOLDER: Mat34 = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, -1.0, 0.0, 0.0, 0.0];

fn close(a: Vec3, b: Vec3) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

#[test]
fn held_against_its_parents_the_bow_leaves_from_the_placed_holder_and_its_string_goes_back_by_the_pull() {
    let model = bow_model(4);
    let before = model.worlds.borrow().clone();
    let writes = RefCell::new(Vec::new());
    let set_local = |e: i32, m: &Mat34| {
        writes.borrow_mut().push((e, *m));
        true
    };
    let hang = from_nock::Hang::Local { root: Some(HOLDER), set_local: &set_local };
    let mut state = from_nock::State::new();
    let now = Instant::now();
    // Not drawn: the arrow would leave from the holder along the line, nothing placed.
    assert!(!from_nock::place(&mut state, &CHROME, &model, &hang, None, false, None, true, now));
    let bow = state.fresh(now).unwrap();
    assert!(close(bow.grip, [0.0, 1.0, 0.0]) && close(bow.axis, [0.0, 0.0, -1.0]), "{bow:?}");
    assert!(writes.borrow().is_empty());
    // Drawn, the string hand 0.3 m behind the nock: the arrow from the nock, the pull 0.3 m, and
    // the bow bone at rest, the nock and the string 0.3 m back, each against its parent.
    assert!(from_nock::place(&mut state, &CHROME, &model, &hang, None, true, Some([0.0, 1.0, 0.5]), true, now));
    let bow = state.fresh(now).unwrap();
    assert!(close(bow.grip, [0.0, 1.0, 0.2]) && close(bow.axis, [0.0, 0.0, -1.0]) && (bow.pull - 0.3).abs() < 1e-5, "{bow:?}");
    let placed: Vec<(i32, Vec3)> = writes.borrow().iter().map(|&(e, m)| (e, [m[3], m[7], m[11]])).collect();
    assert_eq!(placed.iter().map(|p| p.0).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(close(placed[0].1, [0.0; 3]) && close(placed[1].1, [-0.5, 0.0, 0.0]) && close(placed[2].1, [-0.5, 0.05, 0.0]), "{placed:?}");
    assert_eq!(*model.worlds.borrow(), before, "nothing placed in the world");
}

#[test]
fn held_against_its_parents_the_bow_bone_carries_the_turn_toward_the_reticle() {
    let model = bow_model(5);
    let writes = RefCell::new(Vec::new());
    let set_local = |e: i32, m: &Mat34| {
        writes.borrow_mut().push((e, *m));
        true
    };
    let hang = from_nock::Hang::Local { root: Some(HOLDER), set_local: &set_local };
    let mut state = from_nock::State::new();
    let now = Instant::now();
    // The reticle 10 m out along the bow's line from the nock, 1 m up.
    let reticle = [0.0, 2.0, 0.2 - 10.0];
    from_nock::place(&mut state, &CHROME, &model, &hang, Some(reticle), false, None, true, now);
    let toward = math::unit(sub(reticle, [0.0, 1.0, 0.2])).unwrap();
    let bow = state.fresh(now).unwrap();
    assert!(math::dot(bow.axis, toward) > 0.9999, "the arrow at the reticle: {bow:?}");
    let written = writes.borrow();
    assert_eq!(written.len(), 1, "the bow bone alone");
    let (element, local) = written[0];
    let turned = from_nock::out_of_frame(&HOLDER, &local);
    assert!(element == 1 && math::dot([turned[0], turned[4], turned[8]], toward) > 0.9999, "{written:?}");
    // Out of the turn's reach, the bow is not being aimed: nothing turns.
    drop(written);
    writes.borrow_mut().clear();
    let mut state = from_nock::State::new();
    from_nock::place(&mut state, &CHROME, &model, &hang, Some([10.0, 1.0, 0.0]), false, None, true, now);
    assert!(writes.borrow().is_empty() && close(state.fresh(now).unwrap().axis, [0.0, 0.0, -1.0]));
}
