use super::*;

const AER_AFW: Opt = stereo_method(&[StereoMethod::Aer.offer(), StereoMethod::Afw.offer(), STEREO_OFF], StereoMethod::Aer);
const BOTH: Opt = frame_generation(&[FrameGenerator::Off.offer(), FrameGenerator::Fsr.offer()], FrameGenerator::Off);
const OFF_ONLY: Opt = frame_generation(&[FrameGenerator::Off.offer()], FrameGenerator::Off);

#[test]
fn a_method_is_read_only_where_the_game_offers_it() {
    let read = |text: &str| StereoMethod::chosen(&AER_AFW, &Options::parse(text));
    assert_eq!(read("stereo=afw"), StereoMethod::Afw);
    assert_eq!(read("stereo=AER"), StereoMethod::Aer);
    assert_eq!(read("stereo=same"), StereoMethod::Aer, "not offered: the default");
    assert_eq!(read("stereo=off"), StereoMethod::Aer, "stereo off is the eyes' switch, not a method");
    assert_eq!(read("stereo=0"), StereoMethod::Aer);
    assert_eq!(read(""), StereoMethod::Aer);
    for method in METHODS {
        assert_eq!(method.offer().0, method.value());
    }
}

#[test]
fn stereo_off_is_offered_on_the_command_line_only() {
    let Kind::Choice { choices, window, default } = AER_AFW.kind else { panic!() };
    assert_eq!((&choices[window..], default), (&[STEREO_OFF][..], "aer"));
}

#[test]
fn a_generator_is_read_only_where_the_game_offers_it() {
    let read = |text: &str, opt: &Opt| FrameGenerator::chosen(opt, &Options::parse(text));
    assert_eq!(read("framegen=fsr", &BOTH), FrameGenerator::Fsr);
    assert_eq!(read("framegen=off", &BOTH), FrameGenerator::Off);
    assert_eq!(read("framegen=fsr", &OFF_ONLY), FrameGenerator::Off);
    assert_eq!(read("framegen=1", &BOTH), FrameGenerator::Off, "a generator is named");
}

#[test]
#[should_panic(expected = "the stereo method's default is not offered")]
fn a_method_default_not_offered_stops_the_build() {
    // Called at run time here; in a game's const the same check is a build error.
    static AER_ONLY: [Offer; 2] = [StereoMethod::Aer.offer(), STEREO_OFF];
    stereo_method(&AER_ONLY, StereoMethod::Afw);
}

#[test]
#[should_panic(expected = "the stereo method's default is not offered")]
fn stereo_off_is_no_default() {
    static NO_OFF: [Offer; 2] = [StereoMethod::Aer.offer(), StereoMethod::Afw.offer()];
    stereo_method(&NO_OFF, StereoMethod::SameFrame);
}

#[test]
#[should_panic(expected = "the frame generator's default is not offered")]
fn a_generator_default_not_offered_stops_the_build() {
    static OFF: [Offer; 1] = [FrameGenerator::Off.offer()];
    frame_generation(&OFF, FrameGenerator::Fsr);
}

#[test]
fn an_experimental_choice_stays_offered_and_says_so() {
    const MARKED: Opt = stereo_method(&[StereoMethod::Aer.offer(), StereoMethod::Afw.experimental(), STEREO_OFF], StereoMethod::Aer);
    let Kind::Choice { choices, window, .. } = MARKED.kind else { panic!() };
    assert_eq!(&choices[..window], [("aer", "AER (Alt-eye)"), ("afw", "AFW (Frame-warp), experimental")]);
    assert_eq!(StereoMethod::chosen(&MARKED, &Options::parse("stereo=afw")), StereoMethod::Afw);
    assert!(MARKED.help.starts_with(AER_AFW.help) && MARKED.help.ends_with("not yet this game's default because it is still being proven."), "{}", MARKED.help);
    const MARKED_FSR: Opt = frame_generation(&[FrameGenerator::Off.offer(), FrameGenerator::Fsr.experimental()], FrameGenerator::Off);
    assert!(MARKED_FSR.help.starts_with(BOTH.help) && MARKED_FSR.help.len() > BOTH.help.len());
    for help in [MARKED.help, MARKED_FSR.help, AER_AFW.help, BOTH.help] {
        assert!(!help.contains('\u{2014}'), "no em dashes: {help}");
    }
}

#[test]
fn the_rendering_tab_keeps_its_order() {
    let opts = [monaka_contract::manifest::RENDER_SIZE, BOTH, monaka_contract::manifest::HEADSET_DEPTH, AER_AFW];
    let mut sorted = opts;
    sorted.sort_by_key(place);
    let ids: Vec<&str> = sorted.iter().map(|o| o.id).collect();
    assert_eq!(ids, ["stereo", "framegen", "headset_depth", "render_size"]);
    assert_eq!(place(&monaka_contract::manifest::WORLD_HUD), 0, "another tab keeps the game's order");
}

#[test]
fn the_summary_lists_the_rendering_choices_by_label_in_the_tabs_order() {
    let sharpness = Opt { when: Some(("stereo", "afw")), ..opt("fsr_sharpness", "FSR sharpness", "", Kind::Number { min: 0.0, max: 1.0, step: 0.1, default: 0.2 }).on(Shown::Rendering) };
    let elsewhere = opt("hud_size", "HUD size", "", Kind::Number { min: 0.0, max: 1.0, step: 0.1, default: 0.5 });
    let opts = [sharpness, BOTH, AER_AFW, elsewhere];
    assert_eq!(summary(&opts, &Options::parse("stereo=afw\nframegen=fsr")), "Stereo method: AFW (Frame-warp)\nFrame generation: FSR frame generation\nFSR sharpness: 0.2");
    assert_eq!(summary(&opts, &Options::parse("")), "Stereo method: AER (Alt-eye)\nFrame generation: Off", "defaults; the sharpness means nothing without AFW");
}
