use super::*;

#[test]
fn producers_are_recognised_by_name() {
    assert_eq!(producer_name("monaka_dl1.dll").as_deref(), Some("monaka_dl1"));
    assert_eq!(producer_name("MONAKA_DL1-20261005T1-ab12.dll").as_deref(), Some("monaka_dl1"));
    assert_eq!(producer_name("monaka_some_game-v_1.dll").as_deref(), Some("monaka_some_game"));
    assert_eq!(producer_name("monaka_dl1-.dll"), None);
    assert_eq!(producer_name("monaka_dl1-a.b.dll"), None);
    assert_eq!(producer_name("monaka_.dll"), None);
    assert_eq!(producer_name("dlvr_diag.dll"), None);
    assert_eq!(producer_name("other.dll"), None);
}

#[test]
fn a_dll_must_say_its_contract() {
    let not_pe = b"MZ but not really a PE";
    assert!(producer_host("monaka_game.dll", not_pe).is_err(), "an unreadable file is refused");
    let kernel = std::fs::read(r"C:\Windows\System32\kernelbase.dll").unwrap();
    let refused = producer_host("monaka_new_game.dll", &kernel).unwrap_err();
    assert!(refused.contains("MONAKA_CONTRACT"), "{refused}");
}

#[test]
fn only_a_running_copy_is_stopped() {
    assert_eq!(must_stop("monaka_a.dll", State::Running as u32), Ok(true));
    for state in [State::New, State::Stopped, State::Rejected] {
        assert_eq!(must_stop("monaka_a.dll", state as u32), Ok(false), "{state:?}");
    }
    for state in [State::Starting, State::Stopping, State::ShutdownFailed] {
        assert!(must_stop("monaka_a.dll", state as u32).is_err(), "{state:?}");
    }
    assert!(must_stop("monaka_a.dll", 99).is_err());
}
