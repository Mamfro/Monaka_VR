#[test]
#[ignore = "needs a GPU"]
fn a_reader_keeps_up_with_a_steady_producer() {
    use crate::d3d::create_device;
    use crate::fence::FenceReader;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    let name = ChannelName::new(&format!(r"Local\monaka-test-fence11-rate-{}", std::process::id())).unwrap();
    let (width, height) = (1920, 1080);
    let (device, context, _) = create_device(None).unwrap();
    let source = d3d::create_texture(&device, &d3d::simple_texture_desc(width, height, DXGI_FORMAT_B8G8R8A8_UNORM)).unwrap();
    let mut producer = FenceProducer11::create(&name, &device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
    let whole = D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
    // The first pair makes the channel readable.
    assert!(producer.publish(&context, &source, [whole, whole], 0).unwrap());
    let reader_name = name.clone();
    let reader = std::thread::spawn(move || {
        let (device, context, _) = create_device(None).unwrap();
        let mut reader = FenceReader::open(&reader_name, &device).unwrap();
        let targets = [0, 1].map(|_| d3d::create_texture(&device, &d3d::simple_texture_desc(width, height, DXGI_FORMAT_B8G8R8A8_UNORM)).unwrap());
        let (mut seen, mut reads, mut busy) = (0, 0, 0);
        let deadline = Instant::now() + Duration::from_millis(2500);
        while Instant::now() < deadline {
            match reader.read(&context, [&targets[0], &targets[1]], seen).unwrap() {
                Some(pair) => {
                    seen = pair.sequence;
                    reads += 1;
                }
                None => busy += 1,
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        (reads, seen, busy)
    });
    let mut published = 1;
    for i in 0..200 {
        if producer.publish(&context, &source, [whole, whole], i).unwrap() {
            published += 1;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (reads, last, polls_without) = reader.join().unwrap();
    println!("published {published}, read {reads}, last sequence read {last}, polls with nothing new {polls_without}");
    assert!(reads * 10 >= published * 8, "the reader read {reads} of {published}");
}

use super::*;
use crate::d3d::{create_device, create_texture_with, read_pixel};
use crate::fence::FenceReader;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

#[test]
#[ignore = "needs a GPU"]
fn a_d3d11_producer_reaches_the_d3d11_reader() {
    let (device, context, _) = create_device(None).unwrap();
    // A 64x32 side-by-side image: red on the left, blue on the right.
    let pixels: Vec<u8> = (0..32).flat_map(|_| (0..64).flat_map(|x| if x < 32 { [255, 0, 0, 255] } else { [0, 0, 255, 255] })).collect();
    let source = create_texture_with(&device, &d3d::simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM), &pixels).unwrap();
    let name = ChannelName::new(&format!(r"Local\monaka-test-fence11-{}", std::process::id())).unwrap();
    let mut producer = FenceProducer11::create(&name, &device, 32, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    let eye = |x: u32| D3D11_BOX { left: x, top: 0, front: 0, right: x + 32, bottom: 32, back: 1 };
    assert!(producer.publish(&context, &source, [eye(0), eye(32)], 7).unwrap());

    let (reader_device, reader_context, _) = create_device(None).unwrap();
    let mut reader = FenceReader::open(&name, &reader_device).unwrap();
    let targets = [0, 1].map(|_| d3d::create_texture(&reader_device, &d3d::simple_texture_desc(32, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    let pair = loop {
        if let Some(pair) = reader.read(&reader_context, [&targets[0], &targets[1]], 0).unwrap() {
            break pair;
        }
        assert!(Instant::now() < deadline, "no pair");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!((pair.sequence, pair.tick), (1, 7));
    assert_eq!(read_pixel(&reader_device, &reader_context, &targets[0], 5, 5).unwrap(), [255, 0, 0, 255]);
    assert_eq!(read_pixel(&reader_device, &reader_context, &targets[1], 5, 5).unwrap(), [0, 0, 255, 255]);
}

/// A `width` x `height` R32_FLOAT texture filled with `value`, as a game's depth copy would be.
fn float_texture(device: &ID3D11Device, width: u32, height: u32, value: f32) -> ID3D11Texture2D {
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_FLOAT;
    let texels: Vec<u8> = (0..width * height).flat_map(|_| value.to_le_bytes()).collect();
    create_texture_with(device, &d3d::simple_texture_desc(width, height, DXGI_FORMAT_R32_FLOAT), &texels).unwrap()
}

fn float_at_origin(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D) -> f32 {
    let (_, _, texel, texels) = d3d::read_texels(device, context, texture).unwrap();
    assert_eq!(texel, 4);
    f32::from_le_bytes(texels[..4].try_into().unwrap())
}

#[test]
#[ignore = "needs a GPU"]
fn a_d3d11_producer_sends_depth_beside_the_eyes() {
    use monaka_core::protocol::DepthParams;
    let (device, context, _) = create_device(None).unwrap();
    let image = d3d::create_texture(&device, &d3d::simple_texture_desc(32, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap();
    let (left, right) = (float_texture(&device, 16, 8, 0.25), float_texture(&device, 16, 8, 0.75));
    assert!(depth_copyable(&left) && !depth_copyable(&image));
    let name = ChannelName::new(&format!(r"Local\monaka-test-fence11-depth-{}", std::process::id())).unwrap();
    let mut producer = FenceProducer11::create(&name, &device, 32, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    let depth = Depth { eyes: [EyeDepth { texture: &left, rect: None }, EyeDepth { texture: &right, rect: Some([0, 0, 8, 8]) }], params: DepthParams::new(0.1, 100.0, true) };
    assert!(producer.publish_whole_with_depth(&context, [&image, &image], Some(depth), 5).unwrap());

    let (reader_device, reader_context, _) = create_device(None).unwrap();
    let mut reader = FenceReader::open(&name, &reader_device).unwrap();
    let targets = [0, 1].map(|_| d3d::create_texture(&reader_device, &d3d::simple_texture_desc(32, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    let (pair, read) = loop {
        if let Some(read) = reader.read_with_depth(&reader_context, [&targets[0], &targets[1]], 0).unwrap() {
            break read;
        }
        assert!(Instant::now() < deadline, "no pair");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!((pair.sequence, pair.tick), (1, 5));
    let read = read.expect("the pair came with depth");
    assert_eq!((read.width, read.height, read.rects), (16, 8, [[0, 0, 16, 8], [0, 0, 8, 8]]));
    let copies = reader.depth_textures().unwrap();
    assert_eq!(float_at_origin(&reader_device, &reader_context, copies[0]), 0.25);
    assert_eq!(float_at_origin(&reader_device, &reader_context, copies[1]), 0.75);
    assert_eq!(producer.pairs_with_depth(), 1);
}

#[test]
#[ignore = "needs a GPU"]
fn a_lazy_producer_keeps_its_first_frame_s_size_and_family() {
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};
    let (device, _, _) = create_device(None).unwrap();
    let name = ChannelName::new(&format!(r"Local\monaka-test-fence11-lazy-{}", std::process::id())).unwrap();
    let mut lazy = LazyProducer11::new(name);
    let made = lazy.get(&device, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);
    assert!(made.producer.is_some() && made.note.is_some_and(|n| n.contains("created at 64x32")));
    assert!(lazy.get(&device, 64, 32, DXGI_FORMAT_R8G8B8A8_TYPELESS).producer.is_some(), "the same family");
    let refused = lazy.get(&device, 64, 32, DXGI_FORMAT_B8G8R8A8_UNORM);
    assert!(refused.producer.is_none() && refused.note.is_some());
    assert!(lazy.get(&device, 64, 32, DXGI_FORMAT_B8G8R8A8_UNORM).note.is_none(), "noted once");
    assert!(lazy.get(&device, 32, 32, DXGI_FORMAT_R8G8B8A8_UNORM).producer.is_none());
    assert_eq!(lazy.size(), Some((64, 32)));
}
