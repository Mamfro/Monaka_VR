//! The same-frame loop against the code it replaced (Dying Light 2's own pair publishing and the
//! Wolfensteins' capture), step by step.

use super::*;

/// What happened to one pair at its presents: whether the left eye's present was dispatched and
/// copied, and whether the right eye's was.
#[derive(Clone, Copy, Debug)]
struct PairPresents {
    left: bool,
    right: bool,
}

/// Every run of up to five pairs, each with its left and right present there or not.
fn runs() -> Vec<Vec<PairPresents>> {
    let one = [(false, false), (true, false), (false, true), (true, true)].map(|(left, right)| PairPresents { left, right });
    let mut runs = vec![Vec::new()];
    for _ in 0..5 {
        runs = runs.into_iter().flat_map(|run| one.iter().map(move |p| [run.clone(), vec![*p]].concat())).collect();
    }
    runs
}

/// The old way: the engine side numbered each pair before its eyes were presented; each eye copy
/// carried that number, and a pair went out when both copies carried the same one.
fn published_before(run: &[PairPresents]) -> Vec<bool> {
    let (mut copies, mut pairs) = ([None; 2], 0u64);
    run.iter()
        .map(|p| {
            pairs += 1;
            if p.left {
                copies[0] = Some(pairs);
            }
            p.right && {
                copies[1] = Some(pairs);
                copies == [Some(pairs); 2]
            }
        })
        .collect()
}

/// The game's pair number, as its frame labels give it to the loop.
static GAME_PAIR: AtomicU64 = AtomicU64::new(0);

/// The loop: each eye's copy carries the pair the frame labels say ([`Settings::pair`]).
fn published_now(run: &[PairPresents]) -> Vec<bool> {
    let settings = Settings { output: Output::D3D11, presenting: None, pair: Some(|| GAME_PAIR.load(Relaxed)), ready: |_, _| true, queue: None, report: None, hud: None, flat: None, hide_left: || true, rate: None, paused: None, mono: || false };
    let loops_own = AtomicU64::new(0);
    GAME_PAIR.store(0, Relaxed);
    let mut copies = [None; 2];
    run.iter()
        .map(|p| {
            GAME_PAIR.fetch_add(1, Relaxed);
            if p.left {
                copies[0] = Some(settings.pair(0, &loops_own));
            }
            p.right && {
                let pair = settings.pair(1, &loops_own);
                copies[1] = Some(pair);
                copies == [Some(pair); 2]
            }
        })
        .collect()
}

#[test]
fn pairs_go_out_when_they_went_out_before() {
    let runs = runs();
    for run in &runs {
        assert_eq!(published_now(run), published_before(run), "{run:?}");
    }
    assert_eq!(runs.len(), 4usize.pow(5));
}

/// The old D3D11 present of the game's window: the left eye's present (dispatch eye 1) hidden
/// unless shown by a debug flag; a frame of no eye (dispatch eye 0) published mono without a pair
/// in the last 250 ms.
fn dxgi_before(swap_is_dispatched: bool, dispatch_eye: u32, show_left: bool, since_pair_ms: u64) -> d3d11::Shown {
    if swap_is_dispatched && dispatch_eye == 1 && !show_left {
        return d3d11::Shown::Hidden;
    }
    if dispatch_eye == 0 && since_pair_ms > 250 {
        return d3d11::Shown::Flat;
    }
    d3d11::Shown::AsIs
}

#[test]
fn the_d3d11_present_hides_and_publishes_as_before() {
    for dispatched in [false, true] {
        for dispatch_eye in 0..=2u32 {
            for show_left in [false, true] {
                for since in [0, 100, 250, 251, 5000] {
                    // The frame labels: the eye being dispatched (0 left), none outside a pair.
                    let eye = (1..=2).contains(&dispatch_eye).then(|| (dispatch_eye - 1) as usize);
                    let now = d3d11::shown(dispatched, eye, !show_left, since <= RECENT_PAIR_MS);
                    assert_eq!(now, dxgi_before(dispatched, dispatch_eye, show_left, since), "{dispatched} {dispatch_eye} {show_left} {since}");
                }
            }
        }
    }
}

/// The old pause: stereo off, a new history epoch asked for, resumed no sooner than 100 ms later,
/// while publishing and not stopped for the run (and not mono for a check).
#[derive(Default)]
struct PausedBefore {
    active: bool,
    blocked: bool,
    until: u64,
}

impl PausedBefore {
    fn suspend(&mut self, now: u64) {
        self.active = false;
        self.until = now + 100;
    }

    fn update(&mut self, now: u64, publishing: bool) {
        if !self.active && !self.blocked && now >= self.until && publishing {
            self.active = true;
        }
    }
}

#[test]
fn stereo_pauses_and_resumes_as_before() {
    // One step a frame: (ms since the start, what happens: 0 a frame, 1 a pause, 2 stopped for the
    // run, 3 the stop), publishing until the stop.
    let steps: [(u64, u8); 12] = [(0, 0), (5, 0), (10, 1), (40, 0), (109, 0), (110, 0), (120, 1), (220, 0), (230, 0), (300, 2), (900, 0), (950, 3)];
    let mut before = PausedBefore::default();
    let pairs = Pairs::new();
    pairs.capture.store(true, Release);
    let start = monaka_channel::tick();
    let mut publishing = true;
    for (at, what) in steps {
        // The loop's clock is the system's: its pause is measured from now.
        let now = start + at;
        match what {
            1 => {
                before.suspend(now);
                pairs.running.store(false, Release);
                pairs.resume_at.store(now + PAUSE_MS, Release);
            }
            2 => {
                before.active = false;
                before.blocked = true;
                pairs.stop_for_run("a check");
            }
            3 => {
                publishing = false;
                pairs.capture.store(false, Release);
                pairs.halt();
            }
            _ => {}
        }
        before.update(now, publishing);
        // The loop's resume, at `now`.
        if !pairs.running() && !pairs.blocked.load(Acquire) && now >= pairs.resume_at.load(Acquire) && pairs.publishing() {
            pairs.running.store(true, Release);
        }
        assert_eq!(pairs.running(), before.active, "at {at} ms ({what})");
    }
}

/// The old capture's flat frames: a video playing (watched, in the side-by-side layout).
fn flat_before(side_by_side: bool, video_watched: bool, playing: bool) -> bool {
    side_by_side && video_watched && playing
}

#[test]
fn an_opengl_frame_goes_flat_when_it_did() {
    for side_by_side in [false, true] {
        for watched in [false, true] {
            for playing in [false, true] {
                // The game reports the video as the old capture computed it, into the situations.
                let reported = flat_before(side_by_side, watched, playing);
                let mut situations = monaka_core::situation::Situations::new();
                situations.report(monaka_core::situation::Situation::Video, reported, 0, 500);
                let flat = Flat::of_frame(None, None, &situations, 1);
                let now = side_by_side && flat.is_some();
                assert_eq!(now, flat_before(side_by_side, watched, playing));
            }
        }
    }
}

/// The Wolfensteins' own tickets before: a frame's ticket issued at its view build, taken at its
/// swap, the oldest not shown being drawn.
struct TicketsBefore(Tickets);

impl TicketsBefore {
    fn built(&mut self, pose: HeadPose) {
        self.0.issue(View::Both, pose, ());
    }

    fn shown(&mut self) -> (Option<HeadPose>, usize) {
        let shown = self.0.show();
        (shown.ticket.map(|ticket| ticket.pose), shown.waiting)
    }

    fn rendering(&self) -> Option<HeadPose> {
        self.0.drawing().map(|ticket| ticket.pose)
    }
}

#[test]
fn an_opengl_pair_goes_out_with_the_pose_it_went_out_with() {
    let pairs = Pairs::new();
    let mut before = TicketsBefore(Tickets::new(4));
    // Each step a view build (`b`) or a swap (`s`): in turn, a frame of lag, two, a swap with no
    // frame built, more in flight than the pairing holds, then back in turn.
    let steps = "bsbsbbsbssbsssbbbbbbsssbsbsbbs";
    for (i, step) in steps.chars().enumerate() {
        if step == 'b' {
            let pose = HeadPose::synthetic(i as f32 * 3.0, -(i as f32), HeadPose::SYNTHETIC_FOV, 0.064);
            pairs.built(View::Both, pose);
            before.built(pose);
        } else {
            assert_eq!(pairs.shown(), before.shown(), "the pose at swap {i}");
        }
        assert_eq!(pairs.rendering(), before.rendering(), "the pose being drawn after step {i}");
    }
}

// ---- Dying Light 2's own pair counting, eye turns and stage timings, now the loop's ----

/// The old pair counter: each pair started on the engine side took the next number from 1.
#[test]
fn pairs_are_numbered_as_the_engine_side_numbered_them() {
    let pairs = Pairs::new();
    let before = AtomicU64::new(0);
    for _ in 0..5 {
        assert_eq!(pairs.start_pair(), before.fetch_add(1, Relaxed) + 1);
    }
}

/// The old turns: the next eye (1 left, 2 right) and the head kept for the right eye; a right eye
/// drawn from the head its left eye was, a left eye from the current head (kept when there is one).
struct TurnsBefore {
    next: u32,
    head: Option<HeadPose>,
}

impl TurnsBefore {
    fn frame(&mut self, current: Option<HeadPose>) -> (u32, Option<HeadPose>) {
        let next = self.next;
        if next == 2 {
            return (next, self.head);
        }
        if current.is_some() {
            self.head = current;
        }
        (next, current)
    }
}

#[test]
fn the_eyes_take_turns_as_dying_light_2s_alternate_eye_did() {
    let pose = |yaw: f32| Some(HeadPose::synthetic(yaw, 0.0, HeadPose::SYNTHETIC_FOV, 0.064));
    // Each frame: the current head, whether the eye's frame went out, whether stereo paused after.
    let frames = [(pose(1.0), true, false), (pose(2.0), true, false), (None, true, false), (pose(4.0), false, false), (pose(5.0), true, true), (None, false, false), (pose(7.0), true, false), (pose(8.0), true, false)];
    let (turns, mut before) = (Turns::new(), TurnsBefore { next: 1, head: None });
    for (i, (current, drawn, paused)) in frames.into_iter().enumerate() {
        let (eye, head) = turns.frame(current);
        assert_eq!((eye as u32 + 1, head), before.frame(current), "frame {i}");
        if drawn {
            turns.drawn(eye);
            before.next = if before.next == 1 { 2 } else { 1 };
        }
        if paused {
            turns.start_over();
            before.next = 1;
        }
    }
}

/// The old timing line: each stage's average since the last line, the counts started afresh.
fn timing_before(counts: &[u64; 6], nanos: &[u64; 6]) -> String {
    const NAMES: [&str; 6] = ["mono scene", "left scene", "left drain", "right scene", "right drain", "frame interval"];
    let mut line = String::from("timing (average ms):");
    for (i, name) in NAMES.iter().enumerate() {
        if counts[i] > 0 {
            line += &format!(" {name} {:.2} (x{});", nanos[i] as f64 / counts[i] as f64 / 1e6, counts[i]);
        }
    }
    line
}

#[test]
fn the_stages_are_timed_as_dying_light_2_timed_them() {
    let timings = Timings::new();
    let (mut counts, mut nanos) = ([0u64; 6], [0u64; 6]);
    for (stage, ms) in [(Stage::LeftScene, 4.0), (Stage::LeftScene, 6.5), (Stage::RightDrain, 1.25), (Stage::MonoScene, 9.0)] {
        let took = Duration::from_secs_f64(ms / 1e3);
        timings.time(stage, took);
        counts[stage as usize] += 1;
        nanos[stage as usize] += took.as_nanos() as u64;
    }
    assert_eq!(timings.report(), timing_before(&counts, &nanos));
    assert_eq!(timings.report(), "", "started afresh: nothing timed since (the old line was its bare start)");
    // The spacing of the pairs: none for the first pair, one from each pair to the next.
    timings.pair_done();
    timings.pair_done();
    assert!(timings.report().contains("frame interval") && timings.report().is_empty());
}
