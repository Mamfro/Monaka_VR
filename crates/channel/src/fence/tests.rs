use super::*;
use crate::d3d::{create_device, create_texture, find_adapter, read_pixel, simple_texture_desc};
use crate::d3d12::wait_fence;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CLEAR_FLAG_DEPTH, D3D12_COMMAND_QUEUE_DESC, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
    D3D12_HEAP_FLAG_NONE, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_RENDER_TARGET,
    D3D12CreateDevice, ID3D12CommandAllocator, ID3D12CommandList, ID3D12DescriptorHeap, ID3D12GraphicsCommandList, ID3D12PipelineState,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_D32_FLOAT_S8X24_UINT, DXGI_FORMAT_R8G8B8A8_UNORM};

const RT: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_RENDER_TARGET;

fn gpu12() -> (ID3D12Device, ID3D12CommandQueue) {
    unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap();
        let device = device.unwrap();
        let queue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
        (device, queue)
    }
}

/// A 64x32 render target cleared to `rgba`, left in the render-target state, GPU work finished.
fn cleared(device: &ID3D12Device, queue: &ID3D12CommandQueue, rgba: [f32; 4]) -> ID3D12Resource {
    unsafe {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: 64,
            Height: 32,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ..Default::default()
        };
        let mut texture: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, RT, None, &mut texture).unwrap();
        let texture = texture.unwrap();
        let views: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, ..Default::default() })
            .unwrap();
        let view = views.GetCPUDescriptorHandleForHeapStart();
        device.CreateRenderTargetView(&texture, None, view);
        let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>).unwrap();
        list.ClearRenderTargetView(view, &rgba, None);
        list.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().unwrap())]);
        let fence: ID3D12Fence = device.CreateFence(0, windows::Win32::Graphics::Direct3D12::D3D12_FENCE_FLAG_NONE).unwrap();
        queue.Signal(&fence, 1).unwrap();
        assert!(wait_fence(&fence, 1, 5000));
        texture
    }
}

fn until<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn pairs_cross_from_d3d12_to_d3d11() {
    let name = ChannelName::new(format!(r"Local\monaka-test-fence-{}", std::process::id())).unwrap();
    let (device12, queue) = gpu12();
    let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    assert!(FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).is_err(), "a second producer must be refused");

    let described = describe(&name).unwrap();
    assert_eq!((described.width, described.height, described.format), (64, 32, DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32));
    let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
    let mut reader = FenceReader::open(&name, &device).unwrap();
    assert!(matches!(FenceReader::open(&name, &device), Err(Error::ReaderTaken)));

    let targets: [ID3D11Texture2D; 2] =
        std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
    let targets = [&targets[0], &targets[1]];
    assert_eq!(reader.read(&context, targets, 0).unwrap(), None);

    let left = cleared(&device12, &queue, [1.0, 0.0, 0.0, 1.0]);
    let right = cleared(&device12, &queue, [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(producer.next_sequence(), 1);
    assert!(producer.publish((&left, RT), (&right, RT), 7).unwrap());
    let pair = until(|| reader.read(&context, targets, 0).unwrap());
    assert_eq!(pair, Pair { sequence: 1, tick: 7 });
    assert_eq!(read_pixel(&device, &context, targets[0], 3, 3).unwrap(), [255, 0, 0, 255]);
    assert_eq!(read_pixel(&device, &context, targets[1], 3, 3).unwrap(), [0, 255, 0, 255]);

    // The producer keeps finding free slots while the reader keeps up.
    let mut seen = pair.sequence;
    for _ in 0..10 {
        until(|| producer.publish((&right, RT), (&left, RT), 8).unwrap().then_some(()));
        seen = until(|| reader.read(&context, targets, seen).unwrap()).sequence;
    }
    assert_eq!(seen, 11);
    assert_eq!(read_pixel(&device, &context, targets[0], 3, 3).unwrap(), [0, 255, 0, 255]);
}

#[test]
fn a_d3d12_channel_for_one_image_sends_it_as_both_eyes() {
    let name = ChannelName::new(format!(r"Local\monaka-test-fence-mono-{}", std::process::id())).unwrap();
    let (device12, queue) = gpu12();
    let mut producer = FenceProducer::create_mono(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    let described = describe(&name).unwrap();
    let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
    let mut reader = FenceReader::open(&name, &device).unwrap();
    let image = cleared(&device12, &queue, [0.0, 0.0, 1.0, 1.0]);
    let other = cleared(&device12, &queue, [1.0, 0.0, 0.0, 1.0]);
    assert!(producer.publish((&image, RT), (&other, RT), 1).is_err(), "two images are refused");
    assert!(producer.publish_one((&image, RT), 7).unwrap());
    let targets: [ID3D11Texture2D; 2] = std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
    let pair = until(|| reader.read(&context, [&targets[0], &targets[1]], 0).unwrap());
    assert_eq!(pair, Pair { sequence: 1, tick: 7 });
    for target in &targets {
        assert_eq!(read_pixel(&device, &context, target, 3, 3).unwrap(), [0, 0, 255, 255], "the one image in each eye");
    }
}

/// A 64x32 depth buffer of `format` cleared to `depth`, left in the depth-write state, GPU work
/// finished.
fn cleared_depth(device: &ID3D12Device, queue: &ID3D12CommandQueue, format: DXGI_FORMAT, depth: f32) -> ID3D12Resource {
    unsafe {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: 64,
            Height: 32,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
            ..Default::default()
        };
        let mut texture: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, DEPTH, None, &mut texture).unwrap();
        let texture = texture.unwrap();
        let views: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV, NumDescriptors: 1, ..Default::default() })
            .unwrap();
        let view = views.GetCPUDescriptorHandleForHeapStart();
        device.CreateDepthStencilView(&texture, None, view);
        let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>).unwrap();
        list.ClearDepthStencilView(view, D3D12_CLEAR_FLAG_DEPTH, depth, 0, None);
        list.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().unwrap())]);
        let fence: ID3D12Fence = device.CreateFence(0, windows::Win32::Graphics::Direct3D12::D3D12_FENCE_FLAG_NONE).unwrap();
        queue.Signal(&fence, 1).unwrap();
        assert!(wait_fence(&fence, 1, 5000));
        texture
    }
}

const DEPTH: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_DEPTH_WRITE;

/// The first texel of a 4-byte float texture on the reader's device.
fn depth_at_origin(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D) -> f32 {
    let (_, _, texel, texels) = crate::d3d::read_texels(device, context, texture).unwrap();
    assert_eq!(texel, 4);
    f32::from_le_bytes(texels[..4].try_into().unwrap())
}

#[test]
fn depth_crosses_beside_the_eyes() {
    let name = ChannelName::new(format!(r"Local\monaka-test-depth-{}", std::process::id())).unwrap();
    let (device12, queue) = gpu12();
    let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    let described = describe(&name).unwrap();
    let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
    let mut reader = FenceReader::open(&name, &device).unwrap();
    let targets: [ID3D11Texture2D; 2] =
        std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
    let targets = [&targets[0], &targets[1]];
    let image = cleared(&device12, &queue, [1.0, 0.0, 0.0, 1.0]);

    // 32-bit float depth, one eye with stencil: both travel as R32.
    let left = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.25);
    let right = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT_S8X24_UINT, 0.75);
    assert!(depth_copyable(&left) && depth_copyable(&right) && !depth_copyable(&image));
    let params = DepthParams::new(0.05, f32::INFINITY, true);
    let depth = Depth {
        eyes: [EyeDepth { texture: &left, state: DEPTH, rect: None }, EyeDepth { texture: &right, state: DEPTH, rect: Some([8, 0, 48, 32]) }],
        params,
    };
    assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(depth), 3).unwrap());
    let (pair, read) = until(|| reader.read_with_depth(&context, targets, 0).unwrap());
    assert_eq!(pair.sequence, 1);
    let read = read.expect("the pair came with depth");
    assert_eq!(read, ReadDepth { params, rects: [[0, 0, 64, 32], [8, 0, 48, 32]], format: dxgi_format::R32_TYPELESS, width: 64, height: 32 });
    let copies = reader.depth_textures().unwrap();
    assert_eq!(depth_at_origin(&device, &context, copies[0]), 0.25);
    assert_eq!(depth_at_origin(&device, &context, copies[1]), 0.75);

    // The viewer copies it on into a depth swapchain image of the runtime's (D32 float).
    let mut desc = simple_texture_desc(64, 32, windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_D32_FLOAT);
    desc.BindFlags = windows::Win32::Graphics::Direct3D11::D3D11_BIND_DEPTH_STENCIL.0 as u32;
    let swapchain_image = create_texture(&device, &desc).unwrap();
    crate::d3d::copy_texture(&context, &swapchain_image, copies[1]);
    assert_eq!(depth_at_origin(&device, &context, &swapchain_image), 0.75);

    // A pair without depth reads as such; the next with depth has it again.
    until(|| producer.publish((&image, RT), (&image, RT), 4).unwrap().then_some(()));
    let (pair, read) = until(|| reader.read_with_depth(&context, targets, 1).unwrap());
    assert_eq!((pair.sequence, read), (2, None));
    let left = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.5);
    let depth = Depth { eyes: [EyeDepth { texture: &left, state: DEPTH, rect: None }; 2], params };
    until(|| producer.publish_with_depth((&image, RT), (&image, RT), Some(depth), 5).unwrap().then_some(()));
    let (_, read) = until(|| reader.read_with_depth(&context, targets, 2).unwrap());
    assert!(read.is_some());
    assert_eq!(depth_at_origin(&device, &context, reader.depth_textures().unwrap()[1]), 0.5);

    // Depth the channel cannot carry is refused with the pair.
    let bad = Depth { params: DepthParams::new(1.0, 0.5, false), ..depth };
    assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(bad), 6).is_err());
    let outside = Depth { eyes: [EyeDepth { rect: Some([60, 0, 8, 8]), ..depth.eyes[0] }; 2], ..depth };
    assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(outside), 6).is_err());
}

#[test]
fn a_header_writer_refuses_a_depth_block_of_another_major_for_the_run() {
    use crate::shared::{Create, SharedBlock};
    use monaka_contract::protocol::{Block, DEPTH_MAGIC, Stamp};
    let name = ChannelName::new(format!(r"Local\monaka-test-depth-writer-{}", std::process::id())).unwrap();
    let (block, lock) = name.block(Block::Depth);
    let mut other = SharedBlock::<DepthBlock>::create(&block, &lock, Create::OrOpen).unwrap();
    other.lock(0).tolerant().unwrap().stamp = Stamp { magic: DEPTH_MAGIC, major: 99, minor: 0, size: 0, _reserved: 0 };
    let mut writer = HeaderWriter::claim(&name).unwrap();
    assert!(!writer.depth_refused());
    assert!(!writer.new_depth(dxgi_format::R32_TYPELESS, 64, 32, [[0; 2]; FENCE_SLOTS]));
    assert!(writer.depth_refused(), "refused for the rest of the run");
    // Its record is left as the other side made it.
    assert_eq!(other.lock(0).tolerant().unwrap().stamp.major, 99);
}

#[test]
#[ignore = "needs a GPU"]
fn a_depth_block_of_another_major_leaves_the_pairs_without_depth() {
    use crate::shared::{Create, SharedBlock};
    use monaka_contract::protocol::{Block, DEPTH_MAGIC, Stamp};
    let name = ChannelName::new(format!(r"Local\monaka-test-depth-major-{}", std::process::id())).unwrap();
    // Another side made the depth block at a major this side does not read.
    let (block, lock) = name.block(Block::Depth);
    let mut other = SharedBlock::<DepthBlock>::create(&block, &lock, Create::OrOpen).unwrap();
    other.lock(0).tolerant().unwrap().stamp = Stamp { magic: DEPTH_MAGIC, major: 99, minor: 0, size: 0, _reserved: 0 };
    let (device12, queue) = gpu12();
    let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    let described = describe(&name).unwrap();
    let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
    let mut reader = FenceReader::open(&name, &device).unwrap();
    let targets: [ID3D11Texture2D; 2] =
        std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
    let targets = [&targets[0], &targets[1]];
    let image = cleared(&device12, &queue, [1.0, 0.0, 0.0, 1.0]);
    let left = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.25);
    let depth = Depth { eyes: [EyeDepth { texture: &left, state: DEPTH, rect: None }; 2], params: DepthParams::new(0.05, 100.0, false) };
    // The pairs still cross, without their depth, and both sides know depth is off for the run.
    until(|| producer.publish_with_depth((&image, RT), (&image, RT), Some(depth), 3).unwrap().then_some(()));
    let (_, read) = until(|| reader.read_with_depth(&context, targets, 0).unwrap());
    assert_eq!(read, None);
    assert!(reader.depth_refused());
    assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(depth), 4).unwrap());
    drop(other);
}

#[test]
fn a_held_depth_survives_its_source_being_redrawn() {
    let name = ChannelName::new(format!(r"Local\monaka-test-hold-depth-{}", std::process::id())).unwrap();
    let (device12, queue) = gpu12();
    let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    let described = describe(&name).unwrap();
    let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
    let mut reader = FenceReader::open(&name, &device).unwrap();
    let targets: [ID3D11Texture2D; 2] =
        std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
    let image = cleared(&device12, &queue, [0.0, 0.0, 1.0, 1.0]);

    assert!(producer.held_depth().is_none());
    let first = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.125);
    assert!(producer.hold_depth(&first, DEPTH).unwrap());
    let second = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.875);
    let held = producer.held_depth().unwrap();
    let depth = Depth {
        eyes: [EyeDepth { texture: &held, state: D3D12_RESOURCE_STATE_COMMON, rect: None }, EyeDepth { texture: &second, state: DEPTH, rect: None }],
        params: DepthParams::new(0.1, 500.0, false),
    };
    assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(depth), 1).unwrap());
    let (_, read) = until(|| reader.read_with_depth(&context, [&targets[0], &targets[1]], 0).unwrap());
    assert!(read.is_some());
    let copies = reader.depth_textures().unwrap();
    assert_eq!(depth_at_origin(&device, &context, copies[0]), 0.125);
    assert_eq!(depth_at_origin(&device, &context, copies[1]), 0.875);
}

#[test]
fn depth_rects_stay_inside() {
    assert!(rect_inside([0, 0, 64, 32], 64, 32));
    assert!(rect_inside([8, 4, 56, 28], 64, 32));
    assert!(!rect_inside([8, 0, 57, 32], 64, 32), "past the right edge");
    assert!(!rect_inside([0, 0, 0, 32], 64, 32), "empty");
    assert!(!rect_inside([u32::MAX, 0, 2, 2], 64, 32), "overflow");
}

#[test]
fn a_held_eye_survives_its_source_being_redrawn() {
    let name = ChannelName::new(format!(r"Local\monaka-test-hold-{}", std::process::id())).unwrap();
    let (device12, queue) = gpu12();
    let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    let described = describe(&name).unwrap();
    let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
    let mut reader = FenceReader::open(&name, &device).unwrap();
    let targets: [ID3D11Texture2D; 2] =
        std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
    let targets = [&targets[0], &targets[1]];

    assert!(producer.held().is_none());
    let left = cleared(&device12, &queue, [0.0, 0.0, 1.0, 1.0]);
    assert!(producer.hold(&left, RT).unwrap());
    // The same back buffer is drawn over for the other eye.
    let right = cleared(&device12, &queue, [1.0, 1.0, 0.0, 1.0]);
    let held = producer.held().unwrap();
    assert!(producer.publish((&held, D3D12_RESOURCE_STATE_COMMON), (&right, RT), 9).unwrap());
    until(|| reader.read(&context, targets, 0).unwrap());
    assert_eq!(read_pixel(&device, &context, targets[0], 3, 3).unwrap(), [0, 0, 255, 255]);
    assert_eq!(read_pixel(&device, &context, targets[1], 3, 3).unwrap(), [255, 255, 0, 255]);
}
