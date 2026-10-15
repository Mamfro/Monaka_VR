use super::*;

#[test]
fn a_poisoned_lock_is_taken() {
    let lock = std::sync::Arc::new(Mutex::new(1));
    let held = lock.clone();
    let _ = std::thread::spawn(move || {
        let _guard = held.lock().unwrap();
        panic!("poisons the lock");
    })
    .join();
    assert!(lock.is_poisoned());
    *lock.locked() += 1;
    assert_eq!(*lock.locked(), 2);
}
