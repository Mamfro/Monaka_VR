use super::*;

/// Each stage's place in [`Stage::ALL`]. The match is exhaustive: a new stage does not compile
/// here until it is given the next place, and the test below fails until `ALL` has it there.
fn place(stage: Stage) -> usize {
    match stage {
        Stage::MonoScene => 0,
        Stage::LeftScene => 1,
        Stage::LeftDrain => 2,
        Stage::RightScene => 3,
        Stage::RightDrain => 4,
        Stage::FrameInterval => 5,
        Stage::GlColour => 6,
        Stage::GlUpscale => 7,
        Stage::GlWarp => 8,
        Stage::GlPresent => 9,
        Stage::GameSwap => 10,
        Stage::GameRender => 11,
        Stage::RenderCpu => 12,
    }
}

#[test]
fn all_lists_every_stage_once() {
    const PLACES: usize = 13;
    assert_eq!(Stage::ALL.len(), PLACES, "a stage was given a place but not added to ALL");
    for (at, stage) in Stage::ALL.into_iter().enumerate() {
        assert_eq!(place(stage), at, "{stage:?}");
    }
}
