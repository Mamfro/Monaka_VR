use super::*;

#[test]
fn byte_buffers_are_bounded_and_bad_destinations_fail() {
    let mut target = [9u8;8];
    let address = target.as_mut_ptr() as usize;
    assert!(write_bytes(address+2,&[1,2,3]));
    assert_eq!(read::<[u8;8]>(address),Some([9,9,1,2,3,9,9,9]));
    assert!(!write_bytes(address,&[]));
    assert!(!write_bytes(0,&[1]));
    assert!(!write_bytes(8,&[1,2,3]));
    std::hint::black_box(&mut target);
}

#[test]
fn reads_and_writes_survive_bad_addresses() {
    let mut target = [1.0f32, 2.0, 3.0];
    let address = target.as_mut_ptr() as usize;
    assert_eq!(read::<[f32; 3]>(address), Some([1.0, 2.0, 3.0]));
    assert!(write(address + 4, 5.0f32));
    assert_eq!(read::<f32>(address + 4), Some(5.0));
    assert_eq!(read::<u64>(0), None);
    assert_eq!(read::<u64>(8), None, "unmapped low memory fails instead of faulting");
    assert!(!write(8, 1u32));
    std::hint::black_box(&mut target);
}
