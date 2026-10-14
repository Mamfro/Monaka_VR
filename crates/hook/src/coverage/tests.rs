use super::*;

#[inline(never)]
extern "system" fn first(object: u64, b: u64) -> u64 {
    let mut sum = object ^ b;
    for k in 0..3u64 {
        sum = std::hint::black_box(sum.rotate_left(5) ^ k);
    }
    sum
}

#[inline(never)]
extern "system" fn second(a: u64) -> u64 {
    let mut sum = a;
    for k in 0..3u64 {
        sum = std::hint::black_box(sum.wrapping_mul(7) ^ k);
    }
    sum
}

#[inline(never)]
extern "system" fn third(a: u64) -> u64 {
    std::hint::black_box(a.wrapping_add(3).rotate_right(2))
}

/// Arms three functions of this test, calls some, and reads what ran: counts within the budget,
/// the order, the value's register, what ran since a mark, and a narrowing round.
#[test]
fn what_runs_is_counted_and_the_code_comes_back() {
    let f = std::hint::black_box(first as extern "system" fn(u64, u64) -> u64);
    let g = std::hint::black_box(second as extern "system" fn(u64) -> u64);
    let h = std::hint::black_box(third as extern "system" fn(u64) -> u64);
    let expected = (f(1, 2), g(3), h(4));
    let mut starts = vec![first as extern "system" fn(u64, u64) -> u64 as usize, second as extern "system" fn(u64) -> u64 as usize, third as extern "system" fn(u64) -> u64 as usize];
    starts.sort_unstable();
    assert_eq!(arm_functions(starts.clone(), 3).expect("armed"), 3);
    value(Some(0xfeed));

    for _ in 0..5 {
        assert_eq!(f(1, 2), expected.0, "the function runs as it did");
    }
    mark();
    assert_eq!(g(0xfeed), g(0xfeed));
    let ran = hits();
    let of = |address: usize| ran.iter().find(|h| h.start == address).copied();
    assert_eq!(of(first as extern "system" fn(u64, u64) -> u64 as usize).map(|h| h.hits), Some(3), "three hits, its budget");
    assert_eq!(of(second as extern "system" fn(u64) -> u64 as usize).map(|h| h.matched), Some(1), "the value was in rcx");
    assert!(of(third as extern "system" fn(u64) -> u64 as usize).is_none());
    assert_eq!(since().iter().map(|h| h.start).collect::<Vec<_>>(), [second as extern "system" fn(u64) -> u64 as usize], "only second ran after the mark");
    assert_eq!(matches().len(), 1);

    // Narrowing: the two that ran, armed again; one runs; the other is what did not.
    let ran: Vec<usize> = hits().iter().map(|h| h.start).collect();
    assert_eq!(rearm(&ran).expect("rearmed"), 2);
    f(1, 2);
    assert_eq!(not_hit(), [second as extern "system" fn(u64) -> u64 as usize]);

    assert!(disarm() >= 1);
    assert!(!armed());
    assert_eq!((f(1, 2), g(3), h(4)), expected);
    // The first bytes are the code's own again.
    for start in starts {
        assert_ne!(crate::mem::read::<u8>(start), Some(INT3));
    }
}
