use super::*;

#[test]
fn checksums_match_the_standards() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    assert_eq!(crc32(b"IEND"), 0xae42_6082, "the IEND chunk's well-known CRC");
}

#[test]
fn files_have_the_right_shape() {
    let file = rgb(2, 1, &[1, 2, 3, 4, 5, 6]);
    assert_eq!(&file[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    assert_eq!(&file[12..16], b"IHDR");
    assert_eq!(&file[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
    assert_eq!(&file[24..29], &[8, 2, 0, 0, 0], "8-bit RGB");
    let idat = 8 + 25;
    assert_eq!(&file[idat + 4..idat + 8], b"IDAT");
    let data_len = u32::from_be_bytes(file[idat..idat + 4].try_into().unwrap()) as usize;
    let data = &file[idat + 8..idat + 8 + data_len];
    // zlib header, one final stored block of 7 bytes (filter byte + 6), the raw bytes, adler.
    assert_eq!(&data[..7], &[0x78, 0x01, 1, 7, 0, 0xf8, 0xff]);
    assert_eq!(&data[7..14], &[0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(data.len(), 7 + 7 + 4);
    assert_eq!(&file[file.len() - 8..file.len() - 4], b"IEND");
    assert_eq!(rgba(1, 1, &[9, 9, 9, 9])[24..29], [8, 6, 0, 0, 0], "8-bit RGBA");
    // Short input is zero-filled; a big image spans several stored blocks.
    assert_eq!(rgb(2, 2, &[1, 2, 3]).len(), rgb(2, 2, &[0; 12]).len());
    let big = rgba(300, 300, &vec![7; 300 * 300 * 4]);
    // 300 rows of a filter byte and 1200 bytes: 360300 bytes, six stored blocks of at most 65535.
    let data_len = u32::from_be_bytes(big[idat..idat + 4].try_into().unwrap()) as usize;
    assert_eq!(data_len, 2 + 6 * 5 + 360_300 + 4);
    let data = &big[idat + 8..];
    assert_eq!(&data[2..7], &[0, 0xff, 0xff, 0, 0], "a full block, not the last");
}
