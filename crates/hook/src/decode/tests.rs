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
