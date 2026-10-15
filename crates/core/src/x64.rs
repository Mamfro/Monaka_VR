//! An x86-64 instruction length decoder for reading a game's code: where each instruction ends, its
//! ModRM operand, rip-relative or register-relative displacement, immediate and branch target. It
//! does not name instructions beyond what scanning needs; anything it cannot account for is `None`
//! rather than a guess. Covers the legacy one-byte and 0F maps, 0F38/0F3A, VEX and EVEX.
//!
//! 32-bit code ([`Mode::Legacy32`], [`decode_in`]) shares the tables: no REX (0x40-0x4F are
//! `inc`/`dec`), `[disp32]` is an absolute address, moffs are 4 bytes; VEX/EVEX and 16-bit
//! addressing are refused there.

/// The code being decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 64-bit code.
    Long,
    /// 32-bit x86 code.
    Legacy32,
}

/// Which opcode table an instruction comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Map {
    /// One-byte opcodes.
    Legacy,
    /// `0F xx`.
    Escape,
    /// `0F 38 xx`.
    Escape38,
    /// `0F 3A xx`.
    Escape3A,
    /// VEX or EVEX encoded, with the map number they carry (1 = 0F, 2 = 0F38, 3 = 0F3A, ...).
    Vector(u8),
}

/// A memory operand: `[base + index*scale + displacement]` or `[rip + displacement]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    /// Base register number (0-15, REX.B applied), `None` for rip-relative or no base.
    pub base: Option<u8>,
    pub rip_relative: bool,
    pub displacement: i32,
    /// Offset of the displacement within the instruction (4 bytes for rip-relative).
    pub displacement_offset: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub length: u8,
    pub map: Map,
    pub opcode: u8,
    /// REX.W (or VEX/EVEX W).
    pub wide: bool,
    /// ModRM `reg` field with REX.R applied (the register operand, or the opcode extension).
    pub reg: Option<u8>,
    /// ModRM `rm` register when the operand is a register (mod = 3), REX.B applied.
    pub rm_register: Option<u8>,
    pub memory: Option<Memory>,
    pub immediate: Option<i64>,
    /// Relative branch displacement (calls, jumps, conditional jumps, loops).
    pub branch: Option<i32>,
    /// Register in the low opcode bits (push/pop/mov imm/xchg/bswap), REX.B applied.
    pub opcode_register: Option<u8>,
}

impl Instruction {
    /// The address a rip-relative operand refers to, for this instruction at `address`.
    pub fn rip_target(&self, address: u64) -> Option<u64> {
        let memory = self.memory.filter(|m| m.rip_relative)?;
        Some(address.wrapping_add(self.length as u64).wrapping_add_signed(memory.displacement as i64))
    }

    /// Where a relative branch at `address` goes.
    pub fn branch_target(&self, address: u64) -> Option<u64> {
        Some(address.wrapping_add(self.length as u64).wrapping_add_signed(self.branch? as i64))
    }

    /// `call rel32`.
    pub fn is_call(&self) -> bool {
        self.map == Map::Legacy && self.opcode == 0xe8
    }

    /// An indirect call: `call r/m64` (`FF /2`).
    pub fn is_indirect_call(&self) -> bool {
        self.map == Map::Legacy && self.opcode == 0xff && self.reg.map(|r| r & 7) == Some(2)
    }

    /// `ret`, `ret imm16`, an indirect `jmp`, `int3` or `ud2`: control does not fall through.
    pub fn ends_flow(&self) -> bool {
        match self.map {
            Map::Legacy => matches!(self.opcode, 0xc3 | 0xc2 | 0xcc | 0xe9 | 0xeb) || (self.opcode == 0xff && self.reg.map(|r| r & 7) == Some(4)),
            Map::Escape => self.opcode == 0x0b,
            _ => false,
        }
    }

    /// `lea r64, [rip+disp]`.
    pub fn is_lea(&self) -> bool {
        self.map == Map::Legacy && self.opcode == 0x8d
    }

    /// `mov r, r/m` (`8B`): a load into `reg`.
    pub fn is_load(&self) -> bool {
        self.map == Map::Legacy && self.opcode == 0x8b
    }

    /// `mov r/m, r` (`89`): a store from `reg`.
    pub fn is_store(&self) -> bool {
        self.map == Map::Legacy && self.opcode == 0x89
    }
}

/// Immediate operand sizes.
#[derive(Clone, Copy)]
enum Imm {
    None,
    B1,
    B2,
    /// 2 with an operand-size prefix, else 4.
    Z,
    /// `mov r64, imm64` with REX.W, else Z.
    V,
    /// `enter imm16, imm8`.
    Enter,
    /// moffs: 8 bytes (4 with an address-size prefix).
    Moffs,
}

#[derive(Clone, Copy)]
struct Form {
    modrm: bool,
    imm: Imm,
    /// The immediate is a relative branch.
    relative: bool,
    /// Register encoded in the opcode's low 3 bits.
    register: bool,
}

const fn form(modrm: bool, imm: Imm) -> Option<Form> {
    Some(Form { modrm, imm, relative: false, register: false })
}

const fn branch(imm: Imm) -> Option<Form> {
    Some(Form { modrm: false, imm, relative: true, register: false })
}

const fn register(imm: Imm) -> Option<Form> {
    Some(Form { modrm: false, imm, relative: false, register: true })
}

/// The one-byte map; prefixes are handled before this. `None` is invalid in 64-bit mode.
fn legacy(opcode: u8, modrm_reg: impl Fn() -> Option<u8>) -> Option<Form> {
    match opcode {
        0x00..=0x3f => match opcode & 7 {
            0..=3 => form(true, Imm::None),
            4 => form(false, Imm::B1),
            5 => form(false, Imm::Z),
            // push/pop segment, daa/das/aaa/aas and the segment prefixes (taken earlier): invalid.
            _ => None,
        },
        0x50..=0x5f => register(Imm::None),
        0x63 => form(true, Imm::None),
        0x68 => form(false, Imm::Z),
        0x69 => form(true, Imm::Z),
        0x6a => form(false, Imm::B1),
        0x6b => form(true, Imm::B1),
        0x6c..=0x6f => form(false, Imm::None),
        0x70..=0x7f => branch(Imm::B1),
        0x80 | 0x83 => form(true, Imm::B1),
        0x81 => form(true, Imm::Z),
        0x84..=0x8f => form(true, Imm::None),
        0x90..=0x97 => register(Imm::None),
        0x98 | 0x99 | 0x9b..=0x9f => form(false, Imm::None),
        0xa0..=0xa3 => form(false, Imm::Moffs),
        0xa4..=0xa7 | 0xaa..=0xaf => form(false, Imm::None),
        0xa8 => form(false, Imm::B1),
        0xa9 => form(false, Imm::Z),
        0xb0..=0xb7 => register(Imm::B1),
        0xb8..=0xbf => register(Imm::V),
        0xc0 | 0xc1 => form(true, Imm::B1),
        0xc2 | 0xca => form(false, Imm::B2),
        0xc3 | 0xc9 | 0xcb | 0xcc | 0xcf => form(false, Imm::None),
        0xc6 => form(true, Imm::B1),
        // C7 /0 mov imm32; C7 F8 xbegin rel32 has the same length.
        0xc7 => form(true, Imm::Z),
        0xc8 => form(false, Imm::Enter),
        0xcd => form(false, Imm::B1),
        0xd0..=0xd3 => form(true, Imm::None),
        0xd7 => form(false, Imm::None),
        0xd8..=0xdf => form(true, Imm::None),
        0xe0..=0xe3 | 0xeb => branch(Imm::B1),
        0xe4..=0xe7 => form(false, Imm::B1),
        0xe8 | 0xe9 => branch(Imm::Z),
        0xec..=0xef | 0xf1 | 0xf4 | 0xf5 | 0xf8..=0xfd => form(false, Imm::None),
        // test r/m, imm only for /0 and /1.
        0xf6 => form(true, if modrm_reg()? & 6 == 0 { Imm::B1 } else { Imm::None }),
        0xf7 => form(true, if modrm_reg()? & 6 == 0 { Imm::Z } else { Imm::None }),
        0xfe | 0xff => form(true, Imm::None),
        _ => None,
    }
}

/// The 0F map.
fn escape(opcode: u8) -> Option<Form> {
    match opcode {
        0x05..=0x09 | 0x0b | 0x0e | 0x30..=0x35 | 0x37 | 0x77 | 0xa0..=0xa2 | 0xa8..=0xaa => form(false, Imm::None),
        0x80..=0x8f => branch(Imm::Z),
        0xc8..=0xcf => register(Imm::None),
        0x70..=0x73 | 0xa4 | 0xac | 0xba | 0xc2 | 0xc4..=0xc6 => form(true, Imm::B1),
        // 3DNow!: ModRM then an opcode byte in the immediate's place.
        0x0f => form(true, Imm::B1),
        0x04 | 0x0a | 0x0c | 0x24..=0x27 | 0x36 | 0x39..=0x3f | 0x7a | 0x7b | 0xa6 | 0xa7 | 0xff => None,
        _ => form(true, Imm::None),
    }
}

/// The ModRM operand at `at` (32-bit addressing has the same ModRM/SIB shape in either mode;
/// `[disp32]` is rip-relative in 64-bit code only).
fn modrm_operand(bytes: &[u8], at: usize, rex_b: u8, code: Mode) -> Option<(usize, Option<Memory>, Option<u8>)> {
    let m = *bytes.get(at)?;
    let (mode, rm) = (m >> 6, m & 7);
    if mode == 3 {
        return Some((1, None, Some(rm | rex_b)));
    }
    let mut length = 1;
    let mut base = Some(rm | rex_b);
    if rm == 4 {
        let sib = *bytes.get(at + 1)?;
        length += 1;
        base = Some((sib & 7) | rex_b);
        if mode == 0 && sib & 7 == 5 {
            let displacement = i32::from_le_bytes(bytes.get(at + 2..at + 6)?.try_into().ok()?);
            return Some((length + 4, Some(Memory { base: None, rip_relative: false, displacement, displacement_offset: 0 }), None));
        }
    }
    let disp_at = at + length;
    match mode {
        0 if rm == 5 => {
            let displacement = i32::from_le_bytes(bytes.get(disp_at..disp_at + 4)?.try_into().ok()?);
            Some((length + 4, Some(Memory { base: None, rip_relative: code == Mode::Long, displacement, displacement_offset: disp_at as u8 }), None))
        }
        0 => Some((length, Some(Memory { base, rip_relative: false, displacement: 0, displacement_offset: 0 }), None)),
        1 => {
            let displacement = *bytes.get(disp_at)? as i8 as i32;
            Some((length + 1, Some(Memory { base, rip_relative: false, displacement, displacement_offset: disp_at as u8 }), None))
        }
        _ => {
            let displacement = i32::from_le_bytes(bytes.get(disp_at..disp_at + 4)?.try_into().ok()?);
            Some((length + 4, Some(Memory { base, rip_relative: false, displacement, displacement_offset: disp_at as u8 }), None))
        }
    }
}

fn read_imm(bytes: &[u8], at: usize, size: usize) -> Option<i64> {
    let b = bytes.get(at..at + size)?;
    Some(match size {
        1 => b[0] as i8 as i64,
        2 => i16::from_le_bytes(b.try_into().ok()?) as i64,
        3 => (u16::from_le_bytes([b[0], b[1]]) as i64) | ((b[2] as i64) << 16),
        4 => i32::from_le_bytes(b.try_into().ok()?) as i64,
        8 => i64::from_le_bytes(b.try_into().ok()?),
        _ => return None,
    })
}

/// Decodes the 64-bit instruction at the start of `bytes`.
pub fn decode(bytes: &[u8]) -> Option<Instruction> {
    decode_in(bytes, Mode::Long)
}

/// Decodes the instruction at the start of `bytes`, in `mode`'s code.
pub fn decode_in(bytes: &[u8], mode: Mode) -> Option<Instruction> {
    let long = mode == Mode::Long;
    let mut at = 0;
    let mut operand16 = false;
    // 32-bit addresses: an address-size prefix in 64-bit code, the default in 32-bit code.
    let mut address32 = !long;
    while let Some(&b) = bytes.get(at) {
        match b {
            0x66 => operand16 = true,
            0x67 if long => address32 = true,
            0x67 => return None,
            0xf0 | 0xf2 | 0xf3 | 0x2e | 0x36 | 0x3e | 0x26 | 0x64 | 0x65 => {}
            _ => break,
        }
        at += 1;
        if at > 14 {
            return None;
        }
    }
    let mut rex = 0u8;
    if long
        && let Some(&b) = bytes.get(at)
        && (0x40..=0x4f).contains(&b)
    {
        rex = b;
        at += 1;
    }
    let (rex_w, rex_r, rex_b) = (rex & 8 != 0, (rex & 4) << 1, (rex & 1) << 3);
    let lead = *bytes.get(at)?;

    // VEX (C4/C5) and EVEX (62): always a ModRM operand; an imm8 for map 3 and a few map-1 opcodes.
    if matches!(lead, 0xc4 | 0xc5 | 0x62) {
        if rex != 0 || !long {
            return None;
        }
        let (map, wide, reg_ext, b_ext, prefix_length) = match lead {
            0xc5 => {
                let p = *bytes.get(at + 1)?;
                (1, false, if p & 0x80 == 0 { 8 } else { 0 }, 0, 2)
            }
            0xc4 => {
                let (p1, p2) = (*bytes.get(at + 1)?, *bytes.get(at + 2)?);
                (p1 & 0x1f, p2 & 0x80 != 0, if p1 & 0x80 == 0 { 8 } else { 0 }, if p1 & 0x20 == 0 { 8 } else { 0 }, 3)
            }
            _ => {
                let (p0, p1) = (*bytes.get(at + 1)?, *bytes.get(at + 3 - 1)?);
                (p0 & 7, p1 & 0x80 != 0, if p0 & 0x80 == 0 { 8 } else { 0 }, if p0 & 0x20 == 0 { 8 } else { 0 }, 4)
            }
        };
        if !(1..=7).contains(&map) {
            return None;
        }
        at += prefix_length;
        let opcode = *bytes.get(at)?;
        at += 1;
        // vzeroupper / vzeroall have no ModRM.
        if lead != 0x62 && map == 1 && opcode == 0x77 {
            return Some(Instruction {
                length: at as u8,
                map: Map::Vector(map),
                opcode,
                wide,
                reg: None,
                rm_register: None,
                memory: None,
                immediate: None,
                branch: None,
                opcode_register: None,
            });
        }
        let reg = (*bytes.get(at)? >> 3 & 7) | reg_ext;
        let (operand, memory, rm_register) = modrm_operand(bytes, at, b_ext, mode)?;
        at += operand;
        let imm8 = map == 3 || (map == 1 && matches!(opcode, 0x70..=0x73 | 0xc2 | 0xc4..=0xc6));
        let immediate = if imm8 { Some(read_imm(bytes, at, 1)?) } else { None };
        if imm8 {
            at += 1;
        }
        return (at <= 15).then_some(Instruction {
            length: at as u8,
            map: Map::Vector(map),
            opcode,
            wide,
            reg: Some(reg),
            rm_register,
            memory,
            immediate,
            branch: None,
            opcode_register: None,
        });
    }

    let (map, opcode, shape) = if lead == 0x0f {
        let second = *bytes.get(at + 1)?;
        match second {
            0x38 => {
                at += 3;
                (Map::Escape38, *bytes.get(at - 1)?, form(true, Imm::None)?)
            }
            0x3a => {
                at += 3;
                (Map::Escape3A, *bytes.get(at - 1)?, form(true, Imm::B1)?)
            }
            _ => {
                at += 2;
                (Map::Escape, second, escape(second)?)
            }
        }
    } else {
        at += 1;
        let modrm_at = at;
        // 32-bit code's inc/dec r32 (REX in 64-bit code).
        let shape = if !long && (0x40..=0x4f).contains(&lead) { register(Imm::None) } else { legacy(lead, || bytes.get(modrm_at).map(|m| m >> 3 & 7)) };
        (Map::Legacy, lead, shape?)
    };

    let mut reg = None;
    let mut rm_register = None;
    let mut memory = None;
    if shape.modrm {
        reg = Some((*bytes.get(at)? >> 3 & 7) | rex_r);
        let (operand, mem, rm) = modrm_operand(bytes, at, rex_b, mode)?;
        at += operand;
        memory = mem;
        rm_register = rm;
    }
    let size = match shape.imm {
        Imm::None => 0,
        Imm::B1 => 1,
        Imm::B2 => 2,
        Imm::Z => {
            if operand16 && !shape.relative {
                2
            } else {
                4
            }
        }
        Imm::V => {
            if rex_w {
                8
            } else if operand16 {
                2
            } else {
                4
            }
        }
        Imm::Enter => 3,
        Imm::Moffs => {
            if address32 {
                4
            } else {
                8
            }
        }
    };
    let value = if size > 0 { Some(read_imm(bytes, at, size)?) } else { None };
    at += size;
    if at > 15 {
        return None;
    }
    let (immediate, branch) = if shape.relative { (None, value.map(|v| v as i32)) } else { (value, None) };
    Some(Instruction {
        length: at as u8,
        map,
        opcode,
        wide: rex_w,
        reg,
        rm_register,
        memory,
        immediate,
        branch,
        opcode_register: shape.register.then_some((opcode & 7) | rex_b),
    })
}

#[cfg(test)]
mod tests;
