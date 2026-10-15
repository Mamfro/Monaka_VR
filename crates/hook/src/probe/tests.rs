use super::*;
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};

unsafe extern "system" fn shimmed_detour(a: usize, b: usize, _: usize, caller: usize) -> usize {
    assert_eq!((a, b), (7, 9));
    caller
}

crate::caller_shim!(shim_stub => shimmed_detour);

/// Calls through the stub from a small function of its own, returning what the detour saw.
#[inline(never)]
fn call_through_shim() -> usize {
    // SAFETY: the stub forwards to `shimmed_detour`, which takes two arguments and the caller.
    let shimmed: unsafe extern "system" fn(usize, usize) -> usize = unsafe { as_detour(shim_stub) };
    unsafe { shimmed(std::hint::black_box(7), std::hint::black_box(9)) }
}

#[test]
fn the_shimmed_detour_sees_its_call_site() {
    let caller = call_through_shim();
    let here = call_through_shim as fn() -> usize as usize;
    assert!(caller > here && caller < here + 256, "caller {caller:#x} is inside the calling function ({here:#x})");
}

#[test]
fn floats_matching_an_angle() {
    let block: [f32; 6] = [1.0, -40.75, 0.7112, 9.0, (-0.7112f32).sin(), 40.75];
    let found = find_floats(block.as_ptr() as usize, size_of_val(&block), angle_matcher(-0.7112));
    let labels: Vec<_> = found.iter().map(|&(offset, _, label)| (offset, label)).collect();
    assert_eq!(labels, vec![(4, "deg"), (8, "-rad"), (16, "sin"), (20, "-deg")]);
    assert!(find_floats(8, 64, angle_matcher(0.5)).is_empty(), "unreadable memory is skipped");
}

#[test]
fn rust_objects_have_no_rtti() {
    let plain: [usize; 4] = [0; 4];
    assert_eq!(class_name(plain.as_ptr() as usize), None);
}

static SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
unsafe extern "system" fn detour() {
    SEEN.store(caller(), std::sync::atomic::Ordering::SeqCst);
}

#[test]
fn caller_is_the_call_site() {
    // sub rsp,0x28; mov rax,imm64; call rax; add rsp,0x28; ret
    let mut code = vec![0x48, 0x83, 0xec, 0x28, 0x48, 0xb8];
    code.extend_from_slice(&(detour as unsafe extern "system" fn() as usize as u64).to_le_bytes());
    code.extend_from_slice(&[0xff, 0xd0, 0x48, 0x83, 0xc4, 0x28, 0xc3]);
    // SAFETY: a fresh RWX page receives the test's machine code, then runs it.
    unsafe {
        let page = VirtualAlloc(None, 4096, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE) as usize;
        std::ptr::copy_nonoverlapping(code.as_ptr(), page as *mut u8, code.len());
        let stub: unsafe extern "system" fn() = std::mem::transmute(page);
        stub();
        let seen = SEEN.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(seen, page + 16, "the address after `call rax` (page {page:#x}, seen {seen:#x})");
    }
}
