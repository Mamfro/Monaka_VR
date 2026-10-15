use super::*;

#[test]
fn an_unknown_build_runs_only_when_allowed() {
    let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
    let Err(refused) = require_build("kernel32.dll", wrong) else { panic!("a wrong hash passed") };
    assert!(refused.reason.contains("Allow other game versions"), "{}", refused.reason);
    allow_other_builds(true);
    assert!(require_build("kernel32.dll", wrong).is_ok());
    allow_other_builds(false);
}
