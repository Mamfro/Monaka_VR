use super::*;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// A value in a shared cell, with a log of every write.
struct Cell {
    name: &'static str,
    value: &'static Mutex<i32>,
    writes: &'static Mutex<Vec<(&'static str, i32)>>,
    alive: &'static AtomicBool,
}

impl Setting for Cell {
    type Value = i32;
    fn name(&self) -> String {
        self.name.to_owned()
    }
    fn read(&self) -> Option<i32> {
        Some(*self.value.lock().unwrap())
    }
    fn write(&self, value: &i32) -> bool {
        *self.value.lock().unwrap() = *value;
        self.writes.lock().unwrap().push((self.name, *value));
        true
    }
    fn alive(&self) -> bool {
        self.alive.load(Relaxed)
    }
}

#[test]
fn changes_are_recorded_once_and_given_back_newest_first() {
    static A: Mutex<i32> = Mutex::new(1);
    static B: Mutex<i32> = Mutex::new(2);
    static WRITES: Mutex<Vec<(&str, i32)>> = Mutex::new(Vec::new());
    static ALIVE: AtomicBool = AtomicBool::new(true);
    let cell = |name, value| Cell { name, value, writes: &WRITES, alive: &ALIVE };
    let mut ledger = Ledger::new();
    assert_eq!(ledger.set(cell("a", &A), 1), Change::Already, "already held: nothing recorded");
    assert!(ledger.is_empty() && WRITES.lock().unwrap().is_empty());
    assert_eq!(ledger.set(cell("a", &A), 5), Change::Changed);
    assert_eq!(ledger.keep(cell("b", &B)), Some(2));
    // The run writes b every frame; the restore still gives back the value it had first.
    *B.lock().unwrap() = 9;
    WRITES.lock().unwrap().clear();
    assert_eq!(ledger.restore(), 0);
    assert_eq!(*WRITES.lock().unwrap(), [("b", 2), ("a", 1)]);
    assert_eq!((*A.lock().unwrap(), *B.lock().unwrap()), (1, 2));
    assert!(ledger.is_empty());
}

#[test]
fn the_game_thread_makes_and_gives_back_the_changes() {
    static A: Mutex<i32> = Mutex::new(1);
    static WRITES: Mutex<Vec<(&str, i32)>> = Mutex::new(Vec::new());
    static ALIVE: AtomicBool = AtomicBool::new(true);
    static CHANGES: GameThreadLedger = GameThreadLedger::new();
    assert!(!CHANGES.due() && CHANGES.restore(0), "nothing asked: nothing to give back");
    CHANGES.ask();
    assert!(CHANGES.due());
    let apply = |ledger: &mut Ledger| _ = ledger.set(Cell { name: "a", value: &A, writes: &WRITES, alive: &ALIVE }, 7);
    assert_eq!(CHANGES.on_game_thread(apply), Some(Turn::Applied));
    assert_eq!((*A.lock().unwrap(), CHANGES.due()), (7, false));
    assert_eq!(CHANGES.on_game_thread(|_| unreachable!()), None);
    // The game's thread keeps turning while the stop waits.
    let game = std::thread::spawn(|| while CHANGES.on_game_thread(|_| unreachable!()) != Some(Turn::Restored) {});
    assert!(CHANGES.restore(5000));
    game.join().unwrap();
    assert_eq!(*A.lock().unwrap(), 1);
}

#[test]
fn a_setting_gone_by_the_end_is_left_alone() {
    static A: Mutex<i32> = Mutex::new(1);
    static WRITES: Mutex<Vec<(&str, i32)>> = Mutex::new(Vec::new());
    static ALIVE: AtomicBool = AtomicBool::new(true);
    let mut ledger = Ledger::new();
    assert_eq!(ledger.set(Cell { name: "a", value: &A, writes: &WRITES, alive: &ALIVE }, 3), Change::Changed);
    ALIVE.store(false, Relaxed);
    assert_eq!(ledger.restore(), 1);
    assert_eq!(*A.lock().unwrap(), 3, "not written back");
}
