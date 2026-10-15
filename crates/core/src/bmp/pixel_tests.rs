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
