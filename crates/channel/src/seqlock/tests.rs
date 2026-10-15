use super::*;
use crate::ChannelName;
use monaka_contract::protocol::{Block, HeadPose, PoseBlock};
use std::sync::atomic::AtomicBool;

fn block(tag: &str) -> SharedBlock<PoseBlock> {
    let name = ChannelName::new(format!(r"Local\monaka-test-seqlock-{tag}-{}", std::process::id())).unwrap();
    let (name, lock) = name.block(Block::Pose);
    SharedBlock::<PoseBlock>::open_versioned(&name, &lock, 100).unwrap()
}

/// A head whose every number is `n`, so a copy mixing two writes shows.
fn head(n: f32) -> HeadPose {
    HeadPose { orientation: [n; 4], position: [n; 3], ipd: n, fov: [[n; 4]; 2], valid: 1, ..HeadPose::default() }
}

fn whole(pose: &HeadPose) -> bool {
    let n = pose.ipd;
    pose.orientation == [n; 4] && pose.position == [n; 3] && pose.fov == [[n; 4]; 2]
}

#[test]
fn a_reader_racing_a_writer_never_takes_a_torn_record() {
    let writer = block("race");
    let reader = block("race");
    let done = &AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(move || {
            for i in 0..200_000 {
                // SAFETY: a field of the block's record.
                write(&writer, |b| unsafe { &raw mut (*b).head }, &head(i as f32));
            }
            done.store(true, Ordering::Release);
        });
        let mut read = 0u32;
        while !done.load(Ordering::Acquire) {
            // SAFETY: a field of the block's record.
            // (Before the first write the block holds its fresh head, never written: not valid.)
            if let Some(pose) = super::read(&reader, |b| unsafe { &raw const (*b).head }).filter(|p| p.valid != 0) {
                assert!(whole(&pose), "a torn copy was kept: {pose:?}");
                read += 1;
            }
        }
        assert!(read > 0, "the reader got through");
    });
}

#[test]
fn a_write_left_half_done_is_a_miss_until_the_next_write() {
    let block = block("dead");
    // SAFETY: a field of the block's record.
    write(&block, |b| unsafe { &raw mut (*b).head }, &head(1.0));
    // A writer that died mid-write: the counter left odd.
    // SAFETY: the counter of the block's head part.
    unsafe { AtomicU64::from_ptr(&raw mut (*block.record_for_writing()).head.seq).fetch_add(1, Ordering::Relaxed) };
    // SAFETY: a field of the block's record.
    let get = || super::read(&block, |b| unsafe { &raw const (*b).head });
    assert_eq!(get(), None);
    // SAFETY: a field of the block's record.
    write(&block, |b| unsafe { &raw mut (*b).head }, &head(2.0));
    assert_eq!(get(), Some(head(2.0)));
}
