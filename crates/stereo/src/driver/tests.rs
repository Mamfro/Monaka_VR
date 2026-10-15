use super::*;
use monaka_core::options::Options;
use monaka_core::situation::{Situation, Situations};

#[test]
fn a_frame_goes_flat_for_the_first_reason_given() {
    let mut situations = Situations::new();
    assert_eq!(Flat::of_frame(None, None, &situations, 0), None);
    situations.report(Situation::WorldHidden, true, 0, 500);
    assert_eq!(Flat::of_frame(None, None, &situations, 10), Some(Flat::WorldHidden), "from the run's situations");
    assert_eq!(Flat::of_frame(None, Some(|| true), &situations, 10), Some(Flat::Menu), "the game's menu first");
    assert_eq!(Flat::of_frame(Some(|| Some(Flat::CameraTaken)), Some(|| true), &situations, 10), Some(Flat::CameraTaken), "its detector before that");
    assert_eq!(Flat::of_frame(Some(|| None), None, &situations, 600), None, "the report expired");
    situations.report(Situation::CameraTaken, true, 600, 500);
    assert_eq!(Flat::of_frame(None, None, &situations, 610), None, "another camera's view alone is not flat");
    for reason in Flat::ALL {
        assert_eq!(Flat::from_code(Flat::code(Some(reason))), Some(reason));
    }
    assert_eq!(Flat::from_code(Flat::code(None)), None);
}

#[test]
fn a_frame_is_labelled_by_its_camera() {
    let driver = Driver::new();
    driver.capture.store(true, Release);
    let pose = HeadPose { valid: 1, ..HeadPose::default() };
    driver.camera_written([1.0, 2.0, 3.0], 0, pose);
    driver.camera_written([1.1, 2.0, 3.0], 1, pose);
    assert_eq!(driver.label_frame([1.1, 2.0, 3.001]), Some(1));
    assert_eq!(driver.take_label().map(|l| l.eye), Some(1));
    assert!(driver.take_label().is_none(), "taken once");
    assert_eq!(driver.label_frame([5.0, 2.0, 3.0]), None, "no camera written there");
}

#[test]
fn the_default_report_runs_while_the_state_is_held() {
    // The publisher calls the report inside the state lock: the report must not take it again
    // (that deadlocks the present thread at the first published pair, freezing the game).
    let driver = Driver::new();
    use monaka_contract::manifest::{Launch, Manifest, Status};
    let manifest = Manifest {
        id: "test",
        name: "Test",
        status: Status::Experimental,
        executables: &["test.exe"],
        steam_app: 0,
        producer: "monaka_test",
        log: "monaka_test.log",
        options_file: "test-options.txt",
        run_prefix: "test",
        launch: Launch::Manual,
        refused_arguments: &[],
        before: "",
        options: &[],
        licences: &[],
        left_hand_aims: false,
        weapon_ring: false,
        controls: &[],
    };
    *driver.settings.lock().unwrap() = Some(Shared::from_options(&Options::parse("test_separation=0.3"), &manifest));
    let held = driver.state.lock().unwrap();
    let mut pose = HeadPose::default();
    driver.report_centred(&mut pose);
    assert_eq!(pose.ipd, 0.3);
    drop(held);
}
