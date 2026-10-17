use super::*;

#[test]
fn a_running_detour_is_named_by_where_it_took_its_guard() {
    let guard = InFlight::enter();
    let line = line!() - 1;
    let running = InFlight::running();
    assert!(running.contains(&format!("hook/src/in_flight/tests.rs:{line} on thread {}", crate::thread_id())), "{running}");
    drop(guard);
    assert!(!InFlight::running().contains(&format!("tests.rs:{line} ")));
}

#[test]
fn paths_keep_the_crate_folder_and_below() {
    assert_eq!(short_path(r"C:\Users\someone\mods\wolf_tno\src\providers\focus.rs"), "wolf_tno/src/providers/focus.rs");
    assert_eq!(short_path("crates/hook/src/lib.rs"), "hook/src/lib.rs");
    assert_eq!(short_path("src/lib.rs"), "src/lib.rs");
    assert_eq!(short_path("lib.rs"), "lib.rs");
}
