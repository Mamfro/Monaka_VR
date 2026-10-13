use super::*;
use monaka_channel::d3d::{create_device, create_texture_with, read_pixel};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_FLOAT};

fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bytes: &[u8]) -> ID3D11Texture2D {
    create_texture_with(device, &D3D11_TEXTURE2D_DESC { BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..d3d::simple_texture_desc(w, h, format) }, bytes).unwrap()
}

#[test]
fn shaders_compile() {
    for entry in [s!("Clear"), s!("Splat"), s!("Paint"), s!("Resolve")] {
        crate::compile(REPROJECT_SOURCE, "reproject.hlsl", entry).unwrap();
    }
}

#[test]
fn other_eye_from_this_frame_and_gaps_from_its_own_last_frame() {
    let (device, context, _) = create_device(None).unwrap();
    let mut reprojector = Reprojector::new(&device).unwrap();
    let (w, h) = (64u32, 8u32);
    // A standard-depth projection with P00 = P11 = 1 and a wall 2 m away everywhere.
    let (n, f) = (0.1f32, 100.0f32);
    let mut projection = [0.0f32; 16];
    projection[0] = 1.0;
    projection[5] = 1.0;
    projection[10] = f / (n - f);
    projection[11] = n * f / (n - f);
    projection[14] = -1.0;
    let wall = -projection[10] + projection[11] / 2.0;
    let depth: Vec<u8> = (0..w * h).flat_map(|_| wall.to_le_bytes()).collect();
    let depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &depth);
    let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
    // This frame: the left camera, red = 4 * column. The right eye's last frame: plain green.
    let columns: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| [(x * 4) as u8, 0, 0, 255])).collect();
    let green: Vec<u8> = (0..w * h).flat_map(|_| [0, 255, 0, 255]).collect();
    let left = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &columns);
    let last_right = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &green);
    let primary = reprojector.capture(&context, &left, &depth, projection, camera(-0.5), None).unwrap();
    let fill = reprojector.capture(&context, &last_right, &depth, projection, camera(0.5), None).unwrap();
    let mapping = DepthMapping::from_projection(&projection, n).unwrap();
    let view = View { projection, camera: camera(0.5), mapping, width: w, height: h };

    // Cameras 1 m apart, wall 2 m away: the right eye sees each column 16 pixels further left.
    let image = reprojector.run(&context, &view, &primary, Some(&fill)).unwrap().clone();
    assert_eq!(read_pixel(&device, &context, &image, 20, 3).unwrap(), [36 * 4, 0, 0, 255]);
    // The right edge was out of the left camera's view: its own last frame fills it.
    assert_eq!(read_pixel(&device, &context, &image, 55, 3).unwrap(), [0, 255, 0, 255]);
    // Without a fill frame, the gap continues its neighbour instead.
    let image = reprojector.run(&context, &view, &primary, None).unwrap().clone();
    assert_eq!(read_pixel(&device, &context, &image, 55, 3).unwrap(), [63 * 4, 0, 0, 255]);
}

#[test]
fn the_cameras_far_from_the_origin_land_where_they_do_near_it() {
    // The same eyes and wall, the whole scene moved thousands of units away, as a game world's
    // cameras are: the warped image is the one made near the origin.
    let (device, context, _) = create_device(None).unwrap();
    let mut reprojector = Reprojector::new(&device).unwrap();
    let (w, h, near) = (64u32, 8u32, 0.1f32);
    let mut projection = [0.0f32; 16];
    (projection[0], projection[5], projection[11], projection[14]) = (1.0, 1.0, near, -1.0);
    let depth: Vec<u8> = (0..w * h).flat_map(|_| (near / 2.0).to_le_bytes()).collect();
    let depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &depth);
    let columns: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| [(x * 4) as u8, 0, 0, 255])).collect();
    let left = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &columns);
    let mapping = DepthMapping::from_projection(&projection, near).unwrap();
    let mut at = |offset: f32| {
        let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, offset + x, 0.0, 1.0, 0.0, -offset, 0.0, 0.0, 1.0, offset] };
        let primary = reprojector.capture(&context, &left, &depth, projection, camera(-0.5), None).unwrap();
        let view = View { projection, camera: camera(0.5), mapping, width: w, height: h };
        let image = reprojector.run(&context, &view, &primary, None).unwrap().clone();
        (0..w).map(|x| read_pixel(&device, &context, &image, x, 3).unwrap()).collect::<Vec<_>>()
    };
    assert_eq!(at(4000.0), at(0.0));
}
