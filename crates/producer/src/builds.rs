//! A game's measured builds: one row of facts per build, the running one picked once at the start
//! and kept for the run. A row is told by its module's fingerprint ([`Measured`]); which rows are
//! candidates is the game's own rule (its host executable, the manifest the launcher meant, the
//! renderer it loaded). Picking goes through [`crate::require_build`], so `other_builds` lets an
//! unknown build of a module run on the one row that fits it.

use crate::{Rejection, log, require_build};
use monaka_hook::module::Module;
use std::sync::OnceLock;

/// One measured build: the module it is a build of and that module's fingerprint.
pub trait Measured: Sync + 'static {
    /// The module the fingerprint is of (`Game.exe`, `Engine.dll`).
    fn module(&self) -> &str;
    /// Its SHA-256, upper-case hex.
    fn sha256(&self) -> &str;
}

/// A game's build table and the row picked for this run.
pub struct Builds<R: Measured> {
    pub rows: &'static [R],
    chosen: OnceLock<&'static R>,
}

impl<R: Measured> Builds<R> {
    pub const fn new(rows: &'static [R]) -> Self {
        Self { rows, chosen: OnceLock::new() }
    }

    /// The row for this process among the `candidates` (the game's rule), and its module: the one
    /// candidate whose fingerprint the loaded module has; a single candidate whatever its
    /// fingerprint when `other_builds` allows it ([`require_build`]). Several candidates on one
    /// module are told apart by the fingerprint alone: an unknown build of it is refused, there
    /// being no measured row to run it on.
    pub fn pick(&self, candidates: impl Fn(&R) -> bool, what: &str) -> Result<(&'static R, Module), Rejection> {
        let rows: Vec<&'static R> = self.rows.iter().filter(|r| candidates(r)).collect();
        let row = match rows.as_slice() {
            [] => return Err(Rejection::unsupported(format!("no build facts for {what}"))),
            [only] => *only,
            several => {
                let module = Module::find(several[0].module()).ok_or_else(|| Rejection::not_ready(format!("{} is not loaded", several[0].module())))?;
                let sha = module.file_sha256().unwrap_or_default();
                *several.iter().find(|r| r.sha256() == sha).ok_or_else(|| {
                    Rejection::revision(format!("{} is build {sha}, none of the {} measured for {what} (other builds cannot run: the build picks the facts)", several[0].module(), several.len()))
                })?
            }
        };
        let module = require_build(row.module(), row.sha256())?;
        log!("{what}: {} build {}", row.module(), row.sha256());
        let _ = self.chosen.set(row);
        Ok((row, module))
    }

    /// The row picked for this run ([`Builds::pick`]).
    pub fn current(&self) -> Option<&'static R> {
        self.chosen.get().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Row {
        game: &'static str,
        module: &'static str,
        sha256: &'static str,
    }

    impl Measured for Row {
        fn module(&self) -> &str {
            self.module
        }

        fn sha256(&self) -> &str {
            self.sha256
        }
    }

    const UNKNOWN: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn rows_are_told_apart_by_the_game_s_rule_then_the_fingerprint() {
        static ROWS: [Row; 3] = [
            Row { game: "a", module: "kernel32.dll", sha256: UNKNOWN },
            Row { game: "b", module: "kernel32.dll", sha256: UNKNOWN },
            Row { game: "b", module: "kernel32.dll", sha256: "1111111111111111111111111111111111111111111111111111111111111111" },
        ];
        let builds = Builds::new(&ROWS);
        assert!(builds.pick(|r| r.game == "c", "c").is_err(), "no candidate");
        // Several candidates on one module: only the fingerprint tells them apart, whatever
        // `other_builds` says (a single candidate's fingerprint is `require_build`'s, tested there).
        let refused = builds.pick(|r| r.game == "b", "b").err().unwrap();
        assert!(refused.reason.contains("none of the 2 measured"), "{}", refused.reason);
        assert!(builds.current().is_none(), "nothing picked");
    }
}
