use super::*;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn i32_at(bytes: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[test]
fn rgb_rows_are_padded_and_bottom_up() {
    // 3 pixels of 3 bytes: 9 bytes, padded to 12 per row.
    let file = rgb(3, 2, |x, y| [x as u8, y as u8, 7]);
    assert_eq!(&file[..2], b"BM");
    assert_eq!(file.len(), HEADER_BYTES + 12 * 2);
    assert_eq!(u32_at(&file, 2) as usize, file.len(), "file size");
    assert_eq!(u32_at(&file, 10), 54, "pixel offset");
    assert_eq!(u32_at(&file, 14), 40, "info header size");
    assert_eq!((i32_at(&file, 18), i32_at(&file, 22)), (3, 2), "bottom-up: positive height");
    assert_eq!(u16::from_le_bytes([file[28], file[29]]), 24);
    assert_eq!(u32_at(&file, 34), 24, "image size");
    // The first stored row is the bottom one (y = 1), in BGR order, then padding.
    assert_eq!(&file[54..66], &[7, 1, 0, 7, 1, 1, 7, 1, 2, 0, 0, 0]);
    assert_eq!(&file[66..69], &[7, 0, 0]);
}

#[test]
fn bgra_is_top_down_and_always_whole() {
    let rows: Vec<u8> = (0..2 * 2 * 4).map(|i| i as u8).collect();
    let file = bgra(2, 2, &rows);
    assert_eq!(file.len(), HEADER_BYTES + 16);
    assert_eq!(u32_at(&file, 2) as usize, file.len());
    assert_eq!((i32_at(&file, 18), i32_at(&file, 22)), (2, -2), "top-down: negative height");
    assert_eq!(u16::from_le_bytes([file[28], file[29]]), 32);
    assert_eq!(&file[54..], &rows[..]);
    assert_eq!(bgra(2, 2, &rows[..5]).len(), file.len(), "short input is zero-filled");
    assert_eq!(bgra(1, 1, &rows).len(), HEADER_BYTES + 4, "long input is cut");
}
