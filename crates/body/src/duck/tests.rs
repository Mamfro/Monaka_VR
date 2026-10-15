use super::*;

#[test]
fn the_crouch_is_asked_as_the_head_crosses_the_band_and_only_then() {
    let mut ducking = Ducking::new();
    ducking.start(|_| unreachable!("standing: nothing to ask"));
    let mut duck = Duck::new();
    let heights = [1.7, 1.6, 1.4, 1.3, 1.3, 1.45, 1.5, 1.6, 1.2, 1.25, f32::NAN, 1.7];
    let mut asks = Vec::new();
    for (i, &height) in heights.iter().enumerate() {
        let time = 0.05 * i as f64;
        let crouched = if height.is_finite() { duck.feed(time, height, &RULES) } else { duck.crouched() };
        ducking.feed(time, height, &RULES, |on| {
            asks.push((i, on));
            true
        });
        assert_eq!(ducking.asked(), crouched, "{i}");
    }
    assert_eq!(asks, [(3, true), (6, false), (8, true), (11, false)]);
}

#[test]
fn an_ask_not_taken_is_asked_again_and_none_while_off() {
    let mut ducking = Ducking::new();
    let mut asks = 0;
    ducking.feed(0.0, 1.7, &RULES, |_| unreachable!());
    ducking.feed(0.05, 1.2, &RULES, |_| unreachable!("off: nothing asked"));
    ducking.start(|on| {
        asks += 1;
        !on
    });
    ducking.feed(0.15, 1.2, &RULES, |on| {
        asks += 1;
        on
    });
    assert_eq!((asks, ducking.asked()), (2, true), "not taken: asked again");
    ducking.feed(0.2, 1.2, &RULES, |_| unreachable!("already asked"));
    let mut let_go = None;
    ducking.stop(|on| {
        let_go = Some(on);
        true
    });
    assert_eq!((let_go, ducking.asked()), (Some(false), false), "let go at the stop");
}
