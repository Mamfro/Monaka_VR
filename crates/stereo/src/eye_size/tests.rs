use super::*;
use monaka_contract::manifest::Manifest;
use monaka_core::options::Options;
use monaka_producer::game::Adapter;
use monaka_producer::{Rejection, Session};
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering::Relaxed};

struct Toy;

impl Game for Toy {
    const MANIFESTS: &'static [Manifest] = &[];
    const ADAPTER: Adapter<Toy> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> {
        Ok(())
    }
}

/// What the toy game answers when asked: 0 not yet, 1 asked, 2 cannot.
static ANSWER: AtomicU8 = AtomicU8::new(0);
static ASKED: AtomicU32 = AtomicU32::new(0);

impl Resizes for Toy {
    fn ask(_: (u32, u32), _: (u32, u32)) -> Ask {
        ASKED.fetch_add(1, Relaxed);
        match ANSWER.load(Relaxed) {
            0 => Ask::NotYet,
            1 => Ask::Asked,
            _ => Ask::Cannot("no settings".into()),
        }
    }

    fn put_back() {}
}

fn wanted(eye: (u32, u32)) {
    *GATE.locked() = Some(Gate { eye, asked: false, since: Instant::now(), presents: 0 });
    OPEN.store(false, Release);
}

#[test]
fn frames_wait_for_the_eye_size_once_the_game_is_asked() {
    assert!(ready::<Toy>((50, 50)), "no size wanted: frames go out");

    // Asked again at each present until the game can be asked, then not again.
    wanted((100, 100));
    ANSWER.store(0, Relaxed);
    assert!(!ready::<Toy>((50, 50)));
    ANSWER.store(1, Relaxed);
    assert!(!ready::<Toy>((50, 50)));
    assert!(!ready::<Toy>((50, 50)));
    assert_eq!(ASKED.load(Relaxed), 2);
    // At the size: frames go out from then on, whatever the size.
    assert!(ready::<Toy>((100, 100)));
    assert!(ready::<Toy>((50, 50)));

    // A game that cannot be asked renders at its own size, at once.
    wanted((100, 100));
    ANSWER.store(2, Relaxed);
    assert!(ready::<Toy>((50, 50)));
    assert!(ready::<Toy>((50, 50)));
    assert_eq!(ASKED.load(Relaxed), 3);
}
