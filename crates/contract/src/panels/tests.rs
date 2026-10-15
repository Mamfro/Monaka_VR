use super::*;

const MAP: Place = Place { hand: RIGHT_HAND, side: Side::BackOfHand, width: 0.08, beside: false };
const LIST: Place = Place { hand: RIGHT_HAND, side: Side::BackOfHand, width: 0.2, beside: true };

#[test]
fn declared_pieces_read_back_in_order() {
    let mut block = PanelsBlock::default();
    block.start();
    assert!(block.is_compatible() && block.pieces().next().is_none());
    assert_eq!(block.declare([("minimap", MAP), ("quests", LIST)]), 2);
    assert_eq!(block.pieces().collect::<Vec<_>>(), [("minimap", MAP), ("quests", LIST)]);
}

#[test]
fn a_piece_the_block_cannot_hold_is_left_out() {
    let mut block = PanelsBlock::default();
    let long = "x".repeat(PANEL_NAME_LEN + 1);
    let no_hand = Place { hand: 2, ..MAP };
    assert_eq!(block.declare([(long.as_str(), MAP), ("weapon", no_hand), ("", MAP), ("tool", MAP)]), 1);
    assert_eq!(block.pieces().map(|(name, _)| name).collect::<Vec<_>>(), ["tool"]);
    let many: Vec<String> = (0..PANEL_PIECES + 2).map(|i| format!("piece{i}")).collect();
    assert_eq!(block.declare(many.iter().map(|n| (n.as_str(), MAP))), PANEL_PIECES);
}

#[test]
fn a_record_written_otherwise_is_refused() {
    let mut record = PanelPlace::new("health", MAP).unwrap();
    record.side = 7;
    assert_eq!(record.get(), None);
    let mut record = PanelPlace::new("health", MAP).unwrap();
    record.width = f32::NAN;
    assert_eq!(record.get(), None);
}

#[test]
fn panels_block_layout() {
    assert_eq!(size_of::<PanelPlace>(), 40);
    assert_eq!(size_of::<PanelsBlock>(), 24 + 40 * PANEL_PIECES);
}
