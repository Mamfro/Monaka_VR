use super::*;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, ID3D11RenderTargetView};

#[test]
fn eight_byte_texels_come_back_whole() {
    let (device, context, _) = create_device(None).unwrap();
    // 3x2 half-float RGBA: each texel's bytes count up, so a wrong stride or row shows.
    let bytes: Vec<u8> = (0..3 * 2 * 8).map(|i| i as u8).collect();
    let desc = D3D11_TEXTURE2D_DESC { BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..simple_texture_desc(3, 2, DXGI_FORMAT_R16G16B16A16_FLOAT) };
    let texture = create_texture_with(&device, &desc, &bytes).unwrap();
    assert_eq!(read_texels(&device, &context, &texture).unwrap(), (3, 2, 8, bytes));
    assert!(read_image(&device, &context, &texture).is_err(), "not 4-byte texels");
}

#[test]
fn subresource_zero_of_mips_and_layers() {
    let (device, context, _) = create_device(None).unwrap();
    let desc = D3D11_TEXTURE2D_DESC { MipLevels: 2, ArraySize: 2, BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..simple_texture_desc(4, 4, DXGI_FORMAT_R8G8B8A8_UNORM) };
    let texture = create_texture(&device, &desc).unwrap();
    let mut view: Option<ID3D11RenderTargetView> = None;
    // SAFETY: test setup on our own device: a view of the top mip of both layers, cleared.
    unsafe {
        device.CreateRenderTargetView(&texture, None, Some(&mut view)).unwrap();
        context.ClearRenderTargetView(view.as_ref().unwrap(), &[1.0, 0.0, 0.0, 1.0]);
    }
    let (width, height, texel, texels) = read_texels(&device, &context, &texture).unwrap();
    assert_eq!((width, height, texel), (4, 4, 4));
    assert!(texels.chunks_exact(4).all(|t| t == [255, 0, 0, 255]));
    assert_eq!(read_pixel(&device, &context, &texture, 3, 3).unwrap(), [255, 0, 0, 255]);
    assert!(read_pixel(&device, &context, &texture, 4, 0).is_err(), "outside the texture");
}
