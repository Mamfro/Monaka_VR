//! The bow's line from the nock as the draw began.
//!
//! At each posing, where the left hand holds a bow: the bow is turned in the hand about its root
//! so the arrow points at the left reticle (a few degrees, the same each update); while not drawn,
//! where the arrow would leave and which way is kept (the root, along the nock's axis); while drawn,
//! the nock and the string as the draw began are the line, the pull is how far the string hand is
//! behind the nock along it, and the bow bone stays as it rests in the hand while the nock and the
//! string go straight back along the line. The pull of a draw is kept past its end: the bow leaves
//! its drawing state as it is let go, a moment before it looses.
//!
//! The model is read in its own root's terms, so where the engine hangs it while it is read does
//! not matter; how what is placed on it holds is the game's ([`super::Keeps`], [`Hang`]).

use super::{Bowing, Bows, DRAW_FRESH, Keeps, drawing, nock_axis, note_held};
use crate::Skeleton;
use crate::launch::{Fired, Shot};
use crate::held::Kind;
use monaka_core::camera::{Mat34, apply_3x3, unapply_3x3};
use monaka_core::gesture::Pull;
use monaka_core::math::{self, Vec3, dot, sub};
use monaka_contract::protocol::LEFT_HAND;
use monaka_core::sync::Locked;
use monaka_producer::game::lines;
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

/// The held bow model's elements, the nock's axis the arrow lies along and which way along it is
/// forward (none for a model without a nock behind its bow), its bow bone at rest in the root's
/// frame, and where the draw began.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Known {
    model: usize,
    string: Option<i32>,
    nock: Option<i32>,
    bow: Option<i32>,
    arrow: Option<(usize, f32)>,
    bow_at_rest: Option<Mat34>,
    start: Option<Start>,
}

/// Where a draw began: the nock and the string in the bow root's terms.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Start {
    nock: Option<Mat34>,
    string: Option<Mat34>,
}

/// The bow as of the latest posing: where the arrow leaves from, its direction (unit), the pull.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bow {
    pub(super) grip: Vec3,
    pub(super) axis: Vec3,
    pub(super) pull: f32,
    pub(super) at: Instant,
}

/// What is known of the held bow, and the bow as of the latest posing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct State {
    known: Known,
    pub(super) bow: Option<Bow>,
}

impl State {
    pub(super) const fn new() -> Self {
        Self { known: Known { model: 0, string: None, nock: None, bow: None, arrow: None, bow_at_rest: None, start: None }, bow: None }
    }

    /// The bow where it was posed less than [`DRAW_FRESH`] before `now`.
    pub(super) fn fresh(&self, now: Instant) -> Option<Bow> {
        self.bow.filter(|b| now.duration_since(b.at) < DRAW_FRESH)
    }
}

/// How the bow model holds what is placed on it at a posing ([`Keeps`]).
pub(super) enum Hang<'a> {
    /// World matrices set hold: the root is turned and each element placed in the world.
    World,
    /// Each element is placed against its parent (`set_local`); the root is where the hand rig
    /// placed the left holder (`root`; none: where the model has it), and the bow bone carries the
    /// turn.
    Local { root: Option<Mat34>, set_local: &'a dyn Fn(i32, &Mat34) -> bool },
}

static STATE: Mutex<State> = Mutex::new(State::new());
static PLACED: AtomicU64 = AtomicU64::new(0);
static ARROWS: AtomicU64 = AtomicU64::new(0);

/// Each arrow leaves from the bow along its axis, the game's speed kept.
pub(super) fn shot(_fired: &Fired) -> Option<Shot> {
    let bow = STATE.locked().fresh(Instant::now())?;
    let n = ARROWS.fetch_add(1, Relaxed) + 1;
    if n <= 20 {
        log!("bow by hand: arrow {n} from {:.2?} along {:.2?}, pulled {:.2} m", bow.grip, bow.axis, bow.pull);
    }
    Some(Shot { from: bow.grip, direction: bow.axis, speed: None })
}

/// The draw's share whenever the game takes it (as the arrow leaves, or at each of the bow's
/// updates): the latest pull's, none under the least.
pub(super) fn draw(rules: &Bowing) -> Option<f32> {
    Pull::share(STATE.locked().fresh(Instant::now())?.pull, &rules.pull)
}

pub(super) fn posed<G: Bows>(rules: &Bowing, placed: bool, reticle: Option<Vec3>, string: bool) {
    let Some(model) = G::model(LEFT_HAND).filter(|_| G::held(LEFT_HAND) == Kind::Bow) else { return };
    note_held();
    let drawn = drawing::<G>();
    let fingers = G::string_point().filter(|p| p.iter().all(|c| c.is_finite()));
    let set_local = |element: i32, matrix: &Mat34| G::set_local(&model, element, matrix);
    let hang = match G::KEEPS {
        Keeps::World => Hang::World,
        Keeps::Local => Hang::Local { root: placed.then(left_holder).flatten(), set_local: &set_local },
    };
    if place(&mut STATE.locked(), rules, &model, &hang, reticle, drawn, fingers, string, Instant::now()) {
        PLACED.fetch_add(1, Relaxed);
    }
}

/// The left holder as the hand rig placed it at its latest posing.
fn left_holder() -> Option<Mat34> {
    crate::hand_rig::posed().and_then(|posed| posed.holders[LEFT_HAND])
}

/// One posing of the bow `model` at `now`, held as `hang` says: drawn or not, the string hand at
/// `fingers`. Whether the string was placed.
#[allow(clippy::too_many_arguments)]
pub(super) fn place(state: &mut State, rules: &Bowing, model: &impl Skeleton, hang: &Hang, reticle: Option<Vec3>, drawn: bool, fingers: Option<Vec3>, string: bool, now: Instant) -> bool {
    // `frame`: the root as the model's elements are read now; `root`: where it is as drawn.
    let Some(mut frame) = model.world(0) else { return false };
    let holder = match hang {
        Hang::Local { root: Some(root), .. } => *root,
        _ => frame,
    };
    let mut root = holder;
    let known = &mut state.known;
    if known.model != model.id() {
        let nock = rules.nock_in(model);
        *known = Known {
            model: model.id(),
            string: Bowing::element(model, rules.string),
            nock,
            bow: Bowing::element(model, rules.bow),
            arrow: nock.and_then(|nock| nock_axis(model, nock)),
            bow_at_rest: None,
            start: None,
        };
        log!("bow by hand: the string drawn along the bow (string {:?}, nock {:?}, bow {:?}; the arrow along the nock's axis {:?})", known.string, known.nock, known.bow, known.arrow);
        if known.string.is_none() || known.bow.is_none() || known.arrow.is_none() {
            log_elements(model);
        }
    }
    let Some(arrow) = known.arrow else { return false };
    // The nock and the string as the game has them, in the root's terms (before the bow turns).
    let nock_now = known.nock.and_then(|e| model.world(e)).and_then(|m| in_frame(&frame, &m));
    let string_now = known.string.and_then(|e| model.world(e)).and_then(|m| in_frame(&frame, &m));
    // The bow turned in the hand (about its grip) so the arrow points at the viewer's reticle.
    let nock_line = known.start.and_then(|s| s.nock).or(nock_now);
    let mut turned_bow = false;
    if let Some(turned) = reticle.and_then(|r| nock_line.and_then(|local| arrow_line(&root, &local, arrow)).and_then(|(at, way)| aligned(&root, way, sub(r, at), rules.most_turn))) {
        match hang {
            Hang::World => {
                model.set_world(0, &turned);
                frame = turned;
            }
            Hang::Local { .. } => turned_bow = true,
        }
        root = turned;
    }
    let grip = [root[3], root[7], root[11]];
    // Where an element placed now goes: in the world, or against its parent (`parent`, in the
    // root's terms as drawn: where it was placed this posing, else as the model has it).
    let place_at = |element: i32, world: &Mat34, parent: Option<Mat34>| match hang {
        Hang::World => model.set_world(element, world),
        Hang::Local { set_local, .. } => {
            let parent = parent.or_else(|| model.world(model.parent(element)).and_then(|m| in_frame(&frame, &m)));
            if let Some(local) = in_frame(&holder, world).and_then(|at| in_frame(&parent?, &at)) {
                set_local(element, &local);
            }
        }
    };
    if !drawn {
        known.start = None;
        known.bow_at_rest = known.bow.and_then(|e| model.world(e)).and_then(|m| in_frame(&frame, &m));
        // Held against its parent, the bow bone carries the turn.
        if turned_bow && let (Some(e), Some(local)) = (known.bow, known.bow_at_rest) {
            place_at(e, &out_of_frame(&root, &local), None);
        }
        let axis = nock_now.and_then(|local| arrow_line(&root, &local, arrow)).map(|(_, way)| way);
        // The pull of the draw is kept past its end: the bow leaves the drawing state as it is let
        // go, a moment before it looses.
        let kept = state.fresh(now);
        state.bow = kept.or_else(|| axis.map(|axis| Bow { grip, axis, pull: 0.0, at: now }));
        return false;
    }
    // The draw's start: the nock and the string where the game has them, in the bow's terms.
    if known.start.is_none() {
        known.start = Some(Start { nock: nock_now, string: string_now });
    }
    let Some(start) = known.start else { return false };
    let Some(local) = start.nock else { return false };
    let nock = out_of_frame(&root, &local);
    let Some((from, axis)) = arrow_line(&root, &local, arrow) else { return false };
    let pull = fingers.map_or(0.0, |f| Pull::back(from, axis, f, &rules.pull));
    // The bow stays in the hand as it rests there (the game's draw may move it), then the nock and
    // the string go straight back along the arrow from where the draw began.
    let mut bow_placed = None;
    if let (Some(e), Some(local)) = (known.bow, known.bow_at_rest) {
        let world = out_of_frame(&root, &local);
        place_at(e, &world, None);
        bow_placed = Some((e, world));
    }
    let (nock_element, string_element) = (known.nock, known.string);
    state.bow = Some(Bow { grip: from, axis, pull, at: now });
    if !string {
        return false;
    }
    let back = |m: Mat34| {
        let mut m = m;
        for k in 0..3 {
            m[k * 4 + 3] -= axis[k] * pull;
        }
        m
    };
    // The bow bone's place this posing, as the parent of what hangs from it.
    let parent_of = |element: i32| bow_placed.filter(|&(e, _)| e == model.parent(element)).and_then(|(_, world)| in_frame(&holder, &world));
    if let Some(e) = nock_element {
        place_at(e, &back(nock), parent_of(e));
    }
    if let (Some(e), Some(local)) = (string_element, start.string) {
        place_at(e, &back(out_of_frame(&root, &local)), parent_of(e));
    }
    true
}

/// A bow model whose string, bow bone or nock was not found: its elements logged.
fn log_elements(model: &impl Skeleton) {
    let root = model.world(0);
    for e in 0..model.count().min(LOGGED_ELEMENTS) {
        let place = model.world(e).zip(root).and_then(|(m, r)| in_frame(&r, &m)).map(|m| [m[3], m[7], m[11]]);
        log!("bow by hand: bow element [{e}] {} parent {} at {place:.3?} from the root", model.name(e), model.parent(e));
    }
}

/// The model's elements logged when one of the bow's is not found.
const LOGGED_ELEMENTS: i32 = 48;

/// Where the arrow is and which way it points (unit), from the nock `local` in `root`'s terms and
/// the nock's axis the arrow lies along with which way along it is forward (`arrow`).
pub(super) fn arrow_line(root: &Mat34, local: &Mat34, (axis, sign): (usize, f32)) -> Option<(Vec3, Vec3)> {
    let nock = out_of_frame(root, local);
    let column = [nock[axis], nock[4 + axis], nock[8 + axis]];
    let length = math::length(column);
    (length.is_finite() && length > 1e-6).then(|| ([nock[3], nock[7], nock[11]], column.map(|c| c * sign / length)))
}

/// `root` turned about its own position so `arrow` (unit) points along `toward`; none when they
/// are further apart than `most_turn` degrees (the bow then is not being aimed).
pub(super) fn aligned(root: &Mat34, arrow: Vec3, toward: Vec3, most_turn: f32) -> Option<Mat34> {
    let length = math::length(toward);
    if !length.is_finite() || length <= 1e-6 {
        return None;
    }
    let (from, to) = (arrow, toward.map(|c| c / length));
    if dot(from, to) < most_turn.to_radians().cos() {
        return None;
    }
    let column = |c: usize| math::turned([root[c], root[4 + c], root[8 + c]], from, to);
    let [a, b, c] = [column(0), column(1), column(2)];
    Some([a[0], b[0], c[0], root[3], a[1], b[1], c[1], root[7], a[2], b[2], c[2], root[11]])
}

/// `m` in `frame`'s own terms (its 3x3 part may scale): `frame` applied to it gives `m` back.
pub(super) fn in_frame(frame: &Mat34, m: &Mat34) -> Option<Mat34> {
    let columns = [0, 1, 2].map(|c| unapply_3x3(frame, [m[c], m[4 + c], m[8 + c]]));
    let at = unapply_3x3(frame, sub([m[3], m[7], m[11]], [frame[3], frame[7], frame[11]]))?;
    let [x, y, z] = [columns[0]?, columns[1]?, columns[2]?];
    Some([x[0], y[0], z[0], at[0], x[1], y[1], z[1], at[1], x[2], y[2], z[2], at[2]])
}

/// `local` (from [`in_frame`]) put back through `frame`.
pub(super) fn out_of_frame(frame: &Mat34, local: &Mat34) -> Mat34 {
    let column = |c: usize| apply_3x3(frame, [local[c], local[4 + c], local[8 + c]]);
    let [x, y, z] = [column(0), column(1), column(2)];
    let at = apply_3x3(frame, [local[3], local[7], local[11]]);
    [x[0], y[0], z[0], frame[3] + at[0], x[1], y[1], z[1], frame[7] + at[1], x[2], y[2], z[2], frame[11] + at[2]]
}

pub(super) fn summary() -> String {
    let (n, placed) = (ARROWS.load(Relaxed), PLACED.load(Relaxed));
    lines(&[
        if n > 0 { format!("bow by hand: {n} arrows loosed by hand") } else { String::new() },
        if placed > 0 { format!("bow by hand: the string drawn along the bow {placed} times") } else { String::new() },
    ])
}
