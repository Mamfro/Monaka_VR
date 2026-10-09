//! What hooking needs to know about x86-64 (64-bit games).
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_CONTROL_AMD64, CONTEXT_DEBUG_REGISTERS_AMD64, CONTEXT_FLAGS};

/// `jmp [rip+0]` and the 8-byte target after it: runs the same from anywhere.
pub const RELAY_SIZE: usize = 14;

pub fn absolute_jump(target: usize) -> [u8; RELAY_SIZE] {
    let mut code = [0u8; RELAY_SIZE];
    code[..6].copy_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
    code[6..].copy_from_slice(&(target as u64).to_le_bytes());
    code
}

/// The length of an absolute jump at the start of `code` (another hook's relay), if one is there.
pub fn absolute_jump_at(code: &[u8]) -> Option<usize> {
    code.starts_with(&[0xff, 0x25, 0, 0, 0, 0]).then_some(RELAY_SIZE)
}

/// The rel32 that reaches `to` from an instruction ending at `from_end`, if it is in reach.
pub fn relative32(from_end: usize, to: usize) -> Option<i32> {
    i32::try_from(to as i64 - from_end as i64).ok()
}

/// REX prefixes (0x40..=0x4f) exist, and ModRM's `[disp32]` form is rip-relative.
pub const LONG_MODE: bool = true;

/// Where a loaded image's data directories start in its optional header (PE32+ here; PE32 keeps
/// 16 bytes fewer before them). Their count is the 4 bytes before.
pub const DIRECTORIES: usize = 112;

pub const CONTROL: CONTEXT_FLAGS = CONTEXT_CONTROL_AMD64;
pub const DEBUG_REGISTERS: CONTEXT_FLAGS = CONTEXT_DEBUG_REGISTERS_AMD64;

pub fn instruction_pointer(context: &CONTEXT) -> usize {
    context.Rip as usize
}

pub fn stack_pointer(context: &CONTEXT) -> usize {
    context.Rsp as usize
}

/// Debug registers 0-3 set to `addresses`.
pub fn set_breakpoints(context: &mut CONTEXT, addresses: [usize; 4]) {
    [context.Dr0, context.Dr1, context.Dr2, context.Dr3] = addresses.map(|a| a as u64);
}

pub fn set_dr7(context: &mut CONTEXT, value: usize) {
    context.Dr7 = value as u64;
}
