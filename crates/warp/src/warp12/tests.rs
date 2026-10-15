use super::*;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;

#[test]
fn shaders_compile_and_the_warp_initialises() {
    for entry in [s!("Clear"), s!("Splat"), s!("Resolve")] {
        compute::compile(&source(), "warp12.hlsl", entry).unwrap();
    }
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: creates a device on the default adapter.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    let warp = DepthWarp::new(&device.unwrap()).unwrap();
    assert!(warp.eye(0).is_none() && warp.ui().is_none());
}
