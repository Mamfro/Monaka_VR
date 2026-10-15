use super::*;

#[test]
fn every_binding_names_a_control_and_a_known_profile_has_some() {
    for profile in ["valve/frame_controller", "oculus/touch_controller", "valve/index_controller", "microsoft/motion_controller", "htc/vive_controller", "khr/simple_controller"] {
        let bindings = default_bindings(&format!("/interaction_profiles/{profile}"));
        assert!(!bindings.is_empty(), "{profile}");
        for (name, path) in &bindings {
            assert!(CONTROLS.iter().any(|(n, _, _)| n == name), "{profile}: {name}");
            assert!(path.starts_with("/user/hand/"), "{path}");
        }
        let mut names: Vec<_> = bindings.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), bindings.len(), "{profile}: one binding per control");
    }
    assert!(default_bindings("/interaction_profiles/unknown/thing").is_empty());
    for profile in ["valve/frame_controller", "oculus/touch_controller", "valve/index_controller"] {
        let extras = extra_bindings(&format!("/interaction_profiles/{profile}"));
        assert!(!extras.is_empty(), "{profile}");
        for (name, path) in extras.iter().flatten() {
            assert!(CONTROLS.iter().any(|(n, _, _)| n == name) && path.starts_with("/user/hand/left/input/"), "{name} {path}");
        }
    }
}
