use super::seldom;

#[test]
fn the_first_counts_then_powers_of_two() {
    let logged: Vec<u64> = (1..=40).filter(|&n| seldom(n, 5)).collect();
    assert_eq!(logged, [1, 2, 3, 4, 5, 8, 16, 32]);
}

#[test]
fn with_none_first_only_powers_of_two() {
    let logged: Vec<u64> = (1..=20).filter(|&n| seldom(n, 0)).collect();
    assert_eq!(logged, [1, 2, 4, 8, 16]);
}
