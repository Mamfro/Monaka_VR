use super::*;
use monaka_contract::manifest::{AIM_LOOK, AIM_MOTION, AIM_MOUSE, AIM_POINT, FINGER_TRACKING, HUD_SIZE, aim, aims, opt};

const FINGERS: &[Feature] = &[Feature::FingerRig];

#[test]
fn a_set_holds_features() {
    let set = Features::NONE.with(Feature::HandRig).with(Feature::Reload);
    assert!(set.has(Feature::HandRig) && set.has(Feature::Reload) && !set.has(Feature::Look));
    assert_eq!(set.iter().collect::<Vec<_>>(), [Feature::HandRig, Feature::Reload]);
}

#[test]
fn a_fault_names_the_component_and_the_option() {
    let has = Features::NONE.with_all(&[Feature::Stereo, Feature::HandRig, Feature::FingerRig]);
    assert!(fault("fingers", FINGERS, &[FINGER_TRACKING], has).is_none());
    let wrong = fault("eyes", &[Feature::Stereo], &[HUD_SIZE], has).unwrap();
    assert_eq!(wrong.text(), "component `eyes` offers `hud_scale`: only a component providing the HUD layer may offer it");
    const COPY: Opt = opt("finger_tracking", "Fingers", "Its own.", Kind::Switch { default: true });
    assert!(fault("mine", &[], &[COPY], has).unwrap().text().ends_with("offer the shared constant (`manifest::SHARED`)"));
    // Adjusted for a game: another help and no condition, still the shared option.
    const ADJUSTED: Opt = FINGER_TRACKING.help("Your fingers move his.").always();
    assert!(fault("fingers", FINGERS, &[ADJUSTED], has).is_none());
}

#[test]
fn aim_choices_follow_the_features() {
    let rig_only = Features::of(Feature::HandRig);
    const ALL: Opt = aim(&[AIM_MOTION, AIM_LOOK, AIM_POINT, AIM_MOUSE], aims::HAND_RIG);
    let wrong = fault("arms", &[Feature::HandRig], &[ALL], rig_only).unwrap();
    assert!(wrong.text().contains("choice `head` needs a feature"), "{}", wrong.text());
    const RIG: Opt = aim(&[AIM_MOTION, AIM_MOUSE], aims::HAND_RIG);
    assert!(fault("arms", &[Feature::HandRig], &[RIG], rig_only).is_none());
}

#[test]
fn every_shared_option_has_one_owner() {
    use monaka_contract::manifest::SHARED;
    for opt in SHARED {
        assert!(opt.shared, "{} is in SHARED but not marked shared", opt.id);
        assert!(owner(opt.id).is_some(), "{} has no owner in OWNERS", opt.id);
    }
    for (id, _) in OWNERS {
        assert!(SHARED.iter().any(|o| o.id == *id), "{id} is owned but not in SHARED");
    }
}

#[test]
fn an_option_offered_twice_is_found() {
    assert_eq!(repeated(&[&[HUD_SIZE], &[FINGER_TRACKING, HUD_SIZE]]), Some("hud_scale"));
    assert_eq!(repeated(&[&[HUD_SIZE], &[FINGER_TRACKING]]), None);
}

#[test]
fn a_long_message_is_cut_not_broken() {
    let mut m = Message::new();
    for _ in 0..200 {
        m = m.push("ab");
    }
    assert_eq!(m.text().len(), 240);
    // Three-byte characters after one byte: 79 fit in the 239 left, the 80th would not.
    let mut m = Message::new().push("a");
    for _ in 0..100 {
        m = m.push("日");
    }
    assert_eq!(m.text(), format!("a{}", "日".repeat(79)));
}

#[test]
fn a_run_says_what_is_on_off_and_lost() {
    let provided = Features::NONE.with_all(&[Feature::Stereo, Feature::Look, Feature::HandRig]);
    let mut run = Available::new(provided, &[Situation::CameraTaken]);
    run.off(Feature::Look, "aim is off");
    run.lose(Feature::HandRig, "this build's table has no character layout");
    assert!(run.has(Feature::Stereo) && !run.has(Feature::HandRig) && !run.has(Feature::Look));
    assert_eq!(
        run.report(),
        [
            "stereo: on",
            "look: off (aim is off)",
            "hand rig: lost (this build's table has no character layout)",
            "stereo: runs without a report that a video plays, or that the world is not drawn"
        ]
    );
}
