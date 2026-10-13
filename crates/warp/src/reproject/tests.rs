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
fn a_surface_drawn_larger_keeps_its_own_colour_in_its_cracks() {
    // The wall 2 m away seen from 1 m nearer is drawn twice its size: every other target pixel has no
    // source pixel. Those are cracks in the wall, not gaps beside something nearer: the wall's own
    // colour, not the fill's.
    let (device, context, _) = create_device(None).unwrap();
    let mut reprojector = Reprojector::new(&device).unwrap();
    let (w, h) = (64u32, 16u32);
    let (n, f) = (0.1f32, 100.0f32);
    let mut projection = [0.0f32; 16];
    (projection[0], projection[5], projection[10], projection[11], projection[14]) = (1.0, 1.0, f / (n - f), n * f / (n - f), -1.0);
    let wall = -projection[10] + projection[11] / 2.0;
    let depth: Vec<u8> = (0..w * h).flat_map(|_| wall.to_le_bytes()).collect();
    let depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &depth);
    let camera = |z: f32| -> Mat34 { [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, z] };
    let red: Vec<u8> = (0..w * h).flat_map(|_| [255, 0, 0, 255]).collect();
    let green: Vec<u8> = (0..w * h).flat_map(|_| [0, 255, 0, 255]).collect();
    let this = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &red);
    let last = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &green);
    let primary = reprojector.capture(&context, &this, &depth, projection, camera(0.0), None).unwrap();
    let fill = reprojector.capture(&context, &last, &depth, projection, camera(-1.0), None).unwrap();
    let mapping = DepthMapping::from_projection(&projection, n).unwrap();
    let view = View { projection, camera: camera(-1.0), mapping, width: w, height: h };
    let image = reprojector.run(&context, &view, &primary, Some(&fill)).unwrap().clone();
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            assert_eq!(read_pixel(&device, &context, &image, x, y).unwrap(), [255, 0, 0, 255], "pixel {x}, {y}");
        }
    }
}

#[test]
fn a_gap_beside_something_near_shows_what_is_behind_it_not_a_near_fill() {
    // A box 0.5 m away in front of a wall 2 m away; the eye 0.1 m to the right sees the box 4.8
    // pixels further left than the wall, a gap opening right of it. The fill frame is all near
    // (green, at the box's depth: a near thing that moved since, as a held bow): the gap shows the wall.
    let (device, context, _) = create_device(None).unwrap();
    let mut reprojector = Reprojector::new(&device).unwrap();
    let (w, h) = (64u32, 8u32);
    let (n, f) = (0.1f32, 100.0f32);
    let mut projection = [0.0f32; 16];
    (projection[0], projection[5], projection[10], projection[11], projection[14]) = (1.0, 1.0, f / (n - f), n * f / (n - f), -1.0);
    let at = |distance: f32| -projection[10] + projection[11] / distance;
    let (box_columns, wall, near) = (24..36u32, [255u8, 0, 0, 255], [0u8, 0, 255, 255]);
    let depth: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| at(if box_columns.contains(&x) { 0.5 } else { 2.0 }).to_le_bytes())).collect();
    let depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &depth);
    let colours: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| if box_columns.contains(&x) { near } else { wall })).collect();
    let this = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &colours);
    let near_depth: Vec<u8> = (0..w * h).flat_map(|_| at(0.5).to_le_bytes()).collect();
    let near_depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &near_depth);
    let green: Vec<u8> = (0..w * h).flat_map(|_| [0, 255, 0, 255]).collect();
    let last = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &green);
    let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
    let primary = reprojector.capture(&context, &this, &depth, projection, camera(0.0), None).unwrap();
    let fill = reprojector.capture(&context, &last, &near_depth, projection, camera(0.1), None).unwrap();
    let mapping = DepthMapping::from_projection(&projection, n).unwrap();
    let view = View { projection, camera: camera(0.1), mapping, width: w, height: h };
    let image = reprojector.run(&context, &view, &primary, Some(&fill)).unwrap().clone();
    // The box: source columns 24..36 land 6.4 pixels left, 17..29; the wall 1.6 left; the gap between.
    assert_eq!(read_pixel(&device, &context, &image, 22, 3).unwrap(), near);
    for x in 30..33 {
        assert_eq!(read_pixel(&device, &context, &image, x, 3).unwrap(), wall, "column {x}");
    }
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
