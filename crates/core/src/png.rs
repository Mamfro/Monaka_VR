//! PNG files without a codec: the image goes into the zlib stream as stored (uncompressed)
//! deflate blocks, which every PNG reader takes. For the harness and probe frames: larger than
//! compressed PNGs, no slower to make than a BMP, and viewable everywhere.

/// CRC-32 (IEEE, as PNG chunks use), from scratch.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Adler-32, the zlib stream's check.
fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5000) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream of stored deflate blocks (at most 65535 bytes each) holding `raw`.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 11);
    out.extend_from_slice(&[0x78, 0x01]);
    let mut blocks = raw.chunks(65535).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        out.push(u8::from(last));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// An 8-bit PNG of `width` x `height` from `bytes_per_pixel` channels per pixel (3 RGB, 4 RGBA),
/// `rows` tightly packed top-down. Missing bytes are written as zero and extra ones dropped.
fn encode(width: u32, height: u32, bytes_per_pixel: usize, rows: &[u8]) -> Vec<u8> {
    let stride = width as usize * bytes_per_pixel;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0); // filter: none
        let start = y * stride;
        let end = start + stride;
        let have = rows.get(start.min(rows.len())..end.min(rows.len())).unwrap_or(&[]);
        raw.extend_from_slice(have);
        raw.resize((y + 1) * (stride + 1), 0);
    }
    let mut out = Vec::with_capacity(raw.len() + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, if bytes_per_pixel == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// An RGB PNG from tightly packed top-down RGB rows.
pub fn rgb(width: u32, height: u32, rows: &[u8]) -> Vec<u8> {
    encode(width, height, 3, rows)
}

/// An RGBA PNG from tightly packed top-down RGBA rows (straight alpha).
pub fn rgba(width: u32, height: u32, rows: &[u8]) -> Vec<u8> {
    encode(width, height, 4, rows)
}

#[cfg(test)]
mod tests;
