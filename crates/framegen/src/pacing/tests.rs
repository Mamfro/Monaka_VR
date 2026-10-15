use super::*;

fn record(published: &'static Mutex<Vec<&'static str>>, what: &'static str) {
    published.locked().push(what);
}

static PUBLISHED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn publish(what: &'static str) {
    record(&PUBLISHED, what);
}

#[test]
fn a_waiting_pair_goes_out_at_its_time_or_before_the_next_present_publishes() {
    let pacer = Pacer::start("pacing test", publish).unwrap();
    // The generated pair now, the real pair a little later: the thread publishes it.
    pacer.now_then(|| publish("generated 1"), Some((Instant::now() + Duration::from_millis(20), "real 1")));
    assert!(monaka_producer::wait_until(2000, || PUBLISHED.locked().len() == 2), "{:?}", PUBLISHED.locked());
    // A real pair still waiting when the next generated pair goes out goes first.
    pacer.now_then(|| publish("generated 2"), Some((Instant::now() + Duration::from_secs(60), "real 2")));
    pacer.now_then(|| publish("generated 3"), Some((Instant::now() + Duration::from_secs(60), "real 3")));
    // A flat frame: the waiting pair, then the frame.
    pacer.now_then(|| publish("flat"), None);
    drop(pacer);
    assert_eq!(*PUBLISHED.locked(), ["generated 1", "real 1", "generated 2", "real 2", "generated 3", "real 3", "flat"]);
}

static PACED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static PACING: Pacing<&'static str> = Pacing::new("pacing test", paced);

fn paced(what: &'static str) {
    record(&PACED, what);
}

#[test]
fn the_thread_starts_with_the_first_waiting_pair_and_a_stop_drops_what_waits() {
    // Nothing waits: no thread, the pair at once.
    PACING.now_then(|| paced("real 1"), None);
    assert!(PACING.pacer.locked().is_none());
    PACING.now_then(|| paced("generated 2"), Some((Instant::now() + Duration::from_secs(60), "real 2")));
    assert!(PACING.pacer.locked().is_some());
    PACING.now_then(|| paced("real 3"), None);
    PACING.now_then(|| paced("generated 4"), Some((Instant::now() + Duration::from_secs(60), "real 4")));
    PACING.stop();
    assert!(PACING.pacer.locked().is_none());
    PACING.now_then(|| paced("flat"), None);
    assert_eq!(*PACED.locked(), ["real 1", "generated 2", "real 2", "real 3", "generated 4", "flat"]);
}

fn ms(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

#[test]
fn the_real_pair_is_due_half_a_smoothed_interval_later() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut interval = Interval::new(Duration::from_millis(28));
    // The first pair: half the starting interval.
    assert!((ms(interval.due(at(0)) - at(0)) - 14.0).abs() < 0.01);
    // 20 ms later: 28 * 0.8 + 20 * 0.2 = 26.4.
    let due = interval.due(at(20));
    assert!((ms(interval.interval()) - 26.4).abs() < 0.01, "{:?}", interval.interval());
    assert!((ms(due - at(20)) - 13.2).abs() < 0.01);
    // A pause counts as 100 ms: 26.4 * 0.8 + 20 = 41.12.
    interval.due(at(5020));
    assert!((ms(interval.interval()) - 41.12).abs() < 0.01, "{:?}", interval.interval());
    // Half the interval is kept within 2 and 40 ms.
    let mut slow = Interval::new(Duration::from_millis(200));
    assert_eq!(slow.due(at(0)) - at(0), Duration::from_millis(40));
    let mut fast = Interval::new(Duration::from_millis(1));
    assert_eq!(fast.due(at(0)) - at(0), Duration::from_millis(2));
}
