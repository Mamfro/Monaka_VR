use super::*;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12Device};

/// The tests that set the run's pieces take turns.
static TURNS: Mutex<()> = Mutex::new(());

fn state(pieces: &'static [Piece], boxes: Option<Boxes>) -> State {
    let channel = ChannelName::new("monaka-hud-panels-test".to_string()).unwrap();
    State { channel: Arc::new(channel), pieces, masked: false, margin: MARGIN, boxes, see_through: false, shown: vec![None; pieces.len()], _declared: None }
}

#[test]
fn a_box_becomes_the_panels_shape() {
    let rect = rect_of(&[0.846, 0.710, 0.921, 0.749], (2644.0, 2644.0), (768, 384), MARGIN);
    assert!((rect[2] / rect[3] - 2.0).abs() < 1e-4, "{rect:?}");
    assert!((rect[0] + rect[2] / 2.0 - 0.8835).abs() < 1e-4 && (rect[1] + rect[3] / 2.0 - 0.7295).abs() < 1e-4);
    let rect = rect_of(&[0.80, 0.85, 0.95, 0.95], (1920.0, 1080.0), (768, 384), MARGIN);
    assert!(((rect[2] * 1920.0) / (rect[3] * 1080.0) - 2.0).abs() < 1e-3, "{rect:?}");
}

#[test]
fn a_smaller_margin_takes_less_around_the_box() {
    let found = [0.4, 0.05, 0.6, 0.06];
    let (wide, narrow) = (rect_of(&found, (1920.0, 1080.0), (1280, 96), MARGIN), rect_of(&found, (1920.0, 1080.0), (1280, 96), 0.02));
    assert!(narrow[2] < wide[2] && narrow[3] < wide[3], "{narrow:?} against {wide:?}");
}

#[test]
fn a_shown_piece_grows_and_a_hidden_one_starts_afresh() {
    let _turn = TURNS.locked();
    const PIECES: &[Piece] = &[Piece { name: "weapon", size: (768, 384), place: places::WEAPON }];
    *STATE.locked() = Some(state(PIECES, None));
    show(0, [80.0, 70.0, 90.0, 75.0], (100.0, 100.0));
    show(0, [85.0, 72.0, 95.0, 74.0], (100.0, 100.0));
    let found = |s: &Option<State>| s.as_ref().unwrap().shown[0].unwrap().found;
    assert_eq!(found(&STATE.locked()), [0.8, 0.7, 0.95, 0.75]);
    assert!(frame().unwrap().rects[0].is_some());
    hide(0);
    assert!(frame().unwrap().rects[0].is_none());
    show(0, [85.0, 72.0, 95.0, 74.0], (100.0, 100.0));
    assert_eq!(found(&STATE.locked()), [0.85, 0.72, 0.95, 0.74]);
    // Another layout size: measured afresh.
    show(0, [10.0, 10.0, 20.0, 20.0], (200.0, 200.0));
    assert_eq!(found(&STATE.locked()), [0.05, 0.05, 0.1, 0.1]);
    STATE.locked().take();
}

/// A layer-crops game's boxes: the first piece at a corner of the layer, the second not shown.
fn two_boxes(boxes: &mut [Option<[f32; 4]>]) {
    boxes[0] = Some([100.0, 200.0, 300.0, 300.0]);
    boxes[1] = None;
}

#[test]
fn a_layer_games_boxes_show_and_hide_its_pieces() {
    let _turn = TURNS.locked();
    const PIECES: &[Piece] = &[Piece { name: "health", size: (768, 256), place: places::HEALTH }, Piece { name: "tool", size: (768, 384), place: places::TOOL }];
    *STATE.locked() = Some(state(PIECES, Some(two_boxes)));
    show(1, [0.0, 0.0, 10.0, 10.0], (1000.0, 500.0));
    take_boxes((1000, 500));
    let rects = frame().unwrap().rects;
    assert!(rects[1].is_none(), "a piece the game no longer shows is hidden");
    // The first piece's rectangle, in the layer's pixels, holds its box and has its panel's shape.
    let r = pixels(rects[0].unwrap(), (1000, 500));
    assert!(r[0] <= 100.0 && r[1] <= 200.0 && r[0] + r[2] >= 300.0 && r[1] + r[3] >= 300.0, "{r:?}");
    assert!((r[2] / r[3] - 3.0).abs() < 1e-3, "{r:?}");
    // Without boxes from the game (a draw-level game) nothing changes.
    STATE.locked().as_mut().unwrap().boxes = None;
    take_boxes((1000, 500));
    assert!(frame().unwrap().rects[0].is_some());
    STATE.locked().take();
}

fn device() -> ID3D12Device {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: creates a device on the default adapter.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    device.unwrap()
}

#[test]
fn the_d3d12_shaders_compile_and_the_passes_initialise() {
    d3d12::compiles().unwrap();
    let device = device();
    const PIECES: &[Piece] = &[Piece { name: "minimap", size: (1280, 96), place: places::MINIMAP }, Piece { name: "tool", size: (768, 384), place: places::TOOL }, Piece { name: "weapon", size: (384, 384), place: places::WEAPON }];
    assert_eq!(d3d12::made(&device, PIECES).unwrap(), PIECES.len());
    d3d12::take_out::make(&device).unwrap();
}

#[test]
fn a_piece_taken_out_is_clamped_to_whole_pixels_of_the_frame() {
    let clamp = |rect, size| d3d12::take_out::clamp(rect, size).map(|b| (b.left, b.top, b.right, b.bottom));
    assert_eq!(clamp([10.4, 5.6, 20.0, 10.0], (100, 100)), Some((10, 5, 31, 16)));
    assert_eq!(clamp([90.0, 90.0, 20.0, 20.0], (100, 100)), Some((90, 90, 100, 100)));
    assert!(clamp([-50.0, 0.0, 10.0, 10.0], (100, 100)).is_none());
}

#[test]
fn a_panel_goes_out_while_shown_and_once_cleared() {
    let mut states = PanelStates::default();
    assert!(states.start(2), "the frame's first piece draw");
    assert!(!states.start(2), "only once a frame");
    states.ready(0);
    assert!(states.is_ready(0) && !states.is_ready(1));
    assert_eq!(states.end(), [Some(PanelOut::Shown), None]);
    assert!(states.went_out(0, PanelOut::Shown), "shown: a change");
    // The next frame: piece 0 still shows, then stops.
    assert!(states.start(2));
    states.ready(0);
    assert_eq!(states.end(), [Some(PanelOut::Shown), None]);
    assert!(!states.went_out(0, PanelOut::Shown), "still shown: no change");
    assert!(states.start(2));
    assert_eq!(states.end(), [Some(PanelOut::Cleared), None]);
    assert!(states.went_out(0, PanelOut::Cleared));
    assert!(states.start(2));
    assert_eq!(states.end(), [None, None], "cleared once");
}

#[test]
fn a_clear_that_did_not_go_out_is_tried_again() {
    let mut states = PanelStates::default();
    states.went_out(1, PanelOut::Shown);
    assert_eq!(states.out(1, false), Some(PanelOut::Cleared));
    // The publish was skipped (no slot free): nothing recorded, so it is cleared again.
    assert_eq!(states.out(1, false), Some(PanelOut::Cleared));
    states.went_out(1, PanelOut::Cleared);
    assert_eq!(states.out(1, false), None);
}
