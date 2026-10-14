use super::*;
use crate::ChannelName;
use crate::d3d::{create_device, create_texture_with, read_texels, simple_texture_desc};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_FLOAT;

fn depth_of(device: &ID3D11Device, value: f32) -> ID3D11Texture2D {
    let desc = simple_texture_desc(8, 4, DXGI_FORMAT_R32_FLOAT);
    create_texture_with(device, &desc, &value.to_le_bytes().repeat(32)).unwrap()
}

#[test]
fn depth_crosses_between_devices_and_the_latest_wins() {
    let name = ChannelName::new(format!(r"Local\monaka-test-debug-depth-{}", std::process::id())).unwrap();
    let (game, game_context, _) = create_device(None).unwrap();
    let (viewer, viewer_context, _) = create_device(None).unwrap();
    let mut writer = ImageWriter::create(&name.debug_depth(), &game, 8, 4, DXGI_FORMAT_R32_FLOAT).unwrap();
    let mut reader = ImageReader::open(&name.debug_depth(), &viewer).unwrap();
    assert_eq!((reader.width, reader.height), (8, 4));
    let into = depth_of(&viewer, 0.0);
    assert!(!reader.read(&viewer_context, &into), "nothing written yet");
    assert!(writer.write(&game_context, &depth_of(&game, 0.25)));
    // SAFETY: flushes our own context.
    unsafe { game_context.Flush() };
    // Unread, then written again: the reader gets the second.
    assert!(writer.write(&game_context, &depth_of(&game, 0.75)));
    // SAFETY: as above.
    unsafe { game_context.Flush() };
    assert!(reader.read(&viewer_context, &into));
    let (_, _, _, texels) = read_texels(&viewer, &viewer_context, &into).unwrap();
    assert_eq!(f32::from_le_bytes(texels[..4].try_into().unwrap()), 0.75);
    assert!(!reader.read(&viewer_context, &into), "read once");
}
