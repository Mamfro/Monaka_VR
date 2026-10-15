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

#[test]
fn c_strings_read_up_to_an_unreadable_page_and_across_chunks() {
    use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect};
    // SAFETY: two fresh pages of our own; the second made unreadable, both freed at the end.
    let pages = unsafe { VirtualAlloc(None, 8192, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
    assert_ne!(pages, 0);
    let mut old = Default::default();
    // SAFETY: our own page.
    unsafe { VirtualProtect((pages + 4096) as *const _, 4096, PAGE_NOACCESS, &mut old) }.unwrap();
    // A string whose NUL is the first page's last byte.
    let end = pages + 4096;
    assert!(write_bytes(end - 6, b"hello\0"));
    assert_eq!(read_c_string(end - 6, 64).as_deref(), Some("hello"));
    // One without its NUL before the unreadable page fails instead of faulting.
    assert!(write_bytes(end - 3, b"abc"));
    assert_eq!(read_c_string(end - 3, 64), None);
    // A long string crosses chunks; `max` bounds it.
    let long = vec![b'x'; 700];
    assert!(write_bytes(pages, &long));
    assert!(write(pages + 700, 0u8));
    assert_eq!(read_c_string(pages, 1000).map(|s| s.len()), Some(700));
    assert_eq!(read_c_string(pages, 700), None, "no NUL within max");
    // SAFETY: frees the pages allocated above.
    unsafe { VirtualFree(pages as *mut _, 0, MEM_RELEASE) }.unwrap();
}
