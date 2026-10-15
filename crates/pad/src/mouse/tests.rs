use super::*;

#[test]
fn object_data_and_caps_have_directinputs_sizes() {
    assert_eq!(size_of::<ObjectData>(), 16 + size_of::<usize>());
    assert_eq!(size_of::<DevCaps>(), 44);
}

#[test]
fn whole_counts_go_out_and_fractions_stay_per_source() {
    add(2.75, -1.5);
    add_other(1.5, 0.0);
    let taken = take();
    assert_eq!(taken, [[2, -1], [1, 0]]);
    assert_eq!(total(&taken), [3, -1]);
    let rest = pending();
    assert!((rest[0] - 0.75).abs() < 1e-9 && (rest[1] + 0.5).abs() < 1e-9, "only the head's pending: {rest:?}");
    give_back(taken);
    assert_eq!(take(), [[2, -1], [1, 0]]);
    add(-0.75, 0.5);
    add_other(-0.5, 0.0);
    assert_eq!(take(), [[0, 0], [0, 0]]);
}
