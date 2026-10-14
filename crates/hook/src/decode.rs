//! A deliberately small x86-64 (and 32-bit x86) decoder for function prologues, for inline hooks on code whose
//! bytes are not known in advance (a system DLL that differs between Windows builds). It knows the
//! instructions compilers put at function starts (pushes, stack adjustments, register moves, stores
//! of saved registers, SSE spills, sign and zero extensions) and refuses everything else, so a
//! prologue it cannot fully account for is never patched. The one branch it takes is a conditional
//! jump with a 32-bit displacement (an early-out test at a function's start): relocated like any
//! rip-relative operand, and only when it lands past the bytes taken ([`prologue`]); a call, a short
//! jump or a jump back into the taken bytes is refused.

/// One decoded instruction: its length and, if it addresses memory relative to the instruction
/// pointer, where its 32-bit displacement sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub length: usize,
    pub rip_displacement: Option<usize>,
}

/// Length of a ModRM operand (ModRM byte, SIB, displacement) and the offset of a rip-relative
/// displacement within it.
fn modrm(bytes: &[u8]) -> Option<(usize, Option<usize>)> {
    let m = *bytes.first()?;
    let (mode, rm) = (m >> 6, m & 7);
    let mut length = 1;
    if mode != 3 && rm == 4 {
        let sib = *bytes.get(1)?;
        length += 1;
        if mode == 0 && sib & 7 == 5 {
            return Some((length + 4, None));
        }
    }
    match mode {
        // [disp32]: rip-relative in 64-bit code, an absolute address in 32-bit code.
        0 if rm == 5 => Some((length + 4, crate::arch::LONG_MODE.then_some(1))),
        0 | 3 => Some((length, None)),
        1 => Some((length + 1, None)),
        _ => Some((length + 4, None)),
    }
}

/// Decodes one instruction at the start of `bytes`; `None` if it is not one this decoder accepts.
pub fn instruction(bytes: &[u8]) -> Option<Decoded> {
    let mut at = 0;
    // Legacy prefixes used by SSE moves and 16-bit operands, then an optional REX (64-bit code).
    while matches!(bytes.get(at)?, 0x66 | 0xf2 | 0xf3) {
        at += 1;
    }
    if crate::arch::LONG_MODE && (0x40..=0x4f).contains(bytes.get(at)?) {
        at += 1;
    }
    let opcode = *bytes.get(at)?;
    at += 1;
    if !crate::arch::LONG_MODE
        && let Some(decoded) = x86_only(bytes, at, opcode)
    {
        return (decoded.length <= bytes.len()).then_some(decoded);
    }
    let with_modrm = |at: usize, immediate: usize| -> Option<Decoded> {
        let (length, rip) = modrm(bytes.get(at..)?)?;
        Some(Decoded { length: at + length + immediate, rip_displacement: rip.map(|r| at + r) })
    };
    let decoded = match opcode {
        0x50..=0x57 | 0x58..=0x5f | 0x90 => Decoded { length: at, rip_displacement: None },
        // add/or/and/sub/xor/cmp/test/mov/lea with a ModRM operand.
        0x01 | 0x03 | 0x09 | 0x0b | 0x21 | 0x23 | 0x29 | 0x2b | 0x31 | 0x33 | 0x39 | 0x3b | 0x85 | 0x88..=0x8b | 0x8d => with_modrm(at, 0)?,
        0x83 | 0xc6 => with_modrm(at, 1)?,
        0x81 | 0xc7 => with_modrm(at, 4)?,
        0x0f => {
            let second = *bytes.get(at)?;
            match second {
                // movups/movss/movsd, movaps, and the SSE arithmetic seen in prologues; movzx and
                // movsx.
                0x10 | 0x11 | 0x28 | 0x29 | 0x57 | 0x58 | 0x59 | 0x5c | 0x5e | 0xb6 | 0xb7 | 0xbe | 0xbf => with_modrm(at + 1, 0)?,
                // jcc rel32: its displacement is relative to the instruction's end, as a
                // rip-relative operand's is (64-bit code only: 32-bit code's jumps are relocated
                // the same way, but none was needed there).
                0x80..=0x8f if crate::arch::LONG_MODE => Decoded { length: at + 5, rip_displacement: Some(at + 1) },
                _ => return None,
            }
        }
        _ => return None,
    };
    (decoded.length <= bytes.len() && decoded.length <= 15).then_some(decoded)
}

/// The prologue instructions only 32-bit code has (`opcode` read, `at` past it): `inc`/`dec r32`,
/// `push imm8`, `push imm32` (an address, absolute), `mov eax, fs:[0]` (structured exception
/// frames) and `mov r32, imm32`.
fn x86_only(bytes: &[u8], at: usize, opcode: u8) -> Option<Decoded> {
    let length = match opcode {
        0x40..=0x4f => at,
        0x6a => at + 1,
        0x68 | 0xb8..=0xbf => at + 4,
        // fs: then mov eax, [moffs32].
        0x64 if bytes.get(at) == Some(&0xa1) => at + 5,
        _ => return None,
    };
    Some(Decoded { length, rip_displacement: None })
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
mod tests {
    use super::*;

    fn lengths(bytes: &[u8]) -> Option<Vec<usize>> {
        prologue(bytes, 5).map(|d| d.iter().map(|i| i.length).collect())
    }

    #[test]
    fn d3d11_draw_prologues() {
        // d3d11.dll 10.0.26100.9549: DrawIndexed, DrawIndexedInstanced, DrawInstanced.
        assert_eq!(lengths(&[0x48, 0x83, 0xec, 0x38, 0x48, 0x81, 0xc1, 0x28, 0xff, 0xff, 0xff, 0xe8]), Some(vec![4, 7]));
        assert_eq!(lengths(&[0x40, 0x53, 0x48, 0x83, 0xec, 0x40, 0x41, 0x8b, 0xd8]), Some(vec![2, 4]));
        assert_eq!(lengths(&[0x48, 0x83, 0xec, 0x38, 0x44, 0x8b, 0xda, 0x4c, 0x8d]), Some(vec![4, 3]));
    }

    #[test]
    fn engine_prologues_and_relocation() {
        // mov [rsp+8], rbx (SIB + disp8).
        assert_eq!(lengths(&[0x48, 0x89, 0x5c, 0x24, 0x08, 0xcc]), Some(vec![5]));
        // mov rcx, [rcx+8]; mulss xmm1, [rip+0x79c760].
        let decoded = prologue(&[0x48, 0x8b, 0x49, 0x08, 0xf3, 0x0f, 0x59, 0x0d, 0x60, 0xc7, 0x79, 0x00], 5).unwrap();
        assert_eq!(decoded, vec![Decoded { length: 4, rip_displacement: None }, Decoded { length: 8, rip_displacement: Some(4) }]);
        // lea r10, [rcx-0xd8]: disp32 without SIB.
        assert_eq!(instruction(&[0x4c, 0x8d, 0x91, 0x28, 0xff, 0xff, 0xff]).map(|d| d.length), Some(7));
    }

    #[test]
    fn d3d11_map_and_bind_prologues() {
        // d3d11.dll 10.0.26100: Map (sub; lea; movsx rax, byte [rdx+0xc9]) and VSSetConstantBuffers
        // (test r8d, r8d; je +0x179; mov rax, rsp; mov [rax+0x10], rbx).
        let map = [0x48, 0x83, 0xec, 0x48, 0x4c, 0x8d, 0x91, 0x28, 0xff, 0xff, 0xff, 0x48, 0x0f, 0xbe, 0x82, 0xc9, 0, 0, 0, 0x48];
        assert_eq!(prologue(&map, 14).map(|d| d.iter().map(|i| i.length).collect::<Vec<_>>()), Some(vec![4, 7, 8]));
        let bind = [0x45, 0x85, 0xc0, 0x0f, 0x84, 0x79, 0x01, 0, 0, 0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10, 0x4c];
        let decoded = prologue(&bind, 14).unwrap();
        assert_eq!(decoded.iter().map(|i| i.length).collect::<Vec<_>>(), vec![3, 6, 3, 4]);
        assert_eq!(decoded[1].rip_displacement, Some(2), "the jump's displacement is relocated");
        // A jump back into the bytes taken cannot be moved.
        let inside = [0x45, 0x85, 0xc0, 0x0f, 0x84, 0xfa, 0xff, 0xff, 0xff, 0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10];
        assert_eq!(prologue(&inside, 14), None);
        assert_eq!(lengths(&[0x74, 0x05, 0x90, 0x90, 0x90, 0x90]), None, "a short jump");
    }

    #[test]
    fn branches_and_unknown_instructions_are_refused() {
        assert_eq!(lengths(&[0xe9, 0, 0, 0, 0]), None, "a jump (already hooked by someone else)");
        assert_eq!(lengths(&[0x48, 0x83, 0xec, 0x28, 0xe8, 0, 0, 0, 0]), None, "a call inside the stolen bytes");
        assert_eq!(lengths(&[0xff, 0x25, 0, 0, 0, 0]), None);
        assert_eq!(lengths(&[0x48, 0x83]), None, "cut short");
    }
}
