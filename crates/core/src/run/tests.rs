//! Each test names its runs explicitly (the `_in` forms), so tests running at once never share the
//! process's run counter.

use super::*;
use std::any::Any;

#[test]
fn a_cell_set_in_an_earlier_run_reads_as_unset() {
    let cell: RunCell<u32> = RunCell::new();
    assert_eq!(cell.get_in(1), None);
    cell.put(1, Arc::new(7));
    assert_eq!(cell.get_in(1).as_deref(), Some(&7));
    assert_eq!(cell.get_in(2), None, "a restart must not see the first start's value");
    cell.put(2, Arc::new(8));
    assert_eq!(cell.get_in(2).as_deref(), Some(&8));
}

#[test]
fn a_cell_holds_an_unsized_value_and_downcasts_by_type() {
    let cell: RunCell<dyn Any + Send + Sync> = RunCell::new();
    cell.put(3, Arc::new(5u64));
    let any = cell.get_in(3).unwrap();
    assert!(Arc::clone(&any).downcast::<u32>().is_err(), "another type reads as none");
    assert_eq!(*any.downcast::<u64>().unwrap(), 5);
}

#[derive(Default)]
struct Throwing {
    steer: Option<u32>,
    made: Vec<u32>,
}

#[test]
fn a_state_is_made_fresh_for_each_run_and_kept_within_one() {
    let state: RunState<Throwing> = RunState::new(Throwing::default);
    {
        let mut s = state.lock_in(1);
        s.steer = Some(4);
        s.made.push(9);
    }
    assert_eq!(state.lock_in(1).steer, Some(4), "kept within the run");
    let next = state.lock_in(2);
    assert_eq!((next.steer, next.made.len()), (None, 0), "fresh in the next run");
}

#[test]
fn a_poisoned_state_is_taken_as_it_is() {
    let state = Arc::new(RunState::new(|| 1u32));
    let held = Arc::clone(&state);
    let _ = std::thread::spawn(move || {
        let _guard = held.lock_in(1);
        panic!("poisons the lock");
    })
    .join();
    *state.lock_in(1) += 1;
    assert_eq!(*state.lock_in(1), 2);
}

#[test]
fn a_count_starts_again_with_each_run() {
    let count = RunCount::new();
    count.add_in(1, 2);
    count.add_in(1, 3);
    assert_eq!(count.get_in(1), 5);
    assert_eq!(count.get_in(2), 0, "the last run's count does not show in the next");
    count.add_in(2, 1);
    assert_eq!(count.get_in(2), 1);
    assert_eq!(count.get_in(1), 0, "and the old run's is gone");
}
