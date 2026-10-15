use crate::Original;
use crate::patch::{Hooks, Instruction};
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};

type Unary = unsafe extern "C" fn(u32) -> u32;

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

/// `push ebp; mov ebp, esp; mov eax, [ebp+8]; add eax, 1; pop ebp; ret`: x + 1, with the
/// frame MSVC's 32-bit functions start with.
const PLUS_ONE: [u8; 11] = [0x55, 0x8b, 0xec, 0x8b, 0x45, 0x08, 0x83, 0xc0, 0x01, 0x5d, 0xc3];

static DESCRIBED: Original<Unary> = Original::new();
unsafe extern "C" fn times_ten(x: u32) -> u32 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { DESCRIBED.get(&flight)(x) * 10 }
}

#[test]
fn a_described_prologue_is_hooked_and_restored() {
    let target = code(&PLUS_ONE);
    let function: Unary = unsafe { std::mem::transmute(target) };
    let prologue = [Instruction::plain(&[0x55]), Instruction::plain(&[0x8b, 0xec]), Instruction::plain(&[0x8b, 0x45, 0x08])];
    let mut hooks = Hooks::default();
    unsafe { hooks.inline(&DESCRIBED, "described", target, &prologue, times_ten) }.unwrap();
    assert_eq!(unsafe { function(4) }, 5);
    hooks.enable().unwrap();
    assert_eq!(unsafe { function(4) }, 50);
    hooks.disable().unwrap();
    assert_eq!(unsafe { function(4) }, 5);
}

static DECODED: Original<Unary> = Original::new();
unsafe extern "C" fn plus_hundred(x: u32) -> u32 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { DECODED.get(&flight)(x) + 100 }
}

#[test]
fn a_decoded_prologue_is_hooked_and_chained() {
    let target = code(&PLUS_ONE);
    let function: Unary = unsafe { std::mem::transmute(target) };
    let mut first = Hooks::default();
    unsafe { first.inline_decoded(&DECODED, "decoded", target, plus_hundred) }.unwrap();
    first.enable().unwrap();
    assert_eq!(unsafe { function(4) }, 105);
    // A second hook over the first one's jump keeps it running.
    let mut second = Hooks::default();
    unsafe { second.inline_chained(&CHAINED, "chained", target, times_two) }.unwrap();
    second.enable().unwrap();
    assert_eq!(unsafe { function(4) }, 210);
    second.disable().unwrap();
    first.disable().unwrap();
    assert_eq!(unsafe { function(4) }, 5);
}

static CHAINED: Original<Unary> = Original::new();
unsafe extern "C" fn times_two(x: u32) -> u32 {
    let flight = crate::InFlight::enter();
    // SAFETY: the original has the same signature.
    unsafe { CHAINED.get(&flight)(x) * 2 }
}

type Tick = unsafe extern "system" fn() -> u64;
static TICK: Original<Tick> = Original::new();
unsafe extern "system" fn fixed_tick() -> u64 {
    42
}

#[test]
fn import_hooks_read_32_bit_import_tables() {
    use windows::Win32::System::SystemInformation::GetTickCount64;
    let module = crate::module::Module::containing(fixed_tick as Tick as usize).unwrap();
    assert!(module.imported_dlls().iter().any(|d| d.eq_ignore_ascii_case("kernel32.dll")));
    let mut hooks = Hooks::default();
    unsafe { hooks.import(&TICK, "tick", &module, "kernel32.dll", "GetTickCount64", fixed_tick as Tick) }.unwrap();
    hooks.enable().unwrap();
    assert_eq!(unsafe { GetTickCount64() }, 42);
    hooks.disable().unwrap();
    assert_ne!(unsafe { GetTickCount64() }, 42);
}

#[test]
fn exception_frames_and_pushes_decode() {
    // push ebp; mov ebp,esp; push -1; push imm32; mov eax, fs:[0]
    let bytes = [0x55, 0x8b, 0xec, 0x6a, 0xff, 0x68, 1, 2, 3, 4, 0x64, 0xa1, 0, 0, 0, 0];
    let lengths: Vec<usize> = crate::decode::prologue(&bytes, 16).unwrap().iter().map(|d| d.length).collect();
    assert_eq!(lengths, [1, 2, 2, 5, 6]);
    // inc eax is an instruction here, not a prefix; [disp32] is absolute, not relocated.
    assert_eq!(crate::decode::instruction(&[0x40, 0x90]).map(|d| d.length), Some(1));
    let absolute = crate::decode::instruction(&[0x8b, 0x0d, 1, 2, 3, 4]).unwrap();
    assert_eq!((absolute.length, absolute.rip_displacement), (6, None));
}
