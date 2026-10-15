use super::*;

type Unary = unsafe extern "system" fn(u64) -> u64;

/// Executable memory holding `code`, as a function.
fn code(code: &[u8]) -> usize {
    // SAFETY: a fresh RWX page receives the test's machine code.
    unsafe {
        let page = VirtualAlloc(None, 4096, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE) as usize;
        assert_ne!(page, 0);
        std::ptr::copy_nonoverlapping(code.as_ptr(), page as *mut u8, code.len());
        page
    }
}

static PLAIN: Original<Unary> = Original::new();
unsafe extern "system" fn times_ten(x: u64) -> u64 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { PLAIN.get(&flight)(x) * 10 }
}

#[test]
fn inline_hook_runs_the_detour_and_restores() {
    // mov rax, rcx; add rax, 1; ret
    let target = code(&[0x48, 0x89, 0xc8, 0x48, 0x83, 0xc0, 0x01, 0xc3]);
    let function: Unary = unsafe { std::mem::transmute(target) };
    let prologue = [Instruction::plain(&[0x48, 0x89, 0xc8]), Instruction::plain(&[0x48, 0x83, 0xc0, 0x01])];
    let mut hooks = Hooks::default();
    let wrong = [Instruction::plain(&[0x48, 0x89, 0xc9]), prologue[1]];
    assert!(matches!(unsafe { hooks.inline(&PLAIN, "wrong", target, &wrong, times_ten) }, Err(Error::Signature(_))));
    unsafe { hooks.inline(&PLAIN, "plain", target, &prologue, times_ten) }.unwrap();
    assert_eq!(unsafe { function(4) }, 5);
    hooks.enable().unwrap();
    assert_eq!(unsafe { function(4) }, 50);
    hooks.disable().unwrap();
    assert_eq!(unsafe { function(4) }, 5);
}

static FIRST: Original<Unary> = Original::new();
static SECOND: Original<Unary> = Original::new();
unsafe extern "system" fn first_detour(x: u64) -> u64 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { FIRST.get(&flight)(x) * 2 }
}
unsafe extern "system" fn second_detour(x: u64) -> u64 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { SECOND.get(&flight)(x) * 3 }
}

#[test]
fn a_function_is_hooked_inline_once() {
    // mov rax, rcx; add rax, 1; ret
    let target = code(&[0x48, 0x89, 0xc8, 0x48, 0x83, 0xc0, 0x01, 0xc3]);
    let function: Unary = unsafe { std::mem::transmute(target) };
    let prologue = [Instruction::plain(&[0x48, 0x89, 0xc8]), Instruction::plain(&[0x48, 0x83, 0xc0, 0x01])];
    let mut hooks = Hooks::default();
    unsafe { hooks.inline(&FIRST, "first", target, &prologue, first_detour) }.unwrap();
    let again = unsafe { hooks.inline(&SECOND, "second", target, &prologue, second_detour) };
    assert!(matches!(&again, Err(Error::Refused(why)) if why.contains("first")), "{again:?}");
    assert!(matches!(unsafe { hooks.inline_decoded(&SECOND, "second", target, second_detour) }, Err(Error::Refused(_))));
    assert!(!SECOND.is_set(), "a refused hook leaves its original unset");
    hooks.enable().unwrap();
    assert_eq!(unsafe { function(4) }, 10, "the first hook's detour runs");
    hooks.disable().unwrap();
}

static RELATIVE: Original<Unary> = Original::new();
unsafe extern "system" fn plus_hundred(x: u64) -> u64 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { RELATIVE.get(&flight)(x) + 100 }
}

#[test]
fn rip_relative_prologue_is_relocated() {
    // mov rax, [rip+8]; add rax, rcx; ret; int3 x4; dq 7  -> returns x + 7
    let mut bytes = vec![0x48, 0x8b, 0x05, 0x08, 0, 0, 0, 0x48, 0x01, 0xc8, 0xc3, 0xcc, 0xcc, 0xcc, 0xcc];
    bytes.extend_from_slice(&7u64.to_le_bytes());
    let target = code(&bytes);
    let function: Unary = unsafe { std::mem::transmute(target) };
    let prologue = [Instruction::rip_relative(&[0x48, 0x8b, 0x05, 0x08, 0, 0, 0], 3)];
    let mut hooks = Hooks::default();
    unsafe { hooks.inline(&RELATIVE, "relative", target, &prologue, plus_hundred) }.unwrap();
    hooks.enable().unwrap();
    assert_eq!(unsafe { function(1) }, 108);
    hooks.disable().unwrap();
    assert_eq!(unsafe { function(1) }, 8);
}

type Tick = unsafe extern "system" fn() -> u64;
static TICK: Original<Tick> = Original::new();
unsafe extern "system" fn fixed_tick() -> u64 {
    42
}

#[test]
fn import_hook_redirects_only_this_module() {
    let flight = crate::InFlight::enter();
    use windows::Win32::System::SystemInformation::GetTickCount64;
    let module = crate::module::Module::containing(fixed_tick as Tick as usize).unwrap();
    let mut hooks = Hooks::default();
    let missing = unsafe { hooks.import(&TICK, "missing", &module, "kernel32.dll", "NoSuchFunction", fixed_tick as Tick) };
    assert!(missing.is_err());
    unsafe { hooks.import(&TICK, "tick", &module, "kernel32.dll", "GetTickCount64", fixed_tick as Tick) }.unwrap();
    assert!(unsafe { TICK.get(&flight)() } > 1000, "the original is the real function");
    hooks.enable().unwrap();
    assert_eq!(unsafe { GetTickCount64() }, 42);
    hooks.disable().unwrap();
    assert_ne!(unsafe { GetTickCount64() }, 42);
}

static DECODED: Original<Unary> = Original::new();
static CHAINED: Original<Unary> = Original::new();
static CHAINED_ABSOLUTE: Original<Unary> = Original::new();

#[test]
fn chained_hooks_keep_the_earlier_hook() {
    // The "earlier hook": doubles its argument. The hooked function starts with a jump to it.
    let earlier = code(&[0x48, 0x8d, 0x04, 0x09, 0xc3]); // lea rax, [rcx+rcx]; ret
    let mut relative = vec![0xe9, 0, 0, 0, 0, 0xcc, 0xcc, 0xcc];
    let target = code(&relative);
    let rel = (earlier as i64 - (target as i64 + 5)) as i32;
    relative[1..5].copy_from_slice(&rel.to_le_bytes());
    unsafe { std::ptr::copy_nonoverlapping(relative.as_ptr(), target as *mut u8, relative.len()) };
    let function: Unary = unsafe { std::mem::transmute(target) };
    assert_eq!(unsafe { function(7) }, 14);
    let mut hooks = Hooks::default();
    unsafe { hooks.inline_chained(&CHAINED, "relative", target, plus_one_thousand_chained) }.unwrap();
    hooks.enable().unwrap();
    assert_eq!(unsafe { function(7) }, 1014, "ours, then theirs");
    hooks.disable().unwrap();
    assert_eq!(unsafe { function(7) }, 14);

    let mut absolute = vec![0xff, 0x25, 0, 0, 0, 0];
    absolute.extend_from_slice(&(earlier as u64).to_le_bytes());
    let target = code(&absolute);
    let function: Unary = unsafe { std::mem::transmute(target) };
    let mut hooks = Hooks::default();
    unsafe { hooks.inline_chained(&CHAINED_ABSOLUTE, "absolute", target, plus_one_thousand_absolute) }.unwrap();
    hooks.enable().unwrap();
    assert_eq!(unsafe { function(7) }, 1014);
    hooks.disable().unwrap();
    assert_eq!(unsafe { function(7) }, 14);

    let unknown = code(&[0xe8, 0, 0, 0, 0, 0xc3]);
    let refused = unsafe { Hooks::default().inline_chained(&CHAINED, "unknown", unknown, plus_one_thousand_chained) };
    assert!(matches!(&refused, Err(Error::Signature(why)) if why.contains("e8")), "{refused:?}");
}

unsafe extern "system" fn plus_one_thousand_chained(x: u64) -> u64 {
    let flight = crate::InFlight::enter();
    unsafe { CHAINED.get(&flight)(x) + 1000 }
}

unsafe extern "system" fn plus_one_thousand_absolute(x: u64) -> u64 {
    let flight = crate::InFlight::enter();
    unsafe { CHAINED_ABSOLUTE.get(&flight)(x) + 1000 }
}
unsafe extern "system" fn plus_one_thousand(x: u64) -> u64 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { DECODED.get(&flight)(x) + 1000 }
}

#[test]
fn decoded_inline_hook_on_unknown_code() {
    // push rbx; sub rsp,0x20; mov rax,rcx; add rsp,0x20; pop rbx; ret  -> returns x
    let target = code(&[0x40, 0x53, 0x48, 0x83, 0xec, 0x20, 0x48, 0x89, 0xc8, 0x48, 0x83, 0xc4, 0x20, 0x5b, 0xc3]);
    let function: Unary = unsafe { std::mem::transmute(target) };
    let mut hooks = Hooks::default();
    unsafe { hooks.inline_decoded(&DECODED, "decoded", target, plus_one_thousand) }.unwrap();
    hooks.enable().unwrap();
    assert_eq!(unsafe { function(7) }, 1007);
    hooks.disable().unwrap();
    assert_eq!(unsafe { function(7) }, 7);
    let call = code(&[0xe8, 0, 0, 0, 0, 0xc3]);
    assert!(matches!(unsafe { Hooks::default().inline_decoded(&DECODED, "call", call, plus_one_thousand) }, Err(Error::Signature(_))));
}

static METHOD: Original<Unary> = Original::new();
unsafe extern "system" fn doubled(x: u64) -> u64 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { METHOD.get(&flight)(x) * 2 }
}
unsafe extern "system" fn identity(x: u64) -> u64 {
    x
}

#[test]
fn vtable_hook_swaps_one_method() {
    let identity_address = function_address(identity as Unary);
    let table: Box<[usize; 2]> = Box::new([0, identity_address]);
    let object: Box<usize> = Box::new(table.as_ptr() as usize);
    let object = &*object as *const usize as *mut core::ffi::c_void;
    let mut hooks = Hooks::default();
    unsafe { hooks.vtable(&METHOD, "method", object, 1, doubled) }.unwrap();
    hooks.enable().unwrap();
    let method: Unary = unsafe { std::mem::transmute(table[1]) };
    assert_eq!(unsafe { method(21) }, 42);
    hooks.disable().unwrap();
    assert_eq!(table[1], identity_address);
}

#[test]
fn hooks_near_each_other_share_a_region_of_slots() {
    let target = code(&[0xc3]);
    let first = slot_near(target).unwrap();
    let second = slot_near(target).unwrap();
    assert_ne!(first, second, "a slot is never handed out twice");
    assert_eq!(first % SLOT, 0);
    assert_eq!(second % SLOT, 0);
    assert!(first.abs_diff(second) < REGION, "both from one region");
    assert!(first.abs_diff(target) < REACH && second.abs_diff(target) < REACH);
}
