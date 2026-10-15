use super::*;

#[test]
fn dr7_enables_write_watches_of_four_bytes() {
    // L0, RW0 = 01 (write), LEN0 = 11 (4 bytes).
    assert_eq!(dr7_for(1), 0b1101_0000_0000_0000_0001);
    assert_eq!(dr7_for(2), 0b1101_1101_0000_0000_0000_0101);
}

#[test]
fn a_watched_write_on_another_thread_is_recorded() {
    let target = Box::leak(Box::new(0u32)) as *mut u32 as usize;
    let (ready, go) = (std::sync::Arc::new(std::sync::Barrier::new(2)), std::sync::Arc::new(std::sync::Barrier::new(2)));
    let (r, g) = (ready.clone(), go.clone());
    let worker = std::thread::spawn(move || {
        r.wait();
        g.wait();
        // SAFETY: the leaked u32 lives forever.
        unsafe { std::ptr::write_volatile(target as *mut u32, 7) };
    });
    ready.wait();
    let armed = arm(&[target], 2).unwrap();
    go.wait();
    worker.join().unwrap();
    disarm();
    assert!(armed >= 1);
    let hits = hits();
    assert!(hits.iter().any(|h| h.slot == 0 && h.rip != 0), "{hits:?}");
}
