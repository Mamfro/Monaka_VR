use super::*;

/// The tests that steer, one at a time: the look's state is one for the run.
static SERIAL: Mutex<()> = Mutex::new(());

/// A test's turn at steering, the shares recorded before it forgotten.
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    let turn = SERIAL.locked();
    *BAKED.locked() = Baked::EMPTY;
    turn
}

#[test]
fn shares_are_found_by_frame_else_the_last_by_frame() {
    let mut baked = Baked::EMPTY;
    let share = |yaw: f32| Share { yaw, pitch: -yaw };
    assert_eq!(baked.at(3), Share::default(), "nothing recorded");
    for frame in 1..=10 {
        baked.record(Some(frame), Some(share(frame as f32)));
    }
    assert_eq!(baked.at(7), share(7.0));
    assert_eq!(baked.at(1), share(10.0), "evicted frames fall back to the last recorded by frame");
    baked.record(None, Some(share(20.0)));
    assert_eq!((baked.latest, baked.at(99)), (Some(share(20.0)), share(10.0)), "the latest without a frame is not one by frame");
    assert_eq!(baked.misses, 3);
    baked.record(Some(30), None);
    assert_eq!((baked.latest, baked.at(30)), (None, share(10.0)), "a frame that carries none is the latest only");
}
