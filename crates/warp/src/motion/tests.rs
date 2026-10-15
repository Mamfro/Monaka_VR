use super::*;
use monaka_channel::d3d::{create_device, create_texture, create_texture_with, read_image};
use monaka_core::half;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_FLOAT};

fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bind: D3D11_BIND_FLAG, bytes: Option<&[u8]>) -> ID3D11Texture2D {
    let desc = D3D11_TEXTURE2D_DESC { BindFlags: bind.0 as u32, ..d3d::simple_texture_desc(w, h, format) };
    match bytes {
        Some(bytes) => create_texture_with(device, &desc, bytes),
        None => create_texture(device, &desc),
    }
    .unwrap()
}

fn uav(device: &ID3D11Device, texture: &ID3D11Texture2D) -> ID3D11UnorderedAccessView {
    let mut view = None;
    // SAFETY: test setup on our own texture.
    unsafe { device.CreateUnorderedAccessView(texture, None, Some(&mut view)).unwrap() };
    view.unwrap()
}

#[test]
fn shader_compiles() {
    crate::compile(MOTION_SOURCE, "motion.hlsl", s!("Main")).unwrap();
}

/// The camera steps 0.5 m right between frames, a wall 2 m ahead fills the left half and the
/// sky (reversed infinite depth: 0) the right half. The wall was a quarter of the view further
/// right last frame (1/8 in UV); the sky did not move.
#[test]
fn a_sideways_step_moves_the_wall_and_not_the_sky() {
    let (device, context, _) = create_device(None).unwrap();
    let mut pass = MotionPass::new(&device).unwrap();
    let (w, h) = (16u32, 4u32);
    let near = 0.1f32;
    let mut projection = [0.0f32; 16];
    projection[0] = 1.0;
    projection[5] = 1.0;
    projection[11] = near;
    projection[14] = -1.0;
    let wall = near / 2.0;
    let depth: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(move |x| if x < w / 2 { wall } else { 0.0 }.to_le_bytes())).collect();
    let depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, D3D11_BIND_SHADER_RESOURCE, Some(&depth));
    let motion = texture(&device, w, h, DXGI_FORMAT_R16G16_FLOAT, D3D11_BIND_UNORDERED_ACCESS, None);
    let depth_out = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, D3D11_BIND_UNORDERED_ACCESS, None);
    let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
    let current = EyeView { camera: camera(0.5), projection };
    let previous = EyeView { camera: camera(0.0), projection };
    pass.run(&context, &depth, &current, Some(&previous), None, &uav(&device, &motion), &uav(&device, &depth_out), (w, h)).unwrap();
    let (_, _, texels) = read_image(&device, &context, &motion).unwrap();
    let vector = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [half::to_f32(u16::from_le_bytes([texels[i], texels[i + 1]])), half::to_f32(u16::from_le_bytes([texels[i + 2], texels[i + 3]]))]
    };
    let [wx, wy] = vector(3, 2);
    assert!((wx - 0.125).abs() < 1e-3 && wy.abs() < 1e-3, "wall moved by {wx}, {wy}");
    let [sx, sy] = vector(12, 2);
    assert!(sx.abs() < 1e-3 && sy.abs() < 1e-3, "sky moved by {sx}, {sy}");
    let (_, _, depths) = read_image(&device, &context, &depth_out).unwrap();
    assert_eq!(f32::from_le_bytes([depths[8], depths[9], depths[10], depths[11]]), wall);
}
