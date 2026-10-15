use super::*;

fn blank() -> PageBlock {
    let mut block = PageBlock { stamp: Stamp::default(), generation: 0, written_tick: 0, page: Page::default(), presses: [PagePress::default(); PAGE_PRESSES], press_sequence: 0 };
    block.start();
    block
}

#[test]
fn text_is_cut_at_a_character() {
    let cut: [u8; 4] = fixed("aé日b");
    assert_eq!(text_of(&cut), "aé", "the 3-byte character does not fit after 3 bytes");
    let row = Row::button(7, "Turn test", "Moves the mouse");
    assert_eq!((row.id, row.kind(), row.label(), row.value()), (7, Kind::Button, "Turn test", "Moves the mouse"));
}

#[test]
fn a_page_lives_while_written() {
    let mut block = blank();
    assert!(block.fresh(10).is_none(), "never written");
    block.write(&Page::new("Explorer", &[Row::heading("Camera"), Row::text("Best", "VP +64").tone(Tone::Good)]), 1000);
    let page = block.fresh(2000).unwrap();
    assert_eq!((page.title(), page.rows().len(), page.rows()[1].shade()), ("Explorer", 2, Tone::Good));
    assert!(block.fresh(1000 + PAGE_FRESH_MS + 1).is_none(), "its producer stopped");
}

#[test]
fn presses_reach_the_producer_once_and_in_order() {
    let mut block = blank();
    block.press(3, Press::Switch);
    block.press(4, Press::Right);
    let (presses, seen) = block.presses_since(0);
    assert_eq!(presses, [(3, Press::Switch), (4, Press::Right)]);
    assert_eq!(block.presses_since(seen).0, []);
    for k in 0..(PAGE_PRESSES as u32 + 5) {
        block.press(k, Press::Left);
    }
    let (presses, _) = block.presses_since(seen);
    assert_eq!(presses.len(), PAGE_PRESSES, "the oldest were overwritten");
    assert_eq!(presses.last(), Some(&(PAGE_PRESSES as u32 + 4, Press::Left)));
}
