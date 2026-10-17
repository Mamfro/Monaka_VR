use super::*;
use monaka_contract::manifest::{AIM_MOTION, AIM_MOUSE, EYE_SEPARATION, FINGER_TRACKING, aim, aims};

struct Toy;

const EYES: Component<Toy> = Component { name: "eyes", features: &[Feature::Stereo], options: &[EYE_SEPARATION], ..Component::NONE };
const HAND_RIG: Component<Toy> = Component {
    name: "hand rig",
    features: &[Feature::HandRig],
    options: &[aim(&[AIM_MOTION, AIM_MOUSE], aims::HAND_RIG)],
    on: |run| On::when(run.is_on(&EYES), "no eyes"),
    ..Component::NONE
};
const FINGERS: Component<Toy> = Component { name: "fingers", features: &[Feature::FingerRig], options: &[FINGER_TRACKING], games: &["toy"], ..Component::NONE };
const MINE: Component<Toy> = Component { name: "mine", options: &[opt("own", "Own", "The game's own.", Kind::Switch { default: true })], ..Component::NONE };

impl Game for Toy {
    const MANIFESTS: &'static [Manifest] = &[];
    const ADAPTER: Adapter<Toy> = Adapter { components: &[&EYES, &HAND_RIG, &FINGERS, &MINE] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> {
        Ok(())
    }
}

const TOY: [Opt; options_len::<Toy>("toy")] = options::<Toy, { options_len::<Toy>("toy") }>("toy");
const OTHER: [Opt; options_len::<Toy>("other")] = options::<Toy, { options_len::<Toy>("other") }>("other");

#[test]
fn a_game_s_features_and_options_are_its_components() {
    assert_eq!(features::<Toy>("toy"), Features::NONE.with_all(&[Feature::Stereo, Feature::HandRig, Feature::FingerRig]));
    let ids: Vec<&str> = TOY.iter().map(|o| o.id).collect();
    assert_eq!(ids, ["eye_separation", "aim", "finger_tracking", "own"], "in component order");
    let other: Vec<&str> = OTHER.iter().map(|o| o.id).collect();
    assert_eq!(other, ["eye_separation", "aim", "own"], "a component serving one game only is in that game's");
    assert!(!features::<Toy>("other").has(Feature::FingerRig));
}

// ---- The run: a toy game whose components write down what they are asked to do ----

static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static MISSING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FATAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static REFUSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TICKS: AtomicU64 = AtomicU64::new(0);

fn call(what: &str) {
    CALLS.lock().unwrap().push(what.to_owned());
}

fn called(what: &str) -> Result<(), Lack> {
    call(what);
    Ok(())
}

fn calls() -> Vec<String> {
    std::mem::take(&mut *CALLS.lock().unwrap())
}

struct Lived;

const LIVED_GAME: Manifest = Manifest { id: "lived", options: &[], options_file: "lived-options.txt", ..monaka_contract::manifest::MONAKA };

/// A game setting that notes when it is put back (to 0).
struct Noted(&'static str);

impl crate::ledger::Setting for Noted {
    type Value = u32;
    fn name(&self) -> String {
        self.0.to_owned()
    }
    fn read(&self) -> Option<u32> {
        Some(0)
    }
    fn write(&self, value: &u32) -> bool {
        if *value == 0 {
            call(&format!("{} setting back", self.0));
        }
        true
    }
}

const VIDEO: Component<Lived> = Component {
    name: "video",
    install: |start| {
        start.ledger.set(Noted("video"), 1);
        called("video install")
    },
    arm: || call("video arm"),
    every: &[Every::millis(0, |_| {
        TICKS.fetch_add(1, Relaxed);
    })],
    quiet: || call("video quiet"),
    give_back: || call("video give back"),
    release: || call("video release"),
    ..Component::NONE
};
const HANDS: Component<Lived> = Component {
    name: "hands",
    features: &[Feature::HandRig],
    needs: &[Need::On(&VIDEO)],
    install: |_| {
        call("hands install");
        if MISSING.load(Relaxed) { Err(Lack::Missing("no arms in this build".to_owned())) } else { Ok(()) }
    },
    arm: || call("hands arm"),
    frame: Some(|_| call("hands frame")),
    quiet: || call("hands quiet"),
    give_back: || call("hands give back"),
    let_go: Some(|| if REFUSE.load(Relaxed) { Err(Rejection::unsupported("still held")) } else { Ok(()) }),
    release: || call("hands release"),
    ..Component::NONE
};
const BROKEN: Component<Lived> = Component {
    name: "broken",
    on: |_| On::when(FATAL.load(Relaxed), "not this time"),
    install: |_| Err(Lack::Fatal(Rejection::revision("a hook would not go in"))),
    ..Component::NONE
};

impl Game for Lived {
    const MANIFESTS: &'static [Manifest] = &[LIVED_GAME];
    const ADAPTER: Adapter<Lived> = Adapter { components: &[&VIDEO, &HANDS, &BROKEN] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> {
        Ok(())
    }
}

const VIDEO_AT: usize = place::<Lived>(&VIDEO);
const HANDS_AT: usize = place::<Lived>(&HANDS);
const BROKEN_AT: usize = place::<Lived>(&BROKEN);

/// One test, the scenarios in turn: the run's state is the process's own.
#[test]
fn a_run_starts_in_order_stops_in_phases_and_undoes_a_failed_start() {
    let dir = std::env::temp_dir().join(format!("monaka-lifecycle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let session = Session::open_in(dir.clone(), "lifecycle.log").unwrap();

    // A whole run: installed and armed in order, each frame to those on, the stop in phases
    // over them in reverse; a component's settings go back at its own give-back, so they
    // outlast the give-backs of those after it.
    start::<Lived>(session).unwrap();
    frame::<Lived>(&());
    assert!(on::<Lived>(HANDS_AT) && !on::<Lived>(BROKEN_AT));
    // Timed work ticks while the run is on, and not after the stop.
    let waited = Instant::now();
    while TICKS.load(Relaxed) == 0 && waited.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(TIMER_STEP);
    }
    assert!(TICKS.load(Relaxed) > 0, "the timer ticks");
    stop::<Lived>().unwrap();
    let ticked = TICKS.load(Relaxed);
    std::thread::sleep(TIMER_STEP * 3);
    assert_eq!(TICKS.load(Relaxed), ticked, "no ticks after the stop");
    assert_eq!(
        calls(),
        [
            "video install", "hands install", "video arm", "hands arm", "hands frame", "hands quiet", "video quiet", "hands give back", "video setting back",
            "video give back", "hands release", "video release",
        ]
    );
    let features = std::fs::read_to_string(dir.join("features.txt")).unwrap();
    assert!(features.contains("hand rig: on"), "{features}");
    let log = dir.join("lifecycle.log");
    assert!(crate::wait_until(1000, || std::fs::read_to_string(&log).is_ok_and(|text| text.contains("features: "))), "the writer thread writes the log");
    frame::<Lived>(&());
    assert!(calls().is_empty(), "no frame work after the stop");

    // This build lacks the hands: they stay off with the reason, the run goes on.
    MISSING.store(true, Relaxed);
    start::<Lived>(session).unwrap();
    assert!(!on::<Lived>(HANDS_AT) && on::<Lived>(VIDEO_AT));
    stop::<Lived>().unwrap();
    MISSING.store(false, Relaxed);
    assert_eq!(calls(), ["video install", "hands install", "video arm", "video quiet", "video setting back", "video give back", "video release"]);
    let features = std::fs::read_to_string(dir.join("features.txt")).unwrap();
    assert!(features.contains("hand rig: lost (no arms in this build)"), "{features}");

    // A component that has not let go refuses: the stop ends there, hooks kept, and can be tried
    // again; the components it quieted stay off meanwhile.
    start::<Lived>(session).unwrap();
    REFUSE.store(true, Relaxed);
    assert!(stop::<Lived>().is_err());
    assert!(!on::<Lived>(HANDS_AT) && !on::<Lived>(VIDEO_AT), "quieted: off");
    REFUSE.store(false, Relaxed);
    stop::<Lived>().unwrap();
    let tried = calls();
    assert_eq!(&tried[4..7], ["hands quiet", "video quiet", "hands give back"], "{tried:?}");
    assert_eq!(&tried[7..], ["hands quiet", "video quiet", "hands give back", "video setting back", "video give back", "hands release", "video release"]);

    // An install that cannot go in refuses the start, and what installed before it is undone.
    FATAL.store(true, Relaxed);
    assert!(start::<Lived>(session).is_err());
    FATAL.store(false, Relaxed);
    assert_eq!(calls(), ["video install", "hands install", "hands release", "video setting back", "video release"]);
    assert!(!on::<Lived>(VIDEO_AT));

    // The copy done, its log thread ends; a line logged after that is on the disk at once.
    session.close();
    session.log(format_args!("after the close"));
    assert!(std::fs::read_to_string(&log).unwrap().contains("after the close"));
}
