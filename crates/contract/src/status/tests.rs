use super::*;

#[test]
fn states_read_back_from_their_values() {
    for state in State::ALL {
        assert_eq!(State::from_u32(state as u32), Some(state));
    }
    assert_eq!(State::from_u32(State::ALL.len() as u32), None);
    assert_eq!(State::from_u32(u32::MAX), None);
}
