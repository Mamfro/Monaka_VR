use super::*;
use monaka_contract::debug_draw::{Category, WHITE};

fn channel_name(what: &str) -> ChannelName {
    ChannelName::new(format!(r"Local\monaka-test-debug-{what}-{}", std::process::id())).unwrap()
}

#[test]
fn each_eye_gets_its_own_frame() {
    let name = channel_name("eyes");
    let mut producer = DebugChannel::open(&name).unwrap();
    let mut viewer = DebugChannel::open(&name).unwrap();
    let line = |x: f32| Primitive::segment(Category::Probe, [x, 0.0, 0.0], [x, 1.0, 0.0], WHITE);
    assert!(producer.write_frame(20, &[line(0.0)]));
    assert!(producer.write_frame(21, &[line(1.0), line(1.5)]));
    assert!(producer.write_eye_keys(7, [20, 21]));
    let mut eyes = [Vec::new(), Vec::new()];
    assert_eq!(viewer.read_pair(7, &mut eyes), [Source::Synced(20), Source::Synced(21)]);
    assert_eq!((eyes[0].len(), eyes[1].len()), (1, 2));
    assert_eq!(eyes[1][0].a[0], 1.0);
    // A pair without keys gets the newest frame on both eyes.
    assert_eq!(viewer.read_pair(8, &mut eyes), [Source::Newest(21), Source::Newest(21)]);
}

#[test]
fn wants_and_offers_travel() {
    let name = channel_name("wants");
    let mut producer = DebugChannel::open(&name).unwrap();
    let mut viewer = DebugChannel::open(&name).unwrap();
    assert_eq!(producer.wanted(), Wanted::default());
    let wanted = Wanted { categories: Categories::NONE.with(Category::Skeleton, true), pass: 3, buffer: 2, target: 4 };
    assert!(viewer.want(wanted));
    assert_eq!(producer.wanted(), wanted);
    let listed = [monaka_contract::debug_draw::DepthCandidate { width: 3840, height: 2160, format: 44, draws: 380, samples: 1, layers: 1, usable: 1 }];
    assert!(producer.write_candidates(&listed, 1));
    assert_eq!(viewer.candidates(), (listed.to_vec(), 1));
    assert!(producer.offer(Categories::NONE.with(Category::Swings, true)));
    assert!(viewer.offered().has(Category::Swings));
}

#[test]
fn the_games_timing_and_settings_reach_the_viewer() {
    use monaka_contract::debug_draw::{CostTime, GAME_FRAMES};
    let name = channel_name("game");
    let mut producer = DebugChannel::open(&name).unwrap();
    let mut viewer = DebugChannel::open(&name).unwrap();
    assert_eq!(viewer.game().map(|g| g.tick), Some(0), "nothing written yet");
    assert!(producer.write_game_text("Stereo method: AFW (Frame-warp)\nFrame generation: Off"));
    let mut costs = [CostTime::default(); monaka_contract::debug_draw::COSTS];
    costs[0] = CostTime { mean_ms: 0.4, worst_ms: 1.2 };
    let stretch = |frames| Stretch { fps: 71.5, frame_ms: 13.9, worst_frame_ms: 22.0, size: [2644, 2644], frames, costs };
    assert!(producer.write_game(&stretch(&[13.0, 14.0, 22.0])));
    let game = viewer.game().unwrap();
    assert_eq!((game.fps, game.frame_ms, game.worst_frame_ms, game.width, game.height), (71.5, 13.9, 22.0, 2644, 2644));
    assert_eq!(game.costs[0], costs[0]);
    assert_eq!(game.frames_since(1), (vec![14.0, 22.0], 3), "the frames after the first");
    assert_eq!(game.text(), "Stereo method: AFW (Frame-warp)\nFrame generation: Off", "the timing keeps the text");
    // More frames than the ring holds: only the newest are there.
    let many: Vec<f32> = (0..GAME_FRAMES + 10).map(|n| n as f32).collect();
    assert!(producer.write_game(&stretch(&many)));
    let (times, count) = viewer.game().unwrap().frames_since(0);
    // Frames 0-2 are the first three; frame 13, the oldest kept, is the many's eleventh.
    assert_eq!((times.len(), count, times[0]), (GAME_FRAMES, 3 + GAME_FRAMES as u64 + 10, 10.0));
}

#[test]
fn a_settings_line_too_long_is_cut_at_a_character() {
    let mut game = monaka_contract::debug_draw::GameReport::EMPTY;
    let long = "é".repeat(monaka_contract::debug_draw::GAME_TEXT);
    game.set_text(&long);
    assert_eq!(game.text().len(), monaka_contract::debug_draw::GAME_TEXT);
    assert!(game.text().chars().all(|c| c == 'é'));
}
