use super::*;

fn length(bytes: &[u8]) -> Option<u8> {
    decode(bytes).map(|i| i.length)
}

#[test]
fn common_lengths() {
    assert_eq!(length(&[0x48, 0x89, 0x5c, 0x24, 0x08]), Some(5), "mov [rsp+8], rbx");
    assert_eq!(length(&[0x48, 0x81, 0xec, 0x08, 0x01, 0x00, 0x00]), Some(7), "sub rsp, imm32");
    assert_eq!(length(&[0x48, 0x8d, 0xac, 0x24, 0x60, 0xf9, 0xff, 0xff]), Some(8), "lea rbp, [rsp+disp32]");
    assert_eq!(length(&[0x0f, 0xb6, 0x81, 0xa0, 0x07, 0x00, 0x00]), Some(7), "movzx eax, byte [rcx+disp32]");
    assert_eq!(length(&[0x80, 0x3d, 1, 2, 3, 4, 0]), Some(7), "cmp byte [rip+x], imm8");
    assert_eq!(length(&[0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8]), Some(10), "mov rax, imm64");
    assert_eq!(length(&[0x66, 0xc7, 0x41, 0x08, 0x34, 0x12]), Some(6), "mov word [rcx+8], imm16");
    assert_eq!(length(&[0xf6, 0x41, 0x08, 0x01]), Some(4), "test byte [rcx+8], 1");
    assert_eq!(length(&[0xf6, 0xd8]), Some(2), "neg al: no immediate");
    assert_eq!(length(&[0xc5, 0xf8, 0x77]), Some(3), "vzeroupper");
    assert_eq!(length(&[0xc5, 0xfa, 0x10, 0x05, 1, 2, 3, 4]), Some(8), "vmovss xmm0, [rip+x]");
    assert_eq!(length(&[0xc4, 0xe3, 0x79, 0x04, 0xc0, 0x00]), Some(6), "vpermilps imm8 (map 3)");
    assert_eq!(length(&[0x66, 0x0f, 0x3a, 0x0f, 0xc1, 0x08]), Some(6), "palignr imm8");
    assert_eq!(length(&[0x66, 0x0f, 0x38, 0x00, 0xc1]), Some(5), "pshufb");
    assert_eq!(length(&[0x0f, 0x1f, 0x44, 0x00, 0x00]), Some(5), "nop dword [rax+rax]");
    assert_eq!(length(&[0xe8, 0, 0, 0, 0]), Some(5));
    assert_eq!(length(&[0x0f, 0x85, 0, 0, 0, 0]), Some(6), "jne rel32");
    assert_eq!(length(&[0x66, 0x90]), Some(2), "xchg ax, ax");
    assert_eq!(length(&[0x06]), None, "push es: invalid in 64-bit mode");
}

#[test]
fn operands() {
    let lea = decode(&[0x48, 0x8d, 0x15, 0x10, 0x00, 0x00, 0x00]).unwrap();
    assert!(lea.is_lea());
    assert_eq!(lea.reg, Some(2));
    assert_eq!(lea.rip_target(0x1000), Some(0x1000 + 7 + 0x10));

    let add = decode(&[0x48, 0x81, 0xc7, 0x30, 0x0e, 0x00, 0x00]).unwrap();
    assert_eq!((add.reg, add.rm_register, add.immediate), (Some(0), Some(7), Some(0xe30)), "add rdi, 0xE30");

    let cmp = decode(&[0x80, 0x3d, 0x10, 0, 0, 0, 0x05]).unwrap();
    assert_eq!(cmp.rip_target(0), Some(7 + 0x10), "target counts the trailing immediate");

    let load = decode(&[0x4c, 0x8b, 0x81, 0x08, 0x0e, 0x00, 0x00]).unwrap();
    assert_eq!((load.reg, load.memory.unwrap().base, load.memory.unwrap().displacement), (Some(8), Some(1), 0xe08), "mov r8, [rcx+0xE08]");

    let call = decode(&[0xe8, 0xfb, 0xff, 0xff, 0xff]).unwrap();
    assert!(call.is_call());
    assert_eq!(call.branch_target(0x100), Some(0x100));
    assert!(decode(&[0xff, 0x50, 0x58]).unwrap().is_indirect_call(), "call [rax+0x58]");
    assert_eq!(decode(&[0x41, 0x5f]).unwrap().opcode_register, Some(15), "pop r15");
}

#[test]
fn thirty_two_bit_code_has_no_rex_and_absolute_disp32() {
    // inc eax: a REX prefix in 64-bit code.
    assert_eq!(decode_in(&[0x40], Mode::Legacy32).map(|i| (i.length, i.opcode_register)), Some((1, Some(0))));
    assert_eq!(decode(&[0x40, 0x90]).map(|i| i.length), Some(2));
    // mov ecx, [disp32]: an absolute address in 32-bit code, rip-relative in 64-bit code.
    let absolute = decode_in(&[0x8b, 0x0d, 1, 2, 3, 4], Mode::Legacy32).unwrap().memory.unwrap();
    assert!(!absolute.rip_relative && absolute.displacement_offset == 2);
    assert!(decode(&[0x8b, 0x0d, 1, 2, 3, 4]).unwrap().memory.unwrap().rip_relative);
    // mov eax, fs:[moffs32]: four address bytes.
    assert_eq!(decode_in(&[0x64, 0xa1, 0, 0, 0, 0], Mode::Legacy32).map(|i| i.length), Some(6));
    // 16-bit addressing and VEX are refused in 32-bit code.
    assert_eq!(decode_in(&[0x67, 0x8b, 0x07], Mode::Legacy32), None);
    assert_eq!(decode_in(&[0xc5, 0xf8, 0x77], Mode::Legacy32), None);
}
