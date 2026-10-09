//! What hooking needs to know about 32-bit x86 (32-bit games).
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_CONTROL_X86, CONTEXT_DEBUG_REGISTERS_X86, CONTEXT_FLAGS};

/// `push imm32; ret`: runs the same from anywhere.
pub const RELAY_SIZE: usize = 6;

pub fn absolute_jump(target: usize) -> [u8; RELAY_SIZE] {
    let mut code = [0x68, 0, 0, 0, 0, 0xc3];
    code[1..5].copy_from_slice(&(target as u32).to_le_bytes());
    code
}

/// The length of an absolute jump at the start of `code` (another hook's relay), if one is there:
/// `push imm32; ret`, or `jmp [abs32]`.
pub fn absolute_jump_at(code: &[u8]) -> Option<usize> {
    match code {
        [0x68, _, _, _, _, 0xc3, ..] | [0xff, 0x25, ..] => Some(6),
        _ => None,
    }
}

/// The rel32 that reaches `to` from an instruction ending at `from_end`: everything is in reach,
/// since the address wraps at 4 GB.
pub fn relative32(from_end: usize, to: usize) -> Option<i32> {
    Some(to.wrapping_sub(from_end) as u32 as i32)
}

/// No REX prefixes (0x40..=0x4f are `inc` and `dec`), and ModRM's `[disp32]` form is absolute.
pub const LONG_MODE: bool = false;

/// Where a loaded image's data directories start in its optional header (PE32+ here; PE32 keeps
/// 16 bytes fewer before them). Their count is the 4 bytes before.
pub const DIRECTORIES: usize = 96;

pub const CONTROL: CONTEXT_FLAGS = CONTEXT_CONTROL_X86;
pub const DEBUG_REGISTERS: CONTEXT_FLAGS = CONTEXT_DEBUG_REGISTERS_X86;

pub fn instruction_pointer(context: &CONTEXT) -> usize {
    context.Eip as usize
}

pub fn stack_pointer(context: &CONTEXT) -> usize {
    context.Esp as usize
}

/// Debug registers 0-3 set to `addresses`.
pub fn set_breakpoints(context: &mut CONTEXT, addresses: [usize; 4]) {
    [context.Dr0, context.Dr1, context.Dr2, context.Dr3] = addresses.map(|a| a as u32);
}

pub fn set_dr7(context: &mut CONTEXT, value: usize) {
    context.Dr7 = value as u32;
}

#[cfg(test)]
mod tests {
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
        // SAFETY: the original has the same signature.
        unsafe { DESCRIBED.get()(x) * 10 }
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
        // SAFETY: the original has the same signature.
        unsafe { DECODED.get()(x) + 100 }
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
        // SAFETY: the original has the same signature.
        unsafe { CHAINED.get()(x) * 2 }
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
}
