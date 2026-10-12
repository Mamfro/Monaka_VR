//! The game's situations for this run ([`monaka_core::situation`]): one record the game reports
//! into and every shared feature reads, on one clock (milliseconds since the record was first
//! used). Each change is logged, the first few of each situation.

pub use monaka_core::situation::{Situation, Situations};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use monaka_core::sync::Locked;

static SITUATIONS: Mutex<Situations> = Mutex::new(Situations::new());
static CLOCK: OnceLock<Instant> = OnceLock::new();
/// Change lines logged per situation.
static LOGGED: [AtomicU32; Situation::ALL.len()] = [const { AtomicU32::new(0) }; Situation::ALL.len()];
const LOG_LINES: u32 = 40;

/// Milliseconds on the situations' clock.
pub fn now() -> u64 {
    CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn with<R>(f: impl FnOnce(&mut Situations) -> R) -> R {
    f(&mut SITUATIONS.locked())
}

/// The situations the running game's components report; `None` before a run.
static ACCEPTED: Mutex<Option<Vec<Situation>>> = Mutex::new(None);
static REFUSED: [AtomicU32; Situation::ALL.len()] = [const { AtomicU32::new(0) }; Situation::ALL.len()];

/// The run's game reports these and no others (`game::start`, from its components' `reports`):
/// what a component says it reports is what its code reports, or the log shows the difference.
pub fn accept(reports: Vec<Situation>) {
    *ACCEPTED.locked() = Some(reports);
}

/// The game says whether `situation` holds now, good for `fresh_ms` ([`Situations::report`]). A
/// situation the game does not list among its reports is refused (logged once).
pub fn report(situation: Situation, active: bool, fresh_ms: u64) {
    let accepted = ACCEPTED.locked().as_ref().is_none_or(|reports| reports.contains(&situation));
    if !accepted {
        if REFUSED[situation.index()].fetch_add(1, Relaxed) == 0 {
            crate::log!("situation: {} reported, but no component of the game lists it among its reports; refused", situation.label());
        }
        return;
    }
    let at = now();
    if let Some(now_active) = with(|s| s.report(situation, active, at, fresh_ms))
        && LOGGED[situation.index()].fetch_add(1, Relaxed) < LOG_LINES
    {
        crate::log!("situation: {} {}", situation.label(), if now_active { "began" } else { "ended" });
    }
}

/// The latest report while it holds; `None` when not known.
pub fn state(situation: Situation) -> Option<bool> {
    let at = now();
    with(|s| s.state(situation, at))
}

pub fn active(situation: Situation) -> bool {
    let at = now();
    with(|s| s.active(situation, at))
}

/// How long `situation` has held (milliseconds), while it does.
pub fn active_for(situation: Situation) -> Option<u64> {
    let at = now();
    with(|s| s.active_for(situation, at))
}

/// A copy of every situation now and the time it was taken at, for a feature that asks several
/// questions at once (the policies on [`Situations`]: `head_aim_stands_down`, `flat`,
/// `arms_to_game`, `hands_held_back`, `head_position_fades`).
pub fn snapshot() -> (Situations, u64) {
    let at = now();
    (with(|s| *s), at)
}

/// Forgets every report (a new run in the same process).
pub fn clear() {
    with(|s| *s = Situations::new());
    for logged in &LOGGED {
        logged.store(0, Relaxed);
    }
}

/// For the stop's summary: how often each reported situation began.
pub fn summary() -> String {
    format!("situations: {}", with(|s| s.summary()))
}
