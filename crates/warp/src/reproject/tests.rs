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
    assert_eq!(read_pixel(&device, &context, &image, 55, 3).unwrap(), [0, 255, 0, 170]);
    // Without a fill frame, the gap continues its neighbour instead.
    let image = reprojector.run(&context, &view, &primary, None).unwrap().clone();
    assert_eq!(read_pixel(&device, &context, &image, 55, 3).unwrap(), [63 * 4, 0, 0, 85]);
}

#[test]
fn a_surface_drawn_larger_keeps_its_own_colour_in_its_cracks() {
    // The wall 2 m away seen from 1 m nearer is drawn twice its size: every other target pixel has no
    // source pixel. Those are cracks in the wall, not gaps beside something nearer: the wall's own
    // colour (as it is, or across the crack: alpha 255 or 85), never the fill's (170).
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
            let pixel = read_pixel(&device, &context, &image, x, y).unwrap();
            assert!(pixel[..3] == [255, 0, 0] && pixel[3] != 170, "pixel {x}, {y}: {pixel:?}");
        }
    }
}

#[test]
fn a_gap_beside_something_near_shows_what_is_behind_it_not_a_near_fill() {
    // A box 0.5 m away in front of a wall 2 m away; the eye 0.1 m to the right sees the box 4.8
    // pixels further left than the wall, a gap opening right of it. The fill frame is all near
    // (green, at the box's depth: a near thing that moved since, as a held bow): the gap shows the wall,
    // continued from beside it (alpha 85).
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
        assert_eq!(read_pixel(&device, &context, &image, x, 3).unwrap(), [wall[0], wall[1], wall[2], 85], "column {x}");
    }
    // The fill showing something 1 m away there (a bar between the box and the wall, which the
    // real eye could not see behind the box): clearly behind the box, so the fill (alpha 170).
    let bar_depth: Vec<u8> = (0..w * h).flat_map(|_| at(1.0).to_le_bytes()).collect();
    let bar_depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &bar_depth);
    let fill = reprojector.capture(&context, &last, &bar_depth, projection, camera(0.1), None).unwrap();
    let image = reprojector.run(&context, &view, &primary, Some(&fill)).unwrap().clone();
    for x in 30..33 {
        assert_eq!(read_pixel(&device, &context, &image, x, 3).unwrap(), [0, 255, 0, 170], "column {x}: the bar from the fill");
    }
}

#[test]
fn a_hole_the_real_eye_could_not_see_in_a_surface_comes_from_the_eyes_own_frame() {
    // A bar 1 m away in front of a wall 4 m away; in the real eye something 0.25 m away hides the
    // bar's middle rows, but not the wall just left of it. The other eye 0.25 m to the right sees
    // the wall shift 4 pixels, the bar 16: that wall slides into the bar's place in those rows (the
    // wall seen through the bar, in the hider's shape). Its own last frame (from where it is) shows
    // the bar there, and the bar is above and below: the bar, from that frame (alpha 170). Where that
    // frame shows something near (a hand that has moved since) with nothing of it around, the warp.
    let (device, context, _) = create_device(None).unwrap();
    let mut reprojector = Reprojector::new(&device).unwrap();
    let (w, h) = (128u32, 32u32);
    let (n, f) = (0.1f32, 100.0f32);
    let mut projection = [0.0f32; 16];
    (projection[0], projection[5], projection[10], projection[11], projection[14]) = (1.0, 1.0, f / (n - f), n * f / (n - f), -1.0);
    let at = |distance: f32| -projection[10] + projection[11] / distance;
    let (wall, bar, hider) = ([255u8, 0, 0, 255], [0u8, 255, 0, 255], [0u8, 0, 255, 255]);
    let hidden = |x: u32, y: u32| (12..20).contains(&y) && (60..90).contains(&x);
    let real = |x: u32, y: u32| if hidden(x, y) { (hider, 0.25) } else if (60..72).contains(&x) { (bar, 1.0) } else { (wall, 4.0) };
    // The other eye's own frame: the bar where it sees it (16 pixels left), and a near blob at 96..104, 2..8.
    let own = |x: u32, y: u32| if (96..104).contains(&x) && (2..8).contains(&y) { (hider, 0.25) } else if (44..56).contains(&x) { (bar, 1.0) } else { (wall, 4.0) };
    let images = |of: &dyn Fn(u32, u32) -> ([u8; 4], f32)| {
        let colour: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| of(x, y).0)).collect();
        let depth: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| at(of(x, y).1).to_le_bytes())).collect();
        (texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &colour), texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &depth))
    };
    let ((real_colour, real_depth), (own_colour, own_depth)) = (images(&real), images(&own));
    let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
    let primary = reprojector.capture(&context, &real_colour, &real_depth, projection, camera(0.0), None).unwrap();
    let fill = reprojector.capture(&context, &own_colour, &own_depth, projection, camera(0.25), None).unwrap();
    let mapping = DepthMapping::from_projection(&projection, n).unwrap();
    let view = View { projection, camera: camera(0.25), mapping, width: w, height: h };
    let image = reprojector.run(&context, &view, &primary, Some(&fill)).unwrap().clone();
    let pixel = |x: u32, y: u32| read_pixel(&device, &context, &image, x, y).unwrap();
    assert_eq!(pixel(50, 5), [0, 255, 0, 255], "the bar, warped");
    for y in 12..20 {
        assert_eq!(pixel(50, y), [0, 255, 0, 170], "row {y}: the bar from the eye's own frame, not the wall");
    }
    assert_eq!(pixel(100, 5), [255, 0, 0, 255], "the wall, warped: the own frame's near blob has nothing of it around");
}

#[test]
fn a_bar_crossing_a_gap_comes_from_the_eyes_own_frame() {
    // A bar 1 m away in front of a wall 4 m away; in the real eye something 0.25 m away hides a block
    // around the bar's middle rows, bar and wall alike. The other eye 0.25 m to the right sees a gap
    // there with only wall left and right of it (what made it has moved off 64 pixels): the bar runs
    // through the gap, above and below it in the warp, so the bar from its own frame (alpha 170), as
    // is the wall beside it there.
    let (device, context, _) = create_device(None).unwrap();
    let mut reprojector = Reprojector::new(&device).unwrap();
    let (w, h) = (128u32, 32u32);
    let (n, f) = (0.1f32, 100.0f32);
    let mut projection = [0.0f32; 16];
    (projection[0], projection[5], projection[10], projection[11], projection[14]) = (1.0, 1.0, f / (n - f), n * f / (n - f), -1.0);
    let at = |distance: f32| -projection[10] + projection[11] / distance;
    let (wall, bar, hider) = ([255u8, 0, 0, 255], [0u8, 255, 0, 255], [0u8, 0, 255, 255]);
    let real = |x: u32, y: u32| if (12..20).contains(&y) && (40..90).contains(&x) { (hider, 0.25) } else if (60..72).contains(&x) { (bar, 1.0) } else { (wall, 4.0) };
    let own = |x: u32, _: u32| if (44..56).contains(&x) { (bar, 1.0) } else { (wall, 4.0) };
    let images = |of: &dyn Fn(u32, u32) -> ([u8; 4], f32)| {
        let colour: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| of(x, y).0)).collect();
        let depth: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| at(of(x, y).1).to_le_bytes())).collect();
        (texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &colour), texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &depth))
    };
    let ((real_colour, real_depth), (own_colour, own_depth)) = (images(&real), images(&own));
    let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
    let primary = reprojector.capture(&context, &real_colour, &real_depth, projection, camera(0.0), None).unwrap();
    let fill = reprojector.capture(&context, &own_colour, &own_depth, projection, camera(0.25), None).unwrap();
    let mapping = DepthMapping::from_projection(&projection, n).unwrap();
    let view = View { projection, camera: camera(0.25), mapping, width: w, height: h };
    let image = reprojector.run(&context, &view, &primary, Some(&fill)).unwrap().clone();
    let pixel = |x: u32, y: u32| read_pixel(&device, &context, &image, x, y).unwrap();
    for y in 13..19 {
        assert_eq!(pixel(50, y), [0, 255, 0, 170], "row {y}: the bar through the gap");
        assert_eq!(pixel(70, y), [255, 0, 0, 170], "row {y}: the wall in the gap");
    }
}

#[test]
fn what_a_large_near_thing_hid_comes_from_the_eyes_own_frame_and_what_moved_does_not() {
    // As above, but what hides the bar in the real eye is large (a hand holding a flashlight, rows
    // 10..118 of 128): no bar within reach above or below the gap. The real eye is asked instead: its
    // own frame's bar is hidden there in the real eye (something nearer at its place), so the bar.
    // A near blob in the own frame (a hand that has moved since) where the real eye sees the wall,
    // past where the blob would be: the warp's wall.
    let (device, context, _) = create_device(None).unwrap();
    let mut reprojector = Reprojector::new(&device).unwrap();
    let (w, h) = (128u32, 128u32);
    let (n, f) = (0.1f32, 100.0f32);
    let mut projection = [0.0f32; 16];
    (projection[0], projection[5], projection[10], projection[11], projection[14]) = (1.0, 1.0, f / (n - f), n * f / (n - f), -1.0);
    let at = |distance: f32| -projection[10] + projection[11] / distance;
    let (wall, bar, hider) = ([255u8, 0, 0, 255], [0u8, 255, 0, 255], [0u8, 0, 255, 255]);
    let real = |x: u32, y: u32| if (10..118).contains(&y) && (40..90).contains(&x) { (hider, 0.25) } else if (60..72).contains(&x) { (bar, 1.0) } else { (wall, 4.0) };
    let own = |x: u32, y: u32| if (20..26).contains(&x) && (120..126).contains(&y) { (hider, 0.25) } else if (44..56).contains(&x) { (bar, 1.0) } else { (wall, 4.0) };
    let images = |of: &dyn Fn(u32, u32) -> ([u8; 4], f32)| {
        let colour: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| of(x, y).0)).collect();
        let depth: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| at(of(x, y).1).to_le_bytes())).collect();
        (texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &colour), texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &depth))
    };
    let ((real_colour, real_depth), (own_colour, own_depth)) = (images(&real), images(&own));
    let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
    let primary = reprojector.capture(&context, &real_colour, &real_depth, projection, camera(0.0), None).unwrap();
    let fill = reprojector.capture(&context, &own_colour, &own_depth, projection, camera(0.25), None).unwrap();
    let mapping = DepthMapping::from_projection(&projection, n).unwrap();
    let view = View { projection, camera: camera(0.25), mapping, width: w, height: h };
    let image = reprojector.run(&context, &view, &primary, Some(&fill)).unwrap().clone();
    let pixel = |x: u32, y: u32| read_pixel(&device, &context, &image, x, y).unwrap();
    for y in [20, 64, 100] {
        assert_eq!(pixel(50, y), [0, 255, 0, 170], "row {y}: the bar behind the large near thing");
    }
    assert_eq!(pixel(22, 122), [255, 0, 0, 255], "the wall: the own frame's blob has moved");
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
