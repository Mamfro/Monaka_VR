use super::*;

#[test]
fn a_feature_this_host_does_not_know_is_missing() {
    assert!(has(DEPTH));
    assert_eq!(missing(&[PANELS, "teleport", DEPTH, "haptics"]), ["teleport", "haptics"]);
}

#[test]
fn the_run_folder_list_reads_back() {
    assert_eq!(parse_features(&features_text()), HOST);
    assert_eq!(parse_features("depth\r\n\n  panels \n"), ["depth", "panels"]);
}

#[test]
fn names_are_unique() {
    for (i, name) in HOST.iter().enumerate() {
        assert!(!HOST[i + 1..].contains(name), "{name} twice");
        assert!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{name}: lowercase words and _");
    }
}
