use super::*;
use core::mem::{offset_of, size_of};

#[test]
fn debug_block_layout() {
    assert_eq!(size_of::<Primitive>(), 32);
    assert_eq!(size_of::<EyeKeys>(), 24);
    assert_eq!(size_of::<DebugFrame>(), 24 + 32 * DEBUG_PRIMITIVES);
    assert_eq!(size_of::<DebugDepth>(), 48);
    assert_eq!(size_of::<DepthCandidate>(), 24);
    assert_eq!(offset_of!(DebugBlock, wanted_pass), 48);
    assert_eq!(offset_of!(DebugBlock, depth), 56);
    assert_eq!(offset_of!(DebugBlock, candidates), 104);
    let targets = 104 + 24 * DEPTH_CANDIDATES + 8;
    assert_eq!(offset_of!(DebugBlock, wanted_target), targets);
    assert_eq!(offset_of!(DebugBlock, target), targets + 8);
    assert_eq!(offset_of!(DebugBlock, targets), targets + 8 + 48);
    assert_eq!(size_of::<GameReport>(), 32 + 8 + 4 * GAME_FRAMES + 8 * COSTS + GAME_TEXT);
    let game = targets + 8 + 48 + 24 * DEPTH_CANDIDATES + 8;
    assert_eq!(offset_of!(DebugBlock, game), game);
    let pairs = game + size_of::<GameReport>();
    assert_eq!(offset_of!(DebugBlock, pairs), pairs);
    assert_eq!(offset_of!(DebugBlock, frames), pairs + 24 * DEBUG_PAIRS);
    assert_eq!(
        size_of::<DebugBlock>(),
        pairs + 24 * DEBUG_PAIRS + size_of::<DebugFrame>() * DEBUG_FRAMES
    );
}

#[test]
fn colours_pack_red_low() {
    assert_eq!(rgba(1.0, 0.0, 0.0, 1.0), 0xff00_00ff);
    assert_eq!(rgba(0.0, 0.0, 1.0, 0.5), 0x80ff_0000);
}

#[test]
fn frames_and_keys_are_found_by_key_only() {
    with_block(|block| {
        assert!(block.is_blank());
        block.start();
        assert!(block.is_compatible());
        let line = Primitive::segment(Category::Probe, [0.0; 3], [1.0, 0.0, 0.0], WHITE);
        block.write_frame(11, &[line, line], 5);
        assert_eq!(block.frame(11).map(|f| f.primitives().len()), Some(2));
        assert!(block.frame(3).is_none(), "same slot, other key");
        block.write_frame(19, &[line], 6);
        assert!(
            block.frame(11).is_none(),
            "overwritten by a newer key in its slot"
        );
        assert_eq!(block.newest().map(|f| f.key), Some(19));
        block.write_eye_keys(40, [18, 19]);
        assert_eq!(block.eye_keys(40), Some([18, 19]));
        assert_eq!(block.eye_keys(24), None);
    });
}

#[test]
fn a_full_frame_counts_what_it_drops() {
    with_block(|block| {
        let many = vec![Primitive::point(Category::Probe, [0.0; 3], WHITE); DEBUG_PRIMITIVES + 5];
        block.write_frame(1, &many, 1);
        let frame = block.frame(1).unwrap();
        assert_eq!((frame.count as usize, frame.dropped), (DEBUG_PRIMITIVES, 5));
    });
}

#[test]
fn categories_toggle_and_know_actions() {
    let set = Categories::NONE
        .with(Category::Skeleton, true)
        .with(Category::HideHud, true);
    assert!(set.has(Category::Skeleton) && set.has(Category::HideHud) && !set.has(Category::Aim));
    assert!(set.draws());
    assert!(!Categories::NONE.with(Category::HideHud, true).draws());
    assert!(
        Category::ALL
            .iter()
            .all(|c| Category::from_index(*c as u16) == Some(*c) && Category::from_id(c.id()) == Some(*c))
    );
    assert!(
        Category::ALL
            .iter()
            .filter(|c| c.is_action())
            .all(|c| *c as u16 >= Category::FIRST_ACTION)
    );
}

#[test]
fn axes_run_along_the_matrix_columns() {
    let m = [0.0, -1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0, 3.0];
    let [x, y, z] = axes(Category::BoneAxes, &m, 0.5);
    assert_eq!(x.a, [1.0, 2.0, 3.0]);
    assert_eq!(x.b, [1.0, 2.5, 3.0]);
    assert_eq!(y.b, [0.5, 2.0, 3.0]);
    assert_eq!(z.b, [1.0, 2.0, 3.5]);
}

/// Runs `test` on an empty block, on a thread with stack enough for one (about a megabyte).
fn with_block(test: impl FnOnce(&mut DebugBlock) + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let mut block = DebugBlock::EMPTY;
            test(&mut block);
        })
        .unwrap()
        .join()
        .unwrap();
}

/// Each category's place in [`Category::ALL`]. The match is exhaustive: a new category does not
/// compile here until it is given the next place, and the test below fails until `ALL` has it there.
fn place(category: Category) -> usize {
    match category {
        Category::Controllers => 0,
        Category::Skeleton => 1,
        Category::BoneAxes => 2,
        Category::Fingers => 3,
        Category::HandTargets => 4,
        Category::Aim => 5,
        Category::Swings => 6,
        Category::Melee => 7,
        Category::RoomScale => 8,
        Category::MenuReach => 9,
        Category::Probe => 10,
        Category::HideHud => 11,
        Category::SkipHandRig => 12,
        Category::Depth => 13,
        Category::Targets => 14,
        Category::Stats => 15,
        Category::FreezeWorld => 16,
    }
}

#[test]
fn all_lists_every_category_once() {
    const PLACES: usize = 17;
    assert_eq!(Category::ALL.len(), PLACES, "a category was given a place but not added to ALL");
    for (at, category) in Category::ALL.into_iter().enumerate() {
        assert_eq!(place(category), at, "{category:?}");
    }
}
