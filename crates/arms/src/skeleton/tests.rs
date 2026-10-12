use super::*;

#[test]
fn a_rename_table_maps_a_games_names() {
    const PREFIXED: Renames = Renames { sides: [("Left", "l_"), ("Right", "r_")], suffix: "", names: &[("ForeArm", "forearm"), ("Hand", "hand")] };
    assert_eq!(PREFIXED.solver_name(1, "RightForeArm").as_deref(), Some("r_forearm"));
    assert_eq!(PREFIXED.solver_name(0, "LeftHand").as_deref(), Some("l_hand"));
    assert_eq!(PREFIXED.solver_name(0, "RightHand"), None, "the other side's prefix");
    assert_eq!(PREFIXED.solver_name(0, "LeftHandIndex1"), None, "not in the table");
    const SUFFIXED: Renames = Renames { sides: [("l_", "l_"), ("r_", "r_")], suffix: "_jnt", names: &[("lowerArm", "forearm")] };
    assert_eq!(SUFFIXED.solver_name(0, "l_lowerArm_jnt").as_deref(), Some("l_forearm"));
    assert_eq!(SUFFIXED.solver_name(1, "r_lowerArm").as_deref(), Some("r_forearm"), "the suffix may be missing");
}

/// A right arm hung from its attach joint: joint 0 a root, 1 the attach joint, 2 the wrist with
/// three fingers (3, 4, 5), 6 and 7 forearm twists, 8 the elbow, 9 the shoulder.
const ARM: [i32; 10] = [-1, 0, 1, 2, 2, 2, 2, 6, 7, 8];

#[test]
fn an_arm_is_found_by_its_shape() {
    let arm = find_arm(&ARM, 1).unwrap();
    assert_eq!((arm.wrist, arm.elbow, arm.shoulder), (2, 8, 9));
    assert_eq!(arm.forearm, vec![6, 7, 8, 9]);
    assert_eq!(arm.subtree.len(), 9);
    assert_eq!(subtree(&ARM, 6), vec![6, 7, 8, 9]);
    assert_eq!(children(&ARM, 2), vec![3, 4, 5, 6]);
    // Fingers: the three one-joint chains beside the forearm.
    let fingers = finger_chains(&ARM, 2, 6);
    assert_eq!(fingers, vec![(2, vec![3]), (2, vec![4]), (2, vec![5])]);
    // Both arms under one root: that root is no arm, and not an attach joint.
    let mut both = ARM.to_vec();
    for &parent in &ARM[1..] {
        both.push(if parent == 0 { 0 } else { parent + 9 });
    }
    assert!(find_arm(&both, 0).is_none());
    let mut distances = vec![9.0; both.len()];
    distances[0] = 0.0;
    distances[1] = 0.0;
    assert_eq!(attach_joint(&both, &distances), Some(1));
}
