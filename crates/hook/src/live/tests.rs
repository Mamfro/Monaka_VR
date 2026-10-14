use super::*;

/// A function as a game's: integer, float and stack arguments, a body long enough to hook.
#[inline(never)]
extern "system" fn target(object: *mut u32, b: u64, c: f32, d: u64, e: u64, f: u64) -> u64 {
    let mut sum = b.wrapping_add(d).wrapping_add(e).wrapping_add(f);
    for k in 0..4u64 {
        sum = std::hint::black_box(sum.rotate_left(3) ^ k);
    }
    // SAFETY: the tests pass a live object of four u32s.
    unsafe { *object.add(1) += 1 };
    sum.wrapping_add(c as u64)
}

#[test]
fn the_stub_saves_and_restores_what_it_must() {
    let code = stub_code(7, 0x1111_2222_3333_4444, 0x5555_6666_7777_8888);
    // The id, the recorder and the trampoline are where the instructions take them.
    let find = |value: u64| code.windows(8).position(|w| w == value.to_le_bytes());
    assert!(find(7).is_some() && find(0x1111_2222_3333_4444).is_some() && find(0x5555_6666_7777_8888).is_some());
    // Seven pushes, seven pops; ends in jmp [r11].
    assert_eq!(&code[code.len() - 3..], [0x41, 0xFF, 0x23]);
}

#[test]
fn changed_words_are_listed() {
    assert_eq!(changed(&[1, 0, 0, 0, 5, 5, 5, 5], &[1, 0, 0, 0, 5, 6, 5, 5]), [(4, [5, 5, 5, 5], [5, 6, 5, 5])]);
}

#[test]
fn a_probe_records_each_call_and_comes_out() {
    let mut object = [10u32, 20, 30, 40];
    let call = |object: &mut [u32; 4]| std::hint::black_box(target as extern "system" fn(*mut u32, u64, f32, u64, u64, u64) -> u64)(object.as_mut_ptr(), 2, 1.5, 4, 5, 6);
    let expected = call(&mut object);
    let address = target as extern "system" fn(*mut u32, u64, f32, u64, u64, u64) -> u64 as usize;
    let id = add(Spec { address, name: "target".into(), snapshot: Some(Snapshot { register: Register::Rcx, offset: 0, length: 16 }), only: None, returns: false }).expect("added");

    // Calls go on as before, recorded.
    assert_eq!(call(&mut object), expected);
    let recorded = calls(id, 8);
    assert_eq!(recorded.len(), 1);
    let c = recorded[0];
    assert_eq!((c.args[0], c.args[1], c.args[3]), (object.as_ptr() as u64, 2, 4));
    assert_eq!(c.floats[2], 1.5);
    assert_eq!(c.stack, [5, 6]);
    // SAFETY: a plain query on our own thread.
    assert_eq!(c.thread, unsafe { GetCurrentThreadId() });
    assert!(crate::module::Module::containing(c.caller).is_some(), "the caller is in this test's module");

    // The snapshot is taken on entry: the field the call changes differs between two calls.
    mark(id).expect("marked");
    call(&mut object);
    let fields = diff(id).expect("diff");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].0, 4, "the second field");

    present();
    assert!(timeline(2).iter().any(|e| matches!(e, Event::Present { .. })));
    let summary = probes().into_iter().find(|p| p.id == id).expect("listed");
    assert_eq!(summary.count, 2);
    // One call site, or one per place the optimizer copied the helper into.
    assert!(!summary.callers.is_empty() && summary.callers.iter().all(|(c, _)| crate::module::Module::containing(*c).is_some()));

    remove(id).expect("removed");
    call(&mut object);
    assert_eq!(calls(id, 8).len(), 2, "not recorded once removed");
    assert_eq!(call(&mut object), expected.wrapping_add(0), "the game's code runs as it did");
}

/// A function returning an integer and a float, for the return probe.
#[inline(never)]
extern "system" fn returning(a: u64, b: u64) -> u64 {
    let mut sum = a;
    for k in 0..b {
        sum = std::hint::black_box(sum.wrapping_mul(3).wrapping_add(k));
    }
    sum
}

#[inline(never)]
extern "system" fn returning_float(x: f32) -> f32 {
    let mut y = x;
    for _ in 0..3 {
        y = std::hint::black_box(y * 2.0);
    }
    y
}

#[test]
fn a_return_probe_records_what_each_call_returned_and_goes_back() {
    let int = std::hint::black_box(returning as extern "system" fn(u64, u64) -> u64);
    let float = std::hint::black_box(returning_float as extern "system" fn(f32) -> f32);
    let (expected_int, expected_float) = (int(5, 7), float(1.5));
    let a = add(Spec { address: returning as extern "system" fn(u64, u64) -> u64 as usize, name: "returning".into(), snapshot: None, only: None, returns: true }).expect("added");
    let b = add(Spec { address: returning_float as extern "system" fn(f32) -> f32 as usize, name: "returning_float".into(), snapshot: None, only: None, returns: true }).expect("added");
    // Calls return what they did, to where they were called from, nested in a loop.
    for _ in 0..3 {
        assert_eq!(int(5, 7), expected_int);
        assert_eq!(float(1.5), expected_float);
    }
    let ints = calls(a, 8);
    assert_eq!(ints.len(), 3);
    assert!(ints.iter().all(|c| c.returned.is_some_and(|r| r.rax == expected_int)), "{ints:?}");
    assert!(calls(b, 8).iter().all(|c| c.returned.is_some_and(|r| r.xmm0 == expected_float)));
    remove(a).expect("removed");
    remove(b).expect("removed");
    assert_eq!(int(5, 7), expected_int);
    assert!(PENDING.lock().expect("pending").as_ref().is_none_or(|p| p.is_empty()), "every call returned through its stub");
}
