//! Uncompressed BMP files, for probe dumps and screenshots: every image viewer opens them and
//! nothing needs a codec. Both kinds carry a 54-byte header (file header, then `BITMAPINFOHEADER`).

/// File header plus info header.
pub const HEADER_BYTES: usize = 54;

/// The two headers for `image` bytes of pixels; a negative `height` marks top-down rows.
fn header(width: u32, height: i32, bits: u16, image: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + image as usize);
    out.extend_from_slice(b"BM");
    for v in [HEADER_BYTES as u32 + image, 0, HEADER_BYTES as u32, 40] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    // No compression, the image size, 72 dpi both ways, no palette.
    for v in [0u32, image, 2835, 2835, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// A 24-bit BMP of `width` x `height` from `pixel(x, y)` = `[r, g, b]`, y down from the top. Rows
/// are stored bottom-up, each padded to four bytes.
pub fn rgb(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let stride = (width * 3).div_ceil(4) * 4;
    let mut out = header(width, height as i32, 24, stride * height);
    for y in (0..height).rev() {
        let start = out.len();
        for x in 0..width {
            let [r, g, b] = pixel(x, y);
            out.extend_from_slice(&[b, g, r]);
        }
        out.resize(start + stride as usize, 0);
    }
    out
}

/// A 32-bit top-down BMP from `rows`: `width` x `height` BGRA texels, tightly packed (GDI's DIB
/// layout, which needs no padding). Missing bytes are written as zero and extra ones are dropped,
/// so the file always matches its header.
pub fn bgra(width: u32, height: u32, rows: &[u8]) -> Vec<u8> {
    let image = width * height * 4;
    let mut out = header(width, -(height as i32), 32, image);
    out.extend_from_slice(&rows[..rows.len().min(image as usize)]);
    out.resize(HEADER_BYTES + image as usize, 0);
    out
}

#[cfg(test)]
mod tests {
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
}

/// Normalize one packed texel in a supported stereo DXGI format to RGBA8.
/// This is numeric channel conversion, not HDR tone mapping or gamma conversion.
pub fn dxgi_rgba8(format: u32, pixel: [u8; 4]) -> Option<[u8; 4]> {
    use crate::protocol::dxgi_format::*;
    match format {
        R8G8B8A8_UNORM => Some(pixel),
        B8G8R8A8_UNORM => Some([pixel[2], pixel[1], pixel[0], pixel[3]]),
        R10G10B10A2_UNORM => {
            let value = u32::from_le_bytes(pixel);
            Some([
                ((value & 1023) * 255 / 1023) as u8,
                (((value >> 10) & 1023) * 255 / 1023) as u8,
                (((value >> 20) & 1023) * 255 / 1023) as u8,
                (((value >> 30) & 3) * 85) as u8,
            ])
        }
        _ => None,
    }
}
#[cfg(test)]
mod pixel_tests {
    use super::*;
    #[test]
    fn packed_formats_preserve_channel_order_and_alpha() {
        assert_eq!(
            dxgi_rgba8(28, [255, 128, 64, 200]),
            Some([255, 128, 64, 200])
        );
        assert_eq!(
            dxgi_rgba8(87, [64, 128, 255, 200]),
            Some([255, 128, 64, 200])
        );
        let pixel = 1023u32 | (512 << 10) | (128 << 20) | (3 << 30);
        assert_eq!(
            dxgi_rgba8(24, pixel.to_le_bytes()),
            Some([255, 127, 31, 255])
        );
        assert_eq!(dxgi_rgba8(24, [0; 4]), Some([0; 4]));
        assert_eq!(dxgi_rgba8(999, [0; 4]), None);
    }
}
