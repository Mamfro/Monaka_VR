//! What a run changes in the game and gives back at its end. Each change is recorded with what was
//! there before, only when it really changes something; [`Ledger::restore`] puts everything back in
//! the reverse order it was changed. The caller runs the restore on whichever thread the game needs
//! it (the game's main thread for console variables that run callbacks, any thread for plain
//! memory); a setting that can tell whether its object is still there ([`Setting::alive`]) is left
//! alone when it is not.

use std::fmt::Debug;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering::*};
use monaka_core::sync::Locked;

/// Something in the game a run can change and give back: a console variable, a field in memory,
/// a value an engine call sets.
pub trait Setting: Send {
    type Value: Clone + PartialEq + Debug + Send;

    /// What the log calls it.
    fn name(&self) -> String;

    /// What it holds now; None when it cannot be read.
    fn read(&self) -> Option<Self::Value>;

    /// Writes `value`; whether it went in.
    fn write(&self, value: &Self::Value) -> bool;

    /// Whether it is still there to be written back (the object holding it may have been freed or
    /// replaced since); one that is not is left alone at the restore.
    fn alive(&self) -> bool {
        true
    }
}

/// What a [`Ledger::set`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Written, and what it held recorded for the restore.
    Changed,
    /// It already held the value: nothing written or recorded.
    Already,
    /// It could not be read, or the write did not go in: nothing recorded.
    Failed,
}

/// One recorded change, type-erased for the list.
trait Entry: Send {
    /// Puts the recorded value back; false when the setting was gone and left alone.
    fn undo(&mut self) -> bool;
}

struct Recorded<S: Setting> {
    setting: S,
    before: S::Value,
}

impl<S: Setting> Entry for Recorded<S> {
    fn undo(&mut self) -> bool {
        if !self.setting.alive() {
            crate::log!("{} was gone at the end; left alone", self.setting.name());
            return false;
        }
        let back = self.setting.write(&self.before);
        crate::log!("{} back to {:?}{}", self.setting.name(), self.before, if back { "" } else { " (not written)" });
        true
    }
}

/// The changes a run made, oldest first.
#[derive(Default)]
pub struct Ledger {
    entries: Vec<Box<dyn Entry>>,
}

impl Ledger {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Sets `setting` to `value` for the run, recording what it held: nothing is written or
    /// recorded when it already holds `value` or cannot be read.
    pub fn set<S: Setting + 'static>(&mut self, setting: S, value: S::Value) -> Change {
        let Some(before) = setting.read() else { return Change::Failed };
        if before == value {
            return Change::Already;
        }
        if !setting.write(&value) {
            return Change::Failed;
        }
        crate::log!("{} was {before:?}, now {value:?}", setting.name());
        self.entries.push(Box::new(Recorded { setting, before }));
        Change::Changed
    }

    /// Records what `setting` holds now, to be put back at the restore however the run writes it
    /// meanwhile (a value it sets every frame); what it held, or None when it cannot be read.
    pub fn keep<S: Setting + 'static>(&mut self, setting: S) -> Option<S::Value> {
        let before = setting.read()?;
        self.entries.push(Box::new(Recorded { setting, before: before.clone() }));
        Some(before)
    }

    /// Puts every recorded setting back, newest first, and forgets them; how many were gone and
    /// left alone.
    pub fn restore(&mut self) -> usize {
        let mut gone = 0;
        while let Some(mut entry) = self.entries.pop() {
            if !entry.undo() {
                gone += 1;
            }
        }
        gone
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A [`Ledger`] whose changes the game's own thread makes and gives back, for settings a game
/// applies only there (console variables whose change callbacks run on its main thread): the run
/// asks ([`GameThreadLedger::ask`]), a hook the game runs on that thread every frame does the work
/// ([`GameThreadLedger::on_game_thread`]), and the stop waits for the changes to be given back
/// there ([`GameThreadLedger::restore`]).
pub struct GameThreadLedger {
    stage: AtomicU8,
    ledger: Mutex<Ledger>,
}

/// What a [`GameThreadLedger::on_game_thread`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Applied,
    Restored,
}

/// Nothing to do, changes asked for, changes made, changes to give back, given back.
const IDLE: u8 = 0;
const ASKED: u8 = 1;
const APPLIED: u8 = 2;
const RESTORE: u8 = 3;
const RESTORED: u8 = 4;

impl GameThreadLedger {
    pub const fn new() -> Self {
        Self { stage: AtomicU8::new(IDLE), ledger: Mutex::new(Ledger::new()) }
    }

    /// Asks the game's thread to make the run's changes at its next turn.
    pub fn ask(&self) {
        self.stage.store(ASKED, Release);
    }

    /// Whether the game's thread has changes to make or to give back.
    pub fn due(&self) -> bool {
        matches!(self.stage.load(Acquire), ASKED | RESTORE)
    }

    /// On the game's thread: makes the changes with `apply` (recording them in the ledger) when
    /// asked, or gives them all back when the stop asks; which it did, if either.
    pub fn on_game_thread(&self, apply: impl FnOnce(&mut Ledger)) -> Option<Turn> {
        let stage = self.stage.load(Acquire);
        if stage != ASKED && stage != RESTORE {
            return None;
        }
        let mut ledger = self.ledger.locked();
        if stage == ASKED {
            apply(&mut ledger);
            self.stage.store(APPLIED, Release);
            Some(Turn::Applied)
        } else {
            ledger.restore();
            self.stage.store(RESTORED, Release);
            Some(Turn::Restored)
        }
    }

    /// At stop, while the hook still runs on the game's thread: asks it to give the changes back
    /// and waits up to `wait_ms`. True when they were given back, or none were made. When the game
    /// runs no turn meanwhile the restore stays asked for, so a turn before the hook goes still
    /// gives them back.
    pub fn restore(&self, wait_ms: u64) -> bool {
        if self.stage.compare_exchange(APPLIED, RESTORE, AcqRel, Acquire).is_err() {
            // Never applied (or the changes were still only asked for): nothing to give back.
            self.stage.store(IDLE, Release);
            return true;
        }
        crate::wait_until(wait_ms, || self.stage.load(Acquire) == RESTORED)
    }
}

impl Default for GameThreadLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
