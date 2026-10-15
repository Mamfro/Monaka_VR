use super::*;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;

#[test]
fn the_shader_compiles_and_the_lay_over_initialises() {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: creates a device on the default adapter.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    make(&device.unwrap()).unwrap();
    assert_eq!(size_of::<Constants>(), 128, "the shader's constant buffer");
    let params = Params::default().leaving_out(&[[1.0; 4]; 7]);
    assert_eq!(params.piece_count, 5, "at most five pieces");
}
