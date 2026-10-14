use super::*;

#[test]
fn it_keeps_the_last_thirty_seconds() {
    let mut recorder = Recorder::new(None);
    for second in 0..40u64 {
        recorder.push(&Frame { sequence: second, tick: second * 1000, ..Frame::default() });
    }
    assert_eq!(recorder.seconds(), 30.0);
    assert_eq!(recorder.frames.front().map(|(tick, _)| *tick), Some(9000));
    assert!(recorder.save().is_err(), "no folder");
    recorder.clear();
    assert_eq!(recorder.seconds(), 0.0);
}
