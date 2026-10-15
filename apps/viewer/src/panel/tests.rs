use super::*;
use monaka_core::math::rotate;

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

#[test]
fn the_panel_lies_on_the_back_of_the_right_hand_top_toward_the_head_up() {
    // The head's up along the knuckles: the top goes there, the right toward the little finger.
    let q = facing_up([0.3, 0.0, -0.9], 1.0);
    assert!(close(rotate(q, [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]), "faces out of the back of the hand");
    assert!(close(rotate(q, [0.0, 1.0, 0.0]), [0.0, 0.0, -1.0]), "its top toward the knuckles");
    assert!(close(rotate(q, [1.0, 0.0, 0.0]), [0.0, -1.0, 0.0]), "its right toward the little finger");
    // Thumb up (the head's up toward the thumb): the top goes there, the right toward the fingers.
    let q = facing_up([0.0, 2.0, 0.0], 1.0);
    assert!(close(rotate(q, [0.0, 1.0, 0.0]), [0.0, 1.0, 0.0]));
    assert!(close(rotate(q, [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]));
    assert!(close(rotate(q, [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
    // The head's up straight out of the back of the hand: toward the knuckles.
    assert!(close(rotate(facing_up([1.0, 0.05, 0.0], 1.0), [0.0, 1.0, 0.0]), [0.0, 0.0, -1.0]));
}

#[test]
fn the_left_hand_mirrors_its_sides() {
    use monaka_contract::protocol::LEFT_HAND;
    assert_eq!(facing(Side::BackOfHand, LEFT_HAND), -1.0, "the left grip's -X leaves the back of the hand");
    assert_eq!(facing(Side::Palm, LEFT_HAND), 1.0);
    assert!(offset(Side::BackOfHand, LEFT_HAND)[0] < 0.0 && offset(Side::Palm, LEFT_HAND)[0] > 0.0);
    // On the back of the left hand, knuckles up: the right side toward the thumb (+Y), unmirrored.
    let q = facing_up([0.0, 0.0, -1.0], facing(Side::BackOfHand, LEFT_HAND));
    assert!(close(rotate(q, [0.0, 0.0, 1.0]), [-1.0, 0.0, 0.0]));
    assert!(close(rotate(q, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
}

#[test]
fn the_palm_panel_faces_out_of_the_palm_unmirrored() {
    // Palm turned to the eyes, fingers up: the top toward the fingers, the right toward the thumb.
    let q = facing_up([0.0, 0.0, -1.0], -1.0);
    assert!(close(rotate(q, [0.0, 0.0, 1.0]), [-1.0, 0.0, 0.0]), "faces out of the palm");
    assert!(close(rotate(q, [0.0, 1.0, 0.0]), [0.0, 0.0, -1.0]), "its top toward the fingers");
    assert!(close(rotate(q, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]), "its right toward the thumb");
}

#[test]
fn axes_round_trip_through_a_quaternion() {
    for (x, y, z) in [
        ([0.0, -1.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
    ] {
        let q = from_axes(x, y, z);
        assert!(close(rotate(q, [1.0, 0.0, 0.0]), x) && close(rotate(q, [0.0, 1.0, 0.0]), y) && close(rotate(q, [0.0, 0.0, 1.0]), z), "{x:?} {y:?} {z:?}");
    }
}
