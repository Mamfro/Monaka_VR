use super::*;
use monaka_channel::d3d12::{Recorder, placed, read_texture, subresource};
use monaka_core::depth::DepthMapping;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;

#[test]
fn shaders_compile() {
    for entry in [s!("Clear"), s!("Splat"), s!("Paint"), s!("Resolve"), s!("ResolveDepth")] {
        compute::compile(REPROJECT12_SOURCE, "reproject12.hlsl", entry).unwrap();
    }
    assert_eq!(uav_format(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), Some(DXGI_FORMAT_R8G8B8A8_UNORM));
    assert_eq!(uav_format(DXGI_FORMAT_R11G11B10_FLOAT), None);
}

fn device() -> ID3D12Device {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: creates a device on the default adapter.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    device.unwrap()
}

fn queue(device: &ID3D12Device) -> ID3D12CommandQueue {
    // SAFETY: creates a direct queue on a live device.
    unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }) }.unwrap()
}

/// A `w` x `h` texture of `format` (four bytes a texel) holding `bytes`, in COMMON.
fn upload(device: &ID3D12Device, queue: &ID3D12CommandQueue, (w, h): (u32, u32), format: DXGI_FORMAT, bytes: &[u8]) -> ID3D12Resource {
    let texture = d3d12::texture(device, w, h, format, D3D12_RESOURCE_FLAG_NONE).unwrap();
    // SAFETY: a layout query, an upload buffer of ours written through its mapping within its
    // size, and a copy recorded and waited for.
    unsafe {
        let desc = texture.GetDesc();
        let (mut footprint, mut total) = (D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(), 0u64);
        device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, Some(&mut total));
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
        let buffer_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: total,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut buffer: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer_desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut buffer).unwrap();
        let buffer = buffer.unwrap();
        let mut mapped = std::ptr::null_mut();
        buffer.Map(0, None, Some(&mut mapped)).unwrap();
        let row = w as usize * 4;
        for y in 0..h as usize {
            std::ptr::copy_nonoverlapping(bytes[y * row..].as_ptr(), mapped.cast::<u8>().add(footprint.Offset as usize + y * footprint.Footprint.RowPitch as usize), row);
        }
        buffer.Unmap(0, None);
        let mut recorder = Recorder::new(device, 1).unwrap();
        let list = recorder.begin().unwrap().unwrap();
        barrier(&list, &texture, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST);
        list.CopyTextureRegion(&subresource(&texture, 0), 0, 0, 0, &placed(&buffer, footprint), None);
        barrier(&list, &texture, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON);
        recorder.submit(queue).unwrap();
        assert!(recorder.idle(2000));
    }
    texture
}

fn pixel(queue: &ID3D12CommandQueue, texture: &ID3D12Resource, x: usize, y: usize) -> [u8; 4] {
    let (_, _, pitch, bytes) = read_texture(queue, texture, D3D12_RESOURCE_STATE_COMMON).unwrap();
    bytes[y * pitch + x * 4..][..4].try_into().unwrap()
}

fn depth_at(queue: &ID3D12CommandQueue, texture: &ID3D12Resource, x: usize, y: usize) -> f32 {
    let (_, _, pitch, bytes) = read_texture(queue, texture, D3D12_RESOURCE_STATE_COMMON).unwrap();
    f32::from_le_bytes(bytes[y * pitch + x * 4..][..4].try_into().unwrap())
}

#[test]
fn other_eye_from_this_frame_and_gaps_from_its_own_last_frame() {
    let device = device();
    let queue = queue(&device);
    let mut reprojector = Reprojector12::new(&device).unwrap();
    // Colour 64x8, depth at half that (the render size): reverse-Z infinite with P00 = P11 = 1
    // and near 0.1, a wall 2 m away everywhere (depth = near / distance).
    let (w, h, near) = (64u32, 8u32, 0.1f32);
    let mut projection = [0.0f32; 16];
    (projection[0], projection[5], projection[11], projection[14]) = (1.0, 1.0, near, -1.0);
    let wall = near / 2.0;
    let depth: Vec<u8> = (0..(w / 2) * (h / 2)).flat_map(|_| wall.to_le_bytes()).collect();
    let depth = upload(&device, &queue, (w / 2, h / 2), DXGI_FORMAT_R32_TYPELESS, &depth);
    let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
    // This frame: the left camera, red = 4 * column. The right eye's last frame: plain green.
    let columns: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| [(x * 4) as u8, 0, 0, 255])).collect();
    let green: Vec<u8> = (0..w * h).flat_map(|_| [0, 255, 0, 255]).collect();
    let left = upload(&device, &queue, (w, h), DXGI_FORMAT_R8G8B8A8_UNORM, &columns);
    let last_right = upload(&device, &queue, (w, h), DXGI_FORMAT_R8G8B8A8_UNORM, &green);
    let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    let out = d3d12::texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, uav).unwrap();
    let out_depth = d3d12::texture(&device, w / 2, h / 2, DXGI_FORMAT_R32_TYPELESS, uav).unwrap();
    let mapping = DepthMapping::from_projection(&projection, near).unwrap();
    // The camera positions far from the origin, as a game world's are.
    let view = View { projection, camera: camera(300.5), mapping, width: w, height: h };
    let primary = Source { color: &left, depth: &depth, depth_rect: None, jitter: [0.0; 2], projection, camera: camera(299.5) };
    let fill = Source { color: &last_right, depth: &depth, depth_rect: None, jitter: [0.0; 2], projection, camera: camera(300.5) };
    let targets = Targets { color: &out, depth: Some(&out_depth), depth_rect: None };
    let run = |reprojector: &mut Reprojector12, primary: &Source, fill: Option<&Source>, targets: &Targets| {
        let mut recorder = Recorder::new(&device, 1).unwrap();
        let list = recorder.begin().unwrap().unwrap();
        reprojector.record(&list, &view, primary, fill, targets).unwrap();
        recorder.submit(&queue).unwrap();
        assert!(recorder.idle(2000));
    };

    // Cameras 1 m apart, wall 2 m away: the right eye sees each column 16 pixels further left.
    run(&mut reprojector, &primary, Some(&fill), &targets);
    assert_eq!(pixel(&queue, &out, 20, 3), [36 * 4, 0, 0, 255]);
    // The right edge was out of the left camera's view: its own last frame fills it.
    assert_eq!(pixel(&queue, &out, 55, 3), [0, 255, 0, 255]);
    // The right eye's depth: the wall, at the render size.
    assert!((depth_at(&queue, &out_depth, 10, 2) - wall).abs() < 1e-6);
    assert!((depth_at(&queue, &out_depth, 28, 2) - wall).abs() < 1e-6, "filled from the last frame's depth");
    // Without a fill frame, the gap continues its neighbour instead.
    run(&mut reprojector, &primary, None, &targets);
    assert_eq!(pixel(&queue, &out, 55, 3), [63 * 4, 0, 0, 255]);

    // A depth texture twice the render area, the wall in its top-left part and a far surface (depth
    // 0, infinity) elsewhere: read in the part that lines up with the image, the same picture, and
    // the output depth written in that part.
    let far_outside: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| if x < w / 2 && y < h / 2 { wall.to_le_bytes() } else { 0f32.to_le_bytes() })).collect();
    let large = upload(&device, &queue, (w, h), DXGI_FORMAT_R32_TYPELESS, &far_outside);
    let large_out = d3d12::texture(&device, w, h, DXGI_FORMAT_R32_TYPELESS, uav).unwrap();
    let rect = Some([0, 0, w / 2, h / 2]);
    let primary_large = Source { depth: &large, depth_rect: rect, ..primary };
    let fill_large = Source { depth: &large, depth_rect: rect, ..fill };
    let targets_large = Targets { color: &out, depth: Some(&large_out), depth_rect: rect };
    run(&mut reprojector, &primary_large, Some(&fill_large), &targets_large);
    assert_eq!(pixel(&queue, &out, 20, 3), [36 * 4, 0, 0, 255]);
    assert_eq!(pixel(&queue, &out, 55, 3), [0, 255, 0, 255]);
    assert!((depth_at(&queue, &large_out, 10, 2) - wall).abs() < 1e-6, "written in the render area");
    // The depth read moved by its jitter: a far column (depth 0, infinity) at depth pixel 30 is read
    // by colour pixels 60 and 61 as rendered, by 58 and 59 with a jitter of one depth pixel. A far
    // pixel lands on itself (no disparity), where no wall pixel lands (they all move 16 left).
    let notch: Vec<u8> = (0..h / 2).flat_map(|_| (0..w / 2).flat_map(|x| if x == 30 { 0f32.to_le_bytes() } else { wall.to_le_bytes() })).collect();
    let notched = upload(&device, &queue, (w / 2, h / 2), DXGI_FORMAT_R32_TYPELESS, &notch);
    let primary_notched = Source { depth: &notched, jitter: [0.0, 0.0], ..primary };
    run(&mut reprojector, &primary_notched, None, &targets);
    assert_eq!(pixel(&queue, &out, 60, 3), [60 * 4, 0, 0, 255], "far as rendered");
    let primary_jittered = Source { jitter: [1.0, 0.0], ..primary_notched };
    run(&mut reprojector, &primary_jittered, None, &targets);
    assert_eq!(pixel(&queue, &out, 58, 3), [58 * 4, 0, 0, 255], "far with the jitter");
    assert_eq!(pixel(&queue, &out, 60, 3), [59 * 4, 0, 0, 255], "a gap now, continued from its neighbour");
}
