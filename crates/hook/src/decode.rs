//! A deliberately small x86-64 (and 32-bit x86) decoder for function prologues, for inline hooks on code whose
//! bytes are not known in advance (a system DLL that differs between Windows builds). The lengths
//! and operands come from `monaka_core::x64`; this is the allow-list over it: the instructions
//! compilers put at function starts (pushes, stack adjustments, register moves, stores of saved
//! registers, SSE spills, sign and zero extensions), and everything else refused, so a prologue it
//! cannot fully account for is never patched. The one branch it takes is a conditional jump with a
//! 32-bit displacement (an early-out test at a function's start): relocated like any rip-relative
//! operand, and only when it lands past the bytes taken ([`prologue`]); a call, a short jump or a
//! jump back into the taken bytes is refused.

use monaka_core::x64::{self, Instruction, Map, Mode};

/// One decoded instruction: its length and, if it addresses memory relative to the instruction
/// pointer, where its 32-bit displacement sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub length: usize,
    pub rip_displacement: Option<usize>,
}

/// The code this build hooks.
const MODE: Mode = if crate::arch::LONG_MODE { Mode::Long } else { Mode::Legacy32 };

/// Whether the prefixes before the instruction's opcode are ones prologues use: those of SSE moves
/// and 16-bit operands (and REX, in 64-bit code), or 32-bit code's `fs:` before `mov eax, [moffs32]`
/// (structured exception frames).
fn prefixes_allowed(bytes: &[u8]) -> bool {
    let mut at = 0;
    while let Some(0x66 | 0xf2 | 0xf3) = bytes.get(at) {
        at += 1;
    }
    match bytes.get(at..) {
        Some([0x64, 0xa1, ..]) => !crate::arch::LONG_MODE && at == 0,
        Some([0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x67 | 0xf0, ..]) => false,
        _ => true,
    }
}

/// Whether `i` (at the start of `bytes`) is an instruction this decoder takes from a prologue.
fn allowed(i: &Instruction, bytes: &[u8]) -> bool {
    let long = crate::arch::LONG_MODE;
    match i.map {
        Map::Legacy => match i.opcode {
            0x50..=0x5f | 0x90 => true,
            // add/or/and/sub/xor/cmp/test/mov/lea with a ModRM operand, and their immediate forms.
            0x01 | 0x03 | 0x09 | 0x0b | 0x21 | 0x23 | 0x29 | 0x2b | 0x31 | 0x33 | 0x39 | 0x3b | 0x85 | 0x88..=0x8b | 0x8d => true,
            0x81 | 0x83 | 0xc6 | 0xc7 => true,
            // 32-bit code only: inc/dec r32, push imm8/imm32, mov eax, fs:[moffs32], mov r32, imm32.
            0x40..=0x4f | 0x6a | 0x68 | 0xb8..=0xbf => !long,
            0xa1 => !long && bytes.first() == Some(&0x64),
            _ => false,
        },
        // movups/movss/movsd, movaps, the SSE arithmetic seen in prologues; movzx and movsx.
        Map::Escape => match i.opcode {
            0x10 | 0x11 | 0x28 | 0x29 | 0x57 | 0x58 | 0x59 | 0x5c | 0x5e | 0xb6 | 0xb7 | 0xbe | 0xbf => true,
            // jcc rel32 (64-bit code only: none was needed in 32-bit code).
            0x80..=0x8f => long,
            _ => false,
        },
        _ => false,
    }
}

/// Decodes one instruction at the start of `bytes`; `None` if it is not one this decoder accepts.
pub fn instruction(bytes: &[u8]) -> Option<Decoded> {
    if !prefixes_allowed(bytes) {
        return None;
    }
    let i = x64::decode_in(bytes, MODE).filter(|i| allowed(i, bytes))?;
    let length = i.length as usize;
    // A rip-relative operand's displacement, or a jcc's: both relative to the instruction's end.
    let rip_displacement = match (i.memory.filter(|m| m.rip_relative), i.branch) {
        (Some(memory), _) => Some(memory.displacement_offset as usize),
        (None, Some(_)) => Some(length - 4),
        (None, None) => None,
    };
    Some(Decoded { length, rip_displacement })
}

/// Whole instructions covering at least `minimum` bytes from the start of `bytes`, or `None` if
/// any of them is not one this decoder accepts, or a conditional jump among them lands inside the
/// bytes taken (its target would be moved away with them).
pub fn prologue(bytes: &[u8], minimum: usize) -> Option<Vec<Decoded>> {
    let mut decoded = Vec::new();
    let mut starts = Vec::new();
    let mut at = 0;
    while at < minimum {
        let next = instruction(bytes.get(at..)?)?;
        starts.push(at);
        at += next.length;
        decoded.push(next);
    }
    for (start, d) in starts.iter().zip(&decoded) {
        if let Some(target) = jump_target(&bytes[*start..*start + d.length])
            && (0..at as isize).contains(&(*start as isize + target))
        {
            return None;
        }
    }
    Some(decoded)
}

/// Where a conditional jump with a 32-bit displacement goes, from its own start (none for other
/// instructions).
fn jump_target(instruction: &[u8]) -> Option<isize> {
    match instruction {
        [0x0f, 0x80..=0x8f, a, b, c, d] => Some(6 + i32::from_le_bytes([*a, *b, *c, *d]) as isize),
        _ => None,
    }
}

// x86-64 machine code; the 32-bit tests are in `arch/x86.rs`.
#[cfg(all(test, target_arch = "x86_64"))]
mod tests;
