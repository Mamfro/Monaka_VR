use super::*;
use monaka_channel::d3d::{create_device, create_texture, create_texture_with, read_texels, simple_texture_desc};
use monaka_channel::debug_image::ImageReader;

#[test]
fn an_srgb_target_is_shared_as_its_stored_values() {
    let name = ChannelName::new(format!(r"Local\monaka-test-share-target-{}", std::process::id())).unwrap();
    let (device, context, _) = create_device(None).unwrap();
    // The game's target: sRGB, holding 128 (about 0.5 stored; about 0.22 if read as sRGB).
    let mut desc = simple_texture_desc(16, 8, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let target = create_texture_with(&device, &desc, &[128, 0, 255, 255].repeat(16 * 8)).unwrap();
    let mut made = make(&device, &name, &desc).unwrap();
    // SAFETY: our own resources on our own immediate context.
    unsafe {
        context.CopyResource(&made.copy, &target);
        crate::debug_depth::dispatch(&context, &made.shader, &made.read, &made.write, (16, 8));
    }
    assert!(made.writer.write(&context, &made.floats));
    // SAFETY: flushes our own context.
    unsafe { context.Flush() };
    let mut reader = ImageReader::open(&name.debug_target(), &device).unwrap();
    assert_eq!(reader.format, DXGI_FORMAT_R16G16B16A16_FLOAT);
    let into = create_texture(&device, &simple_texture_desc(16, 8, DXGI_FORMAT_R16G16B16A16_FLOAT)).unwrap();
    assert!(reader.read(&context, &into));
    let (_, _, _, texels) = read_texels(&device, &context, &into).unwrap();
    let red = monaka_core::half::to_f32(u16::from_le_bytes([texels[0], texels[1]]));
    assert!((red - 128.0 / 255.0).abs() < 0.01, "{red}");
}

#[test]
fn a_plain_target_is_not_a_back_buffer() {
    let (device, _, _) = create_device(None).unwrap();
    let mut desc = simple_texture_desc(16, 8, DXGI_FORMAT_R8G8B8A8_UNORM);
    desc.BindFlags = D3D11_BIND_RENDER_TARGET.0 as u32;
    assert!(!back_buffer(&create_texture(&device, &desc).unwrap()));
}

#[test]
fn a_swapchain_s_buffer_is_a_back_buffer() {
    use windows::Win32::Graphics::Dxgi::*;
    let (device, _, _) = create_device(None).unwrap();
    // SAFETY: plain calls on our own device and factory; a swapchain for no window.
    let buffer = unsafe {
        let factory: IDXGIFactory2 = device.cast::<IDXGIDevice>().unwrap().GetAdapter().unwrap().GetParent().unwrap();
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 16,
            Height: 8,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            ..Default::default()
        };
        let chain = factory.CreateSwapChainForComposition(&device, &desc, None).unwrap();
        chain.GetBuffer::<ID3D11Texture2D>(0).unwrap()
    };
    assert!(back_buffer(&buffer));
}

#[test]
fn formats_read_as_plain_values() {
    assert_eq!(formats(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), Some((DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM)));
    assert_eq!(formats(DXGI_FORMAT_R10G10B10A2_UNORM).map(|f| f.1), Some(DXGI_FORMAT_R10G10B10A2_UNORM));
    assert!(formats(DXGI_FORMAT_D24_UNORM_S8_UINT).is_none());
}
