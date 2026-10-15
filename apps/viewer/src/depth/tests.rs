use super::*;

#[test]
fn depth_goes_to_the_depth_format_of_its_family() {
    use dxgi_format::*;
    let offered = [R8G8B8A8_UNORM_SRGB as i64, D32_FLOAT as i64, D16_UNORM as i64, D24_UNORM_S8_UINT as i64];
    assert_eq!(swapchain_format(&offered, R32_TYPELESS), Some(D32_FLOAT));
    assert_eq!(swapchain_format(&offered, R16_TYPELESS), Some(D16_UNORM));
    assert_eq!(swapchain_format(&[D24_UNORM_S8_UINT as i64], R32_TYPELESS), None, "no copy across families");
    assert_eq!(swapchain_format(&offered, R8G8B8A8_UNORM), None);
}

#[test]
fn distances_read_as_metres() {
    assert_eq!(metres(0.05), "0.05 m");
    assert_eq!(metres(f32::INFINITY), "infinite");
}
