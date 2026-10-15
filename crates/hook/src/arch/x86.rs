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

// A `path` module: its children resolve beside it, not in `x86/`.
#[cfg(test)]
#[path = "x86/tests.rs"]
mod tests;
