use super::*;

#[test]
fn the_viewport_puts_the_rectangle_on_the_canvas() {
    // The layout's right quarter, its lower half: four layouts wide, two high, moved left by
    // three canvases; its bottom at the canvas's bottom.
    let [x, y, w, h] = viewport_for([0.75, 0.5, 0.25, 0.5], (100, 50)).unwrap();
    assert_eq!((w, h), (400, 100));
    assert_eq!(x, -300);
    assert_eq!(y, 0, "the layout's bottom is the rectangle's bottom");
    // The top half instead: the layout's top at the canvas's top.
    let [_, y, _, h] = viewport_for([0.75, 0.0, 0.25, 0.5], (100, 50)).unwrap();
    assert_eq!(y + h, 50);
    assert!(viewport_for([0.0, 0.0, 0.0, 0.5], (100, 50)).is_none(), "an empty rectangle");
}
