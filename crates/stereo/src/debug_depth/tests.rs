use super::*;
use monaka_channel::d3d::{create_device, create_texture, read_texels, simple_texture_desc};
use monaka_channel::debug_image::ImageReader;

#[test]
fn the_clear_is_slot_53_and_the_bind_with_views_34() {
    assert_eq!(std::mem::offset_of!(ID3D11DeviceContext_Vtbl, ClearDepthStencilView) / size_of::<usize>(), CLEAR_DEPTH_SLOT);
    assert_eq!(std::mem::offset_of!(ID3D11DeviceContext_Vtbl, OMSetRenderTargetsAndUnorderedAccessViews) / size_of::<usize>(), BIND_WITH_VIEWS_SLOT);
}

#[test]
fn a_d24_depth_is_shared_as_floats() {
    let name = ChannelName::new(format!(r"Local\monaka-test-share-depth-{}", std::process::id())).unwrap();
    let (device, context, _) = create_device(None).unwrap();
    // The game's depth: D24, cleared to 0.5.
    let mut desc = simple_texture_desc(16, 8, DXGI_FORMAT_D24_UNORM_S8_UINT);
    desc.BindFlags = D3D11_BIND_DEPTH_STENCIL.0 as u32;
    let depth = create_texture(&device, &desc).unwrap();
    let mut view = None;
    // SAFETY: a depth view of our own depth texture, cleared on our own context.
    unsafe {
        device.CreateDepthStencilView(&depth, None, Some(&mut view)).unwrap();
        context.ClearDepthStencilView(view.as_ref().unwrap(), D3D11_CLEAR_DEPTH.0, 0.5, 0);
    }
    let mut made = make(&device, &name, &desc).unwrap();
    // SAFETY: our own resources on our own immediate context.
    unsafe {
        context.CopyResource(&made.copy, &depth);
        dispatch(&context, &made.shader, &made.read, &made.write, made.size);
    }
    assert!(made.writer.write(&context, &made.floats));
    // SAFETY: flushes our own context.
    unsafe { context.Flush() };
    let mut reader = ImageReader::open(&name.debug_depth(), &device).unwrap();
    let into = create_texture(&device, &simple_texture_desc(16, 8, DXGI_FORMAT_R32_FLOAT)).unwrap();
    assert!(reader.read(&context, &into));
    let (_, _, _, texels) = read_texels(&device, &context, &into).unwrap();
    let value = f32::from_le_bytes(texels[..4].try_into().unwrap());
    assert!((value - 0.5).abs() < 1e-3, "{value}");
}

#[test]
fn square_and_colour_formats_are_not_depth() {
    assert!(formats(DXGI_FORMAT_R8G8B8A8_UNORM).is_none());
    assert_eq!(formats(DXGI_FORMAT_D32_FLOAT).map(|f| f.1), Some(DXGI_FORMAT_R32_FLOAT));
}

#[test]
fn the_depth_most_draws_use_is_the_scene_s() {
    let (device, _, _) = create_device(None).unwrap();
    let depth = |w: u32, h: u32| {
        let mut desc = simple_texture_desc(w, h, DXGI_FORMAT_D24_UNORM_S8_UINT);
        desc.BindFlags = D3D11_BIND_DEPTH_STENCIL.0 as u32;
        create_texture(&device, &desc).unwrap()
    };
    let (gui, scene) = (depth(64, 32), depth(32, 16));
    let name = ChannelName::new(r"Local\monaka-test-most-drawn").unwrap();
    let half = depth(16, 8);
    let mut state = State { channel: name, view: (64, 32), scene: Some((gui.clone(), 64 * 32, Some(false))), made: None, generation: 0, kinds: Vec::new(), bound: Vec::new(), drawn: Vec::new(), most_drawn: None, cleared: Vec::new(), frames: 0, tabled: 0 };
    let drawn = |texture: &ID3D11Texture2D, draws: u64| Drawn { texture: texture.clone(), desc: monaka_channel::d3d::texture_desc(texture), draws, average: draws as f32 * 10.0 };
    state.drawn.push(drawn(&gui, 12));
    state.drawn.push(drawn(&scene, 900));
    state.drawn.push(drawn(&half, 1500));
    most_drawn(&mut state);
    assert!(state.scene.as_ref().is_some_and(|(t, _, _)| t == &scene), "the busier depth, though never cleared, over a quarter-size one with more draws");
    assert!(state.scene.as_ref().is_some_and(|(_, _, reversed)| reversed.is_none()), "never cleared: not known which way it runs");
    assert!(state.drawn.iter().all(|d| d.draws == 0), "counts start over each frame");
    state.let_go();
    assert!(state.scene.is_none() && state.drawn.is_empty() && state.bound.is_empty(), "nothing of the game's held");
}

#[test]
fn a_square_depth_is_the_scene_s_only_at_the_picture_s_size() {
    let desc = simple_texture_desc(2644, 2644, DXGI_FORMAT_D32_FLOAT_S8X24_UINT);
    assert!(!usable(&desc, (3840, 2160)), "a shadow map");
    assert!(usable(&desc, (2644, 2644)), "square eyes");
    assert!(usable(&simple_texture_desc(3840, 2160, DXGI_FORMAT_D24_UNORM_S8_UINT), (0, 0)));
}
