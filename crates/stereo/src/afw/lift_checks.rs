//! The core against the two implementations it was lifted from, decision by decision: recorded runs
//! of frames (which eye, the poses, the depth conventions, pauses, missing inputs, a busy GPU, a
//! changed latency) give each backend the same real eye, the same views to warp from and into, the
//! same fill, the same poses and the same pairs going out as the old D3D11 code (Dying Light 1's)
//! and the old D3D12 code (The Beast's) gave, each under its own rules.

use super::*;
use crate::{afw11, afw12};
use monaka_core::camera::Frustum;

/// The recorded runs: a small deterministic generator.
struct Runs(u64);

impl Runs {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A projection of `convention` (0 standard, 1 reversed, 2 reversed infinite) with `near`.
fn projection(convention: u64, near: f32) -> Mat44 {
    let far = 2000.0;
    let mut p = [0.0f32; 16];
    (p[0], p[5], p[14]) = (0.92, 1.03, -1.0);
    (p[10], p[11]) = match convention {
        0 => (far / (near - far), near * far / (near - far)),
        1 => (near / (far - near), near * far / (far - near)),
        _ => (0.0, near),
    };
    p
}

/// The head at time `t` (presents): turning and walking, valid.
fn head(t: u64) -> HeadPose {
    let t = t as f32;
    let mut pose = HeadPose::synthetic((t * 0.7) % 360.0, 10.0 * (t * 0.05).sin(), HeadPose::SYNTHETIC_FOV, 0.064);
    pose.position = [0.01 * t, 1.7 + 0.02 * (t * 0.3).sin(), -0.005 * t];
    pose
}

/// The game camera of `pose`, the eye `eye` of it `separation` apart, far from the origin.
fn camera(pose: &HeadPose, eye: usize, separation: f32) -> Mat34 {
    let yaw = 2.0 * pose.orientation[1].atan2(pose.orientation[3]);
    let (s, c) = yaw.sin_cos();
    let centre = [c, 0.0, s, 1650.0 + pose.position[0], 0.0, 1.0, 0.0, 35.0 + pose.position[1], -s, 0.0, c, -820.0 + pose.position[2]];
    offset_right(&centre, if eye == 0 { -separation / 2.0 } else { separation / 2.0 })
}

// ---- D3D11: Dying Light 1 ----------------------------------------------------------------------

/// One present of a D3D11 run: what the eye site recorded after it (none while paused), whether
/// the frame it shows has its scene depth and its capture succeeds, whether the loop calls it flat.
#[derive(Clone, Copy, Debug)]
struct Present11 {
    n: u64,
    schedule: Schedule,
    recorded: Option<EyeCameras>,
    depth: bool,
    captures: bool,
    flat: bool,
}

/// What went out at a D3D11 present: the real eye, the view it was warped into, how it was
/// rendered, its fill (how that frame was rendered, and the present it was captured at), the pose.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pair11 {
    eye: usize,
    target: (Mat34, Mat44, DepthMapping),
    primary: (Mat34, Mat44),
    fill: Option<(Mat34, Mat44, u64)>,
    pose: HeadPose,
}

fn run11(runs: &mut Runs, presents: u64) -> Vec<Present11> {
    let mut latency = 2;
    let (mut paused, mut convention) = (0, runs.below(3));
    (1..=presents)
        .map(|n| {
            if runs.chance(1) {
                latency = 1 + runs.below(3);
            }
            if runs.chance(2) {
                paused = 1 + runs.below(40);
            }
            paused = paused.saturating_sub(1);
            if runs.chance(1) {
                convention = runs.below(3);
            }
            let pose = head(n);
            let near = 0.05 + 0.05 * convention as f32;
            let separation = 0.064;
            let recorded = (paused == 0 && !runs.chance(3)).then(|| {
                let projection = |e: usize| {
                    let mut p = projection(convention, near);
                    p[2] = if e == 0 { 0.05 } else { -0.05 };
                    p
                };
                let mapping = |e| DepthMapping::from_projection(&projection(e), near).expect("a perspective projection");
                EyeCameras { camera: [0, 1].map(|e| camera(&pose, e, separation)), projection: [projection(0), projection(1)], mapping: [mapping(0), mapping(1)], pose }
            });
            Present11 { n, schedule: Schedule { latency }, recorded, depth: !runs.chance(4), captures: !runs.chance(2), flat: paused > 0 || runs.chance(2) }
        })
        .collect()
}

/// The old D3D11 code's decisions (`Afw11::capture` and `Afw11::publish` as they were): each eye's
/// latest real frame with the whole record of its cameras and the present it came at.
#[derive(Default)]
struct Before11 {
    real: [Option<(EyeCameras, u64)>; 2],
    latest: Option<usize>,
    ring: PresentRing<EyeCameras>,
}

impl Before11 {
    fn present(&mut self, p: &Present11) -> Option<Pair11> {
        // The capture, once per frame.
        let eye = p.schedule.eye_at(p.n);
        let chosen_at = p.schedule.chosen_at(p.n);
        if p.depth
            && let Some(cameras) = self.ring.get(chosen_at)
            && eye == Schedule::camera_eye(chosen_at)
        {
            let _reuse = self.real[eye].take();
            if p.captures {
                self.real[eye] = Some((cameras, p.n));
                self.latest = Some(eye);
            } else {
                self.latest = None;
            }
        }
        // The publish.
        let published = (|| {
            let eye = self.latest.take()?;
            let other = 1 - eye;
            let (cameras, _) = self.real[eye]?;
            let fill = self.real[other].map(|(c, at)| (c.camera[other], c.projection[other], at));
            Some(Pair11 {
                eye,
                target: (cameras.camera[other], cameras.projection[other], cameras.mapping[other]),
                primary: (cameras.camera[eye], cameras.projection[eye]),
                fill,
                pose: cameras.pose,
            })
        })();
        // The eye site records after the present.
        if let Some(cameras) = p.recorded {
            self.ring.put(p.n, cameras);
        }
        published
    }
}

/// The D3D11 backend's decisions now: the core's, called as `afw11` calls them.
struct Now11 {
    history: History,
    presents: Presents,
    frames: [Option<u64>; 2],
    latest: Option<Shot>,
    ring: PresentRing<EyeCameras>,
}

impl Now11 {
    fn new() -> Self {
        Self { history: History::new(afw11::RULES), presents: Presents::new(afw11::RULES), frames: [None, None], latest: None, ring: PresentRing::new() }
    }

    fn present(&mut self, p: &Present11, now: Instant) -> Option<Pair11> {
        if p.depth
            && let Some(shot) = Shot::scheduled(p.schedule, p.n, &self.ring)
        {
            self.history.begin();
            let _reuse = self.frames[shot.eye].take();
            if p.captures {
                self.frames[shot.eye] = Some(p.n);
                self.history.keep(&shot);
                self.latest = Some(shot);
            } else {
                self.history.lose(shot.eye);
                self.latest = None;
            }
        }
        let shot = self.latest.take();
        let out = self.presents.out(p.flat, true, shot.is_some(), now);
        if out.forgets() {
            self.history.forget();
        }
        let published = match (out, shot) {
            (Out::Warped, Some(shot)) => {
                let warp = self.history.warp(&shot);
                let other = shot.other_eye();
                self.frames[shot.eye].map(|_| Pair11 {
                    eye: warp.eye,
                    target: (warp.target.camera, warp.target.projection, warp.target.mapping),
                    primary: (warp.primary.camera, warp.primary.projection),
                    fill: warp.fill.and_then(|r| Some((r.camera, r.projection, self.frames[other]?))),
                    pose: shot.pose,
                })
            }
            (out, _) => {
                assert_eq!(out, Out::Nothing, "warped pairs only on D3D11");
                None
            }
        };
        if let Some(cameras) = p.recorded {
            self.ring.put(p.n, cameras);
        }
        published
    }
}

#[test]
fn dying_light_1_gets_the_pairs_it_got() {
    let start = Instant::now();
    let (mut published, mut filled) = (0, 0);
    for seed in 1..=12 {
        let mut runs = Runs(seed);
        let run = run11(&mut runs, 3000);
        let (mut before, mut now) = (Before11::default(), Now11::new());
        for p in &run {
            let expected = before.present(p);
            assert_eq!(now.present(p, start + Duration::from_millis(11 * p.n)), expected, "seed {seed}, present {}", p.n);
            published += expected.is_some() as u32;
            filled += expected.is_some_and(|pair| pair.fill.is_some()) as u32;
        }
    }
    // The runs reach every case: pairs, fills, and long pauses whose old frames still fill.
    assert!(published > 10_000 && filled > 9_000, "{published} {filled}");
}

// ---- D3D12: The Beast --------------------------------------------------------------------------

/// One present of a D3D12 run: the label at the present request, the inputs (how the frame was
/// rendered, the game's reset), the eyes' separation and the other eye's frustum, what the GPU
/// does (sizes changed, no command list, the warp failing after its copy, frame generation's
/// list and streams), and at the loop's step: its present number, its label, flat, the newest
/// head pose, the time, and the plain pair's command list.
#[derive(Clone, Copy, Debug)]
struct Present12 {
    present: u64,
    label: Option<Pending>,
    inputs: Option<(Rendered, bool)>,
    separation: Option<f32>,
    frustum: Option<Frustum>,
    resized: bool,
    busy: bool,
    fails: bool,
    framegen: bool,
    generate_busy: bool,
    made: Option<[bool; 2]>,
    shown_at: u64,
    shown_label: Option<Pending>,
    flat: bool,
    current: HeadPose,
    millis: u64,
    plain_busy: bool,
}

/// What a D3D12 warp made: the real eye, the view it was warped into, how it was rendered, its
/// fill, both eyes' views.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Warped12 {
    eye: usize,
    target: Rendered,
    primary: Rendered,
    fill: Option<Rendered>,
    views: [Rendered; 2],
}

/// What went out at a D3D12 present.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Went {
    Flat,
    Waiting,
    BothEyes(HeadPose),
    Plain([HeadPose; 2]),
    Warped { eye: usize, pose: HeadPose },
    Generated(HeadPose),
    Real(HeadPose),
}

/// Everything decided at one D3D12 present: the warp, frame generation's (whether its history ran
/// on, the generated pose), what went out.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Decided12 {
    warped: Option<Warped12>,
    generation: Option<(bool, Option<HeadPose>)>,
    went: Went,
}

fn run12(runs: &mut Runs, presents: u64) -> Vec<Present12> {
    let (mut eye, mut millis) = (0usize, 0u64);
    let (mut flat, mut unlabelled, mut convention) = (0, 0, runs.below(3));
    let framegen = runs.chance(50);
    (1..=presents)
        .map(|n| {
            // Mostly the other eye; now and then the same eye twice.
            if !runs.chance(3) {
                eye = 1 - eye;
            }
            millis += if runs.chance(2) { 150 + runs.below(300) } else { 11 };
            if runs.chance(1) {
                flat = 1 + runs.below(30);
            }
            if runs.chance(1) {
                unlabelled = 1 + runs.below(40);
            }
            (flat, unlabelled) = (flat.saturating_sub(1), unlabelled.saturating_sub(1));
            if runs.chance(1) {
                convention = runs.below(3);
            }
            let pose = head(n);
            let label = (unlabelled == 0 && !runs.chance(2)).then_some(Pending { eye, pose });
            let near = 0.05;
            let rendered = Rendered::new(camera(&pose, eye, 0.064), projection(convention, near), near).expect("a rigid camera");
            let shown_label = if runs.chance(2) { None } else if runs.chance(1) { Some(Pending { eye: 1 - eye, pose }) } else { label };
            let made = match runs.below(10) {
                0 => None,
                1 => Some([true, false]),
                _ => Some([true, true]),
            };
            Present12 {
                present: n,
                label,
                inputs: (!runs.chance(4)).then_some((rendered, runs.chance(2))),
                separation: (!runs.chance(1)).then_some(0.064),
                frustum: runs.chance(50).then(|| Frustum::centred([1.1, 1.0]).expect("usable")),
                resized: runs.chance(1),
                busy: runs.chance(2),
                fails: runs.chance(1),
                framegen,
                generate_busy: runs.chance(2),
                made,
                shown_at: if runs.chance(2) { n + 1 } else { n },
                shown_label,
                flat: flat > 0,
                current: head(n + 1000),
                millis,
                plain_busy: runs.chance(3),
            }
        })
        .collect()
}

/// A prepared pair, as the old code kept it between the request and the step.
#[derive(Clone, Copy)]
struct Prepared12 {
    present: u64,
    eye: usize,
    pose: HeadPose,
    shown: bool,
    generated: Option<HeadPose>,
}

/// The old D3D12 code's decisions (`afw12`'s `warp`, `Afw12::warp`, `generate`, `publish` and
/// `plain` as they were).
#[derive(Default)]
struct Before12 {
    kept: [Option<(Rendered, u64)>; 2],
    presents: u64,
    prepared: Option<Prepared12>,
    previous: Option<(HeadPose, usize, u64)>,
    last_labelled: Option<Instant>,
    plain: [Option<(HeadPose, u64)>; 2],
}

const FILL_AGE: u64 = 4;

impl Before12 {
    fn warp(&mut self, p: &Present12) -> (Option<Warped12>, Option<(bool, Option<HeadPose>)>) {
        self.prepared = None;
        let Some(label) = p.label else { return (None, None) };
        let (eye, other) = (label.eye.min(1), 1 - label.eye.min(1));
        let Some((rendered, reset)) = p.inputs else { return (None, None) };
        let Some(separation) = p.separation else { return (None, None) };
        let other_view = rendered.other_eye(eye, separation, p.frustum);
        if p.resized {
            self.kept = [None, None];
        }
        if p.busy {
            return (None, None);
        }
        self.presents += 1;
        self.kept[eye] = Some((rendered, self.presents));
        if p.fails {
            return (None, None);
        }
        let fill = match self.kept[other] {
            Some((rendered, at)) if self.presents - at <= FILL_AGE => Some(rendered),
            _ => None,
        };
        let warped = Warped12 { eye, target: other_view, primary: rendered, fill, views: in_eye_order(eye, rendered, other_view) };
        let mut prepared = Prepared12 { present: p.present, eye, pose: label.pose, shown: false, generated: None };
        let generation = p.framegen.then(|| {
            let previous = self.previous.take();
            let continuous = previous.is_some_and(|(_, e, at)| e != eye && at + 1 == prepared.present) && !reset;
            if p.generate_busy {
                return (continuous, None);
            }
            prepared.shown = true;
            if p.made.is_some() {
                self.previous = Some((prepared.pose, eye, prepared.present));
            }
            if p.made == Some([true, true])
                && let Some((before, ..)) = previous
            {
                prepared.generated = Some(monaka_framegen::blend(&before, &prepared.pose, 0.5));
            }
            (continuous, prepared.generated)
        });
        self.prepared = Some(prepared);
        (Some(warped), generation)
    }

    fn forget(&mut self) {
        self.kept.iter_mut().for_each(|k| *k = None);
        self.previous = None;
    }

    fn publish(&mut self, p: &Present12, now: Instant) -> Went {
        let label = p.shown_label;
        let prepared = self.prepared.take().filter(|q| q.present == p.shown_at && label.is_some_and(|l| l.eye.min(1) == q.eye));
        if p.flat {
            self.forget();
            return Went::Flat;
        }
        let Some(label) = label else {
            if self.last_labelled.is_some_and(|t| (now - t).as_millis() < 200) {
                return Went::Waiting;
            }
            self.forget();
            return Went::BothEyes(p.current);
        };
        self.last_labelled = Some(now);
        let Some(prepared) = prepared else { return self.plain(p, label.eye.min(1), label.pose) };
        match (prepared.shown, prepared.generated) {
            (false, _) => Went::Warped { eye: prepared.eye, pose: prepared.pose },
            (true, Some(halfway)) => Went::Generated(halfway),
            (true, None) => Went::Real(prepared.pose),
        }
    }

    fn plain(&mut self, p: &Present12, eye: usize, pose: HeadPose) -> Went {
        self.plain[eye] = None;
        if p.plain_busy {
            return Went::Waiting;
        }
        self.plain[eye] = Some((pose, p.shown_at));
        match self.plain[1 - eye] {
            Some((other, at)) if p.shown_at.saturating_sub(at) <= 1 => Went::Plain(in_eye_order(eye, pose, other)),
            _ => Went::Waiting,
        }
    }
}

/// The D3D12 backend's decisions now: the core's, called as `afw12` calls them.
struct Now12 {
    history: History,
    presents: Presents,
    prepared: Option<Prepared12>,
}

impl Now12 {
    fn new() -> Self {
        Self { history: History::new(afw12::RULES), presents: Presents::new(afw12::RULES), prepared: None }
    }

    fn warp(&mut self, p: &Present12) -> (Option<Warped12>, Option<(bool, Option<HeadPose>)>) {
        self.prepared = None;
        let Some(label) = p.label else { return (None, None) };
        let other = 1 - label.eye.min(1);
        let Some((rendered, reset)) = p.inputs else { return (None, None) };
        let Some(separation) = p.separation else { return (None, None) };
        let shot = Shot::labelled(label, rendered, separation, p.frustum);
        if p.resized {
            self.history.forget();
        }
        if p.busy {
            return (None, None);
        }
        self.history.begin();
        self.history.keep(&shot);
        if p.fails {
            return (None, None);
        }
        let warp = self.history.warp(&shot);
        assert_eq!(warp.eye, 1 - other);
        let warped = Warped12 { eye: warp.eye, target: warp.target, primary: warp.primary, fill: warp.fill, views: shot.views() };
        let mut prepared = Prepared12 { present: p.present, eye: shot.eye, pose: shot.pose, shown: false, generated: None };
        let generation = p.framegen.then(|| {
            let (previous, continuous) = self.presents.generation(shot.eye, prepared.present, reset);
            if p.generate_busy {
                return (continuous, None);
            }
            prepared.shown = true;
            if p.made.is_some() {
                self.presents.generated(Previous { pose: prepared.pose, eye: shot.eye, present: prepared.present });
            }
            if p.made == Some([true, true])
                && let Some(before) = previous
            {
                prepared.generated = Some(halfway(&before.pose, &prepared.pose));
            }
            (continuous, prepared.generated)
        });
        self.prepared = Some(prepared);
        (Some(warped), generation)
    }

    fn publish(&mut self, p: &Present12, now: Instant) -> Went {
        let label = p.shown_label;
        let prepared = self.prepared.take().filter(|q| q.present == p.shown_at && label.is_some_and(|l| l.eye.min(1) == q.eye));
        let out = self.presents.out(p.flat, label.is_some(), prepared.is_some(), now);
        if out.forgets() {
            self.history.forget();
            self.presents.forget();
        }
        match (out, label, prepared) {
            (Out::Flat, ..) => Went::Flat,
            (Out::BothEyes, ..) => Went::BothEyes(p.current),
            (Out::Plain, Some(label), _) => {
                let eye = label.eye.min(1);
                self.presents.lose_plain(eye);
                if p.plain_busy {
                    return Went::Waiting;
                }
                match self.presents.plain(eye, label.pose, p.shown_at) {
                    Some(other) => Went::Plain(in_eye_order(eye, label.pose, other)),
                    None => Went::Waiting,
                }
            }
            (Out::Warped, _, Some(prepared)) => match (prepared.shown, prepared.generated) {
                (false, _) => Went::Warped { eye: prepared.eye, pose: prepared.pose },
                (true, Some(halfway)) => Went::Generated(halfway),
                (true, None) => Went::Real(prepared.pose),
            },
            _ => Went::Waiting,
        }
    }
}

#[test]
fn the_beast_gets_the_pairs_it_got() {
    let start = Instant::now();
    let mut seen = std::collections::HashMap::<&str, u32>::new();
    for seed in 1..=24 {
        let mut runs = Runs(seed * 7919);
        let run = run12(&mut runs, 3000);
        let (mut before, mut now) = (Before12::default(), Now12::new());
        for p in &run {
            let at = start + Duration::from_millis(p.millis);
            let (warped, generation) = before.warp(p);
            let expected = Decided12 { warped, generation, went: before.publish(p, at) };
            let (warped, generation) = now.warp(p);
            let decided = Decided12 { warped, generation, went: now.publish(p, at) };
            assert_eq!(decided, expected, "seed {seed}, present {}", p.present);
            let went = match expected.went {
                Went::Flat => "flat",
                Went::Waiting => "waiting",
                Went::BothEyes(_) => "both eyes",
                Went::Plain(_) => "plain",
                Went::Warped { .. } => "warped",
                Went::Generated(_) => "generated",
                Went::Real(_) => "real",
            };
            *seen.entry(went).or_default() += 1;
            if expected.warped.is_some_and(|w| w.fill.is_some()) {
                *seen.entry("filled").or_default() += 1;
            }
            if expected.generation.is_some_and(|(continuous, _)| continuous) {
                *seen.entry("continuous").or_default() += 1;
            }
        }
    }
    // The runs reach every case.
    for case in ["flat", "waiting", "both eyes", "plain", "warped", "generated", "real", "filled", "continuous"] {
        assert!(seen.get(case).is_some_and(|&n| n > 20), "{case}: {seen:?}");
    }
}

#[test]
fn the_report_line_reads_as_it_did() {
    let counters = Counters::new();
    for (counter, n) in [(&counters.no_inputs, 3), (&counters.plain, 4), (&counters.flat, 5), (&counters.generated, 6), (&counters.real, 7), (&counters.no_hud, 8)] {
        (0..n).for_each(|_| Counters::add(counter));
    }
    let warped = Counts { pairs: 10, filled: 9, busy: 2, failures: 1 };
    assert_eq!(
        report_line([11, 12], warped, &counters, 13, Some(14)),
        "frame warping: inputs L/R 11/12, warped pairs 10 (gaps from the eye's own frame 9), GPU busy 2, failed 1, frames without inputs 3, plain pairs 4, \
         flat 5, without the HUD 8, pairs with depth 13; in-between pairs 6, real pairs 7, eyes not generated 14"
    );
    assert_eq!(
        report_line([11, 12], warped, &counters, 0, None),
        "frame warping: inputs L/R 11/12, warped pairs 10 (gaps from the eye's own frame 9), GPU busy 2, failed 1, frames without inputs 3, plain pairs 4, \
         flat 5, without the HUD 8"
    );
}
