use super::*;

#[test]
fn drop_files_holds_the_header_then_the_wide_path_twice_ended() {
    let data = drop_files(Path::new(r"C:\a.jpg"));
    assert_eq!(&data[..4], &DROPFILES_SIZE.to_le_bytes(), "names start after the header");
    assert_eq!(&data[16..20], &1u32.to_le_bytes(), "the names are UTF-16");
    let names: Vec<u16> = data[20..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let expected: Vec<u16> = r"C:\a.jpg".encode_utf16().chain([0, 0]).collect();
    assert_eq!(names, expected);
}
