use super::*;

#[test]
fn the_panel_viewport_puts_the_rectangle_on_the_panel() {
    let base = D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: 1000.0, Height: 500.0, MinDepth: 0.0, MaxDepth: 1.0 };
    // The layout's right quarter, lower half: four panels wide, two high, moved left by three.
    let view = panel_viewport(base, [0.75, 0.5, 0.25, 0.5], (100, 50));
    assert_eq!((view.Width, view.Height), (400.0, 100.0));
    assert_eq!((view.TopLeftX, view.TopLeftY), (-300.0, -50.0));
}
