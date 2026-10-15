use super::*;
use monaka_core::depth::DepthMapping;
use monaka_core::math::Vec3;
use monaka_contract::protocol::DepthParams;
use monaka_ffx::Float2;
use monaka_framegen::inputs::Place;
use monaka_ffx::upscale::{GameDispatch, GameResource, create};
use monaka_streamline::{Constants, constants as at};

/// `v` (a row vector) through `m`.
fn row(v: [f32; 4], m: &Mat44) -> [f32; 4] {
    std::array::from_fn(|c| (0..4).map(|k| v[k] * m[k * 4 + c]).sum())
}

/// Where a point seen by `from` at `ndc` (with its depth) lands in `to`'s normalised coordinates,
/// through the motion passes' matrix.
fn landed(from: &Rendered, to: &Rendered, ndc: [f32; 3]) -> [f32; 3] {
    let moved = row([ndc[0], ndc[1], ndc[2], 1.0], &to_clip(from, to).unwrap());
    [moved[0] / moved[3], moved[1] / moved[3], moved[2] / moved[3]]
}

#[test]
fn the_clip_to_clip_matrix_moves_a_point_by_the_eyes_disparity() {
    let left = left_eye();
    let right = left.other_eye(0, 0.064, None);
    for distance in [0.5f32, 2.0, 30.0] {
        // A point `distance` ahead, a little right and down, as the left eye sees it.
        let c = &left.camera;
        let point = std::array::from_fn(|k| c[k * 4 + 3] - distance * c[k * 4 + 2] + 0.3 * c[k * 4] - 0.1 * c[k * 4 + 1]);
        let ndc = seen(&left, point);
        let in_right = landed(&left, &right, ndc);
        // The right eye sees it further left, by the eyes' disparity; the left eye where it was.
        let disparity = 0.064 * 1.036 / distance;
        assert!((ndc[0] - in_right[0] - disparity).abs() < 2e-3, "{distance} m: {} -> {}", ndc[0], in_right[0]);
        assert!((in_right[1] - ndc[1]).abs() < 1e-4, "no vertical move");
        let same = landed(&left, &left, ndc);
        assert!(same.iter().zip(ndc).all(|(a, b)| (a - b).abs() < 1e-5), "{same:?} {ndc:?}");
    }
}

#[test]
fn the_motion_passes_rebase_onto_each_eyes_previous_image_from_what_the_game_measured() {
    let left = left_eye();
    let right = left.other_eye(0, 0.064, None);
    // The right eye real with the head still: the left eye warped from it. The pair before: the
    // left eye real, the right eye warped. Each eye's newest frame before: its own real one.
    let views = [left, right];
    let previous = [Some(left), Some(right)];
    let scale = [0.5, -0.5];
    let c = &left.camera;
    let point: Vec3 = std::array::from_fn(|k| c[k * 4 + 3] - 2.0 * c[k * 4 + 2] + 0.3 * c[k * 4]);
    let seen_by_right = seen(&right, point);
    let through = |m: &Mat44, ndc: [f32; 3]| {
        let moved = row([ndc[0], ndc[1], ndc[2], 1.0], m);
        moved[0] / moved[3] - ndc[0]
    };
    let disparity = 0.064 * 1.036 / 2.0;
    // The game had the left eye's frame before: its vectors reach it, over one frame.
    let other = motion_constants(&views, previous[0].unwrap(), 1.0, 1, &views, scale, [1440, 1600]).unwrap();
    let (real, warped) = (other[1], other[0]);
    assert!((through(&real.game_to_prev, seen_by_right) - disparity).abs() < 2e-3, "the game's previous frame is the left eye's");
    assert!(through(&real.same_to_prev, seen_by_right).abs() < 1e-5, "the right eye's previous image is its own view, the head still");
    assert_eq!((real.object_scale, real.mvec_scale, real.size), (1.0, scale, [0, 0]));
    assert_eq!(warped.game_to_prev, real.game_to_prev, "the warped eye reads the real eye's vectors");
    assert!(through(&warped.same_to_prev, seen(&left, point)).abs() < 1e-5, "the warped left eye's previous image is its own real frame");
    assert!((through(&warped.target_to_source, seen(&left, point)) + disparity).abs() < 2e-3, "from the warped eye's pixels to the real eye's");
    assert_eq!(warped.size, [1440, 1600]);
    // The game rendered the right eye against its own frame before, two frames back: its vectors
    // reach that frame, and carry two frames of object motion.
    let own = motion_constants(&views, previous[1].unwrap(), 0.5, 1, &views, scale, [1440, 1600]).unwrap();
    assert!(through(&own[1].game_to_prev, seen_by_right).abs() < 1e-5, "the game's previous frame is the right eye's own");
    assert_eq!(own[1].object_scale, 0.5);
}

#[test]
fn the_dominant_eye_renders_every_present_but_one_in_each_run() {
    let right: Vec<usize> = (0..REFRESH * 2).map(dominant_eye).collect();
    assert_eq!(right.iter().filter(|&&eye| eye == 1).count() as u64, 2 * (REFRESH - 1), "{right:?}");
    assert_eq!((right[REFRESH as usize - 1], right[2 * REFRESH as usize - 1]), (0, 0), "the left eye at the end of each run");
    assert_eq!(right[0], 1);
}

#[test]
fn each_eyes_camera_is_the_real_eyes_moved_along_its_right_axis() {
    let camera = Camera { position: [1.0, 2.0, 3.0], right: [0.0, 0.0, -1.0], ..Camera::default() };
    assert_eq!(eye_camera(&camera, 0, 0, 0.064).position, [1.0, 2.0, 3.0], "the real eye stays");
    assert_eq!(eye_camera(&camera, 0, 1, 0.064).position, [1.0, 2.0, 3.0 - 0.064], "the right eye to the right");
    assert_eq!(eye_camera(&camera, 1, 0, 0.064).position, [1.0, 2.0, 3.0 + 0.064], "the left eye to the left");
}

#[test]
fn the_constants_give_the_frame_its_camera() {
    // The camera at (1, 2, 3) looking down -z (forward), DLSS's reverse-Z infinite projection.
    let mut bytes = vec![0u8; at::BYTES];
    let mut put = |offset: usize, values: &[f32]| {
        for (i, v) in values.iter().enumerate() {
            bytes[offset + 4 * i..offset + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
    };
    put(at::CAMERA_VIEW_TO_CLIP, &[1.036, 0.0, 0.0, 0.0, 0.0, 1.84, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.05, 0.0]);
    put(at::POSITION, &[1.0, 2.0, 3.0]);
    put(at::UP, &[0.0, 1.0, 0.0]);
    put(at::RIGHT, &[1.0, 0.0, 0.0]);
    put(at::FORWARD, &[0.0, 0.0, -1.0]);
    put(at::NEAR, &[0.05]);
    let constants = Constants::read(bytes.as_ptr()).unwrap();
    let rendering = Rendering::from_constants(&constants);
    let rendered = Rendered::from_rendering(&rendering).unwrap();
    // As the constants say, read straight from them.
    let read = Rendered::from_row_vectors(
        constants.vec3(at::POSITION),
        constants.vec3(at::RIGHT),
        constants.vec3(at::UP),
        constants.vec3(at::FORWARD),
        &constants.matrix(at::CAMERA_VIEW_TO_CLIP),
        constants.f32(at::NEAR),
    );
    assert_eq!(Some(rendered), read);
    assert_eq!([rendered.camera[3], rendered.camera[7], rendered.camera[11]], [1.0, 2.0, 3.0]);
    assert_eq!(rendered.mapping, DepthMapping { a: 1.0, b: 0.0 }, "the depth is the nearness");
    assert_eq!(rendered.camera[10], 1.0, "the camera's back axis is +z");
    assert_eq!(rendered.depth_params(1.0), Some(DepthParams::new(0.05, f32::INFINITY, true)));
    assert!(warpable(&rendering));
    assert!(!warpable(&Rendering::from_constants(&Constants::read([0u8; at::BYTES].as_ptr()).unwrap())), "no camera");
}

#[test]
fn fsrs_dispatch_gives_the_frame_the_camera_dlss_constants_give() {
    // The same frame as DLSS's constants and as FSR's dispatch with the game's place for it.
    let mut bytes = vec![0u8; at::BYTES];
    let mut put = |offset: usize, values: &[f32]| {
        for (i, v) in values.iter().enumerate() {
            bytes[offset + 4 * i..offset + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
    };
    let place = Place { position: [1.0, 2.0, 3.0], right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], forward: [0.0, 0.0, -1.0] };
    put(at::CAMERA_VIEW_TO_CLIP, &[1.036, 0.0, 0.0, 0.0, 0.0, 1.84, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.05, 0.0]);
    put(at::POSITION, &place.position);
    put(at::UP, &place.up);
    put(at::RIGHT, &place.right);
    put(at::FORWARD, &place.forward);
    put(at::NEAR, &[0.05]);
    let dlss = Rendered::from_rendering(&Rendering::from_constants(&Constants::read(bytes.as_ptr()).unwrap())).unwrap();
    let resource = GameResource { resource: 0, format: 0, width: 2560, height: 1440, state: 0 };
    let dispatch = GameDispatch {
        command_list: 0,
        color: resource,
        depth: resource,
        motion_vectors: resource,
        reactive: resource,
        transparency_and_composition: resource,
        output: resource,
        jitter: Float2::default(),
        motion_vector_scale: Float2 { x: 1.0, y: 1.0 },
        render_size: Size2 { width: 2560, height: 1440 },
        upscale_size: Size2 { width: 3840, height: 2160 },
        sharpening: None,
        frame_time_ms: 16.0,
        pre_exposure: 1.0,
        reset: false,
        near: 0.05,
        far: 4000.0,
        fov_vertical: 1.0,
        view_space_to_meters: 1.0,
        flags: 0,
    };
    let frustum = Frustum::centred([1.0 / 1.036, 1.0 / 1.84]).unwrap();
    let flags = create::DEPTH_INVERTED | create::DEPTH_INFINITE;
    let fsr = Rendered::from_rendering(&Rendering::from_dispatch(&dispatch, place, &frustum, flags).unwrap()).unwrap();
    assert_eq!(fsr.camera, dlss.camera);
    assert!(fsr.projection.iter().zip(dlss.projection).all(|(a, b)| (a - b).abs() < 1e-5), "{:?} {:?}", fsr.projection, dlss.projection);
    assert!((fsr.mapping.a - dlss.mapping.a).abs() < 1e-5 && (fsr.mapping.b - dlss.mapping.b).abs() < 1e-5);
    assert_eq!(fsr.depth_params(1.0), dlss.depth_params(1.0));
}
use monaka_channel::d3d12::read_texture;
use monaka_core::depth::reprojection;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM};

/// The Beast's view-to-clip (row vectors): reverse-Z infinite, near 0.05.
const NEAR: f32 = 0.05;
const VIEW_TO_CLIP: Mat44 = [1.036, 0.0, 0.0, 0.0, 0.0, 1.8418, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, NEAR, 0.0];

fn apply(m: &Mat44, v: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|row| (0..4).map(|k| m[row * 4 + k] * v[k]).sum())
}

/// Where world point `p` is seen by `rendered`: normalised x, y and its depth.
fn seen(rendered: &Rendered, p: Vec3) -> [f32; 3] {
    let c = &rendered.camera;
    let d = [p[0] - c[3], p[1] - c[7], p[2] - c[11]];
    let view = std::array::from_fn::<f32, 3, _>(|k| c[k] * d[0] + c[4 + k] * d[1] + c[8 + k] * d[2]);
    let clip = apply(&rendered.projection, [view[0], view[1], view[2], 1.0]);
    [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]]
}

/// A game camera far from the origin, turned 30 degrees and pitched down a little.
fn left_eye() -> Rendered {
    let (yaw, pitch) = (30f32.to_radians(), -10f32.to_radians());
    let forward = [-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos()];
    let right = [yaw.cos(), 0.0, -yaw.sin()];
    let up = monaka_core::math::cross(right, forward.map(|v| -v));
    Rendered::from_row_vectors([-338.0, 64.3, -116.1], right, up, forward, &VIEW_TO_CLIP, NEAR).unwrap()
}

#[test]
fn the_constants_give_a_reverse_infinite_depth_and_a_camera_looking_forward() {
    let left = left_eye();
    assert_eq!(left.mapping, DepthMapping { a: 1.0, b: 0.0 }, "the depth is the nearness");
    // A point 2 m straight ahead: the centre of the image, depth near / 2.
    let forward = [-left.camera[2], -left.camera[6], -left.camera[10]];
    let ahead = std::array::from_fn(|k| left.camera[k * 4 + 3] + 2.0 * forward[k]);
    let [x, y, d] = seen(&left, ahead);
    assert!(x.abs() < 1e-4 && y.abs() < 1e-4 && (d - NEAR / 2.0).abs() < 1e-5, "{x} {y} {d}");
    let skewed = [1.0, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    assert!(Rendered::new(skewed, left.projection, NEAR).is_none(), "not a rigid camera");
}

#[test]
fn a_point_seen_by_the_left_eye_lands_where_the_right_eye_sees_it() {
    let left = left_eye();
    let right = left.other_eye(0, 0.064, None);
    let gap = monaka_core::math::distance([left.camera[3], left.camera[7], left.camera[11]], [right.camera[3], right.camera[7], right.camera[11]]);
    assert!((gap - 0.064).abs() < 1e-4, "the eyes one separation apart: {gap}");
    // Points 0.3 m to 40 m away, off to the side and up.
    for (distance, side, height) in [(0.3f32, 0.05f32, -0.02f32), (2.0, -0.4, 0.3), (40.0, 6.0, -3.0)] {
        let c = &left.camera;
        let point = std::array::from_fn(|k| c[k * 4 + 3] - distance * c[k * 4 + 2] + side * c[k * 4] + height * c[k * 4 + 1]);
        let [x, y, d] = seen(&left, point);
        let expected = seen(&right, point);
        // The warp's matrix, built as the reprojector builds it (from the target's position).
        let to_origin = |r: &Rendered| {
            let mut c = r.camera;
            (c[3], c[7], c[11]) = (c[3] - right.camera[3], c[7] - right.camera[7], c[11] - right.camera[11]);
            c
        };
        let m = reprojection(&left.projection, &to_origin(&left), &right.projection, &to_origin(&right)).unwrap();
        let clip = apply(&m, [x, y, d, 1.0]);
        let landed = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
        for k in 0..3 {
            assert!((landed[k] - expected[k]).abs() < 2e-4, "{distance} m: {landed:?} {expected:?}");
        }
        // Seen further left by the right eye, by the eyes' disparity.
        let disparity = 0.064 * 1.036 / distance;
        assert!((x - landed[0] - disparity).abs() < 2e-3, "{distance} m: {x} -> {}", landed[0]);
    }
}

#[test]
fn the_other_eye_takes_its_own_frustum_and_keeps_the_depth_terms() {
    let left = left_eye();
    let frustum = Frustum::centred([1.2, 0.6]).unwrap();
    let right = left.other_eye(0, 0.064, Some(frustum));
    assert!((right.projection[0] - 1.0 / 1.2).abs() < 1e-6 && (right.projection[5] - 1.0 / 0.6).abs() < 1e-6);
    assert_eq!((right.projection[10], right.projection[11], right.projection[14]), (left.projection[10], left.projection[11], left.projection[14]));
    let back = right.other_eye(1, 0.064, None);
    assert!(monaka_core::camera::distance(&back.camera, &left.camera) < 1e-4, "the right eye's other eye is the left");
}

fn device() -> ID3D12Device {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: creates a device on the default adapter.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    device.unwrap()
}

/// A `w` x `h` image cleared to `rgba` and a depth image at half its size cleared to `depth`,
/// both in COMMON.
fn frame(device: &ID3D12Device, queue: &ID3D12CommandQueue, (w, h): (u32, u32), rgba: [f32; 4], depth: f32) -> (ID3D12Resource, ID3D12Resource) {
    let color = d3d12::texture(device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET).unwrap();
    let depth_image = d3d12::texture(device, w / 2, h / 2, DXGI_FORMAT_D32_FLOAT, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL).unwrap();
    // SAFETY: views of our own textures in heaps of our own, cleared on a list of ours that is
    // waited for.
    unsafe {
        let rtv: ID3D12DescriptorHeap =
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, ..Default::default() }).unwrap();
        let dsv: ID3D12DescriptorHeap =
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV, NumDescriptors: 1, ..Default::default() }).unwrap();
        let (rtv, dsv) = (rtv.GetCPUDescriptorHandleForHeapStart(), dsv.GetCPUDescriptorHandleForHeapStart());
        device.CreateRenderTargetView(&color, None, rtv);
        device.CreateDepthStencilView(&depth_image, None, dsv);
        let mut recorder = Recorder::new(device, 1).unwrap();
        let list = recorder.begin().unwrap().unwrap();
        d3d12::barrier(&list, &color, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_RENDER_TARGET);
        d3d12::barrier(&list, &depth_image, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_DEPTH_WRITE);
        list.ClearRenderTargetView(rtv, &rgba, None);
        list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, depth, 0, None);
        d3d12::barrier(&list, &color, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COMMON);
        d3d12::barrier(&list, &depth_image, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_COMMON);
        recorder.submit(queue).unwrap();
        assert!(recorder.idle(2000));
    }
    (color, depth_image)
}

fn pixel(queue: &ID3D12CommandQueue, texture: &ID3D12Resource, x: usize, y: usize) -> [u8; 4] {
    let (_, _, pitch, bytes) = read_texture(queue, texture, D3D12_RESOURCE_STATE_COMMON).unwrap();
    bytes[y * pitch + x * 4..][..4].try_into().unwrap()
}

#[test]
fn each_present_makes_a_pair_whose_gaps_come_from_the_warped_eyes_own_last_frame() {
    let device = device();
    // SAFETY: creates a direct queue on a live device.
    let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }) }.unwrap();
    // The warp used wherever it has pixels (the fill first would show the eye's own frame on the wall).
    let mut afw = Afw12::new(&device, 1, RULES.fill_first(false)).unwrap();
    // Eyes 1 m apart, square pixels and a 90 degree view, a wall 2 m away: the eyes see it 16
    // pixels apart on a 64 pixel wide image.
    let size = (64, 8);
    let mut view_to_clip = VIEW_TO_CLIP;
    (view_to_clip[0], view_to_clip[5]) = (1.0, 8.0);
    let left = Rendered::from_row_vectors([10.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], &view_to_clip, NEAR).unwrap();
    let right = left.other_eye(0, 1.0, None);
    let (red, green) = ([1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]);
    let common = D3D12_RESOURCE_STATE_COMMON;
    let present = |afw: &mut Afw12, eye: usize, rgba| {
        let (color, depth) = frame(&device, &queue, size, rgba, NEAR / 2.0);
        let (rendered, other) = if eye == 0 { (left, right) } else { (right, left) };
        let shot = Shot { eye, pose: HeadPose::default(), rendered, other };
        let real = RealEye { color: &color, color_state: common, depth: &depth, depth_state: common, depth_rect: None, jitter: [0.0; 2] };
        let pair = afw.warp(&queue, &shot, &real).unwrap().unwrap();
        assert!(afw.idle(2000));
        pair
    };

    // The left eye first: the right eye is all warped, its right edge continued. A warped pixel's
    // alpha says where it came from (`reproject.hlsl`): 255 the real eye, 170 the eye's own last
    // frame, 85 a neighbour.
    let pair = present(&mut afw, 0, red);
    assert_eq!((pair.real_eye, pair.filled), (0, false));
    assert_eq!(pixel(&queue, &pair.images[0], 30, 4), [255, 0, 0, 255], "the real eye on the left");
    assert_eq!(pixel(&queue, &pair.images[1], 60, 4), [255, 0, 0, 85], "continued from a neighbour");
    // Then the right eye: the left eye's left edge, which the right eye could not see, comes
    // from the left eye's own last real frame.
    let pair = present(&mut afw, 1, green);
    assert_eq!((pair.real_eye, pair.filled), (1, true));
    assert_eq!(pair.previous, [Some(left), None], "the left eye's frame before; the right eye had none");
    assert_eq!(pixel(&queue, &pair.images[1], 30, 4), [0, 255, 0, 255], "the real eye on the right");
    assert_eq!(pixel(&queue, &pair.images[0], 40, 4), [0, 255, 0, 255], "warped from the right eye");
    assert_eq!(pixel(&queue, &pair.images[0], 5, 4), [255, 0, 0, 170], "the gap from its own last frame");
    // Both eyes' depth: the wall, each in its own view.
    let (_, _, pitch, bytes) = read_texture(&queue, &afw.depth(0).unwrap().0.clone(), common).unwrap();
    let wall = f32::from_le_bytes(bytes[2 * pitch + 20 * 4..][..4].try_into().unwrap());
    assert!((wall - NEAR / 2.0).abs() < 1e-6, "{wall}");
    // After a cut nothing old fills the gaps.
    afw.forget();
    let pair = present(&mut afw, 0, red);
    assert!(!pair.filled);
    assert_eq!(afw.counts().pairs, 3);
}
