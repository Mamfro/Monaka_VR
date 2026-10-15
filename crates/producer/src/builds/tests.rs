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
