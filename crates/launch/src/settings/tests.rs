use super::*;

#[test]
fn round_trips() {
    let text = "selected=dl2\ndl2.mode=framegen\ndl1.extra=probe_rig=1 hand_hold=0,0.01,0\na pack.x=1\nremoved_pack.x=1\nmonaka.controller_smoothing=5\njunk\n";
    let settings = Settings::parse(text);
    assert_eq!(settings.monaka.0["controller_smoothing"], "5");
    assert!(!settings.values.contains_key("monaka"));
    assert_eq!(settings.values("dl1").0["controller_smoothing"], "5", "every game's run gets Monaka's own");
    assert_eq!(settings.values("unknown").0.len(), 1);
    assert_eq!(settings.selected.as_deref(), Some("dl2"));
    assert_eq!(settings.values("dl1").0["extra"], "probe_rig=1 hand_hold=0,0.01,0");
    assert!(!settings.values.contains_key("a pack"));
    assert_eq!(settings.values("removed_pack").0["x"], "1", "kept for when the pack comes back");
    assert_eq!(Settings::parse(&settings.text()), settings);
}
