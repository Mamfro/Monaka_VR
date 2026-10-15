use super::*;
use monaka_ffx::Size2;
use monaka_ffx::upscale::GameResource;
use monaka_channel::d3d12::Recorder;
use monaka_streamline::{TAG_BYTES, constants as at, read_tags};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;

#[test]
fn a_depth_named_last_for_one_frame_is_passed_over_and_a_lasting_one_taken() {
    let (a, b) = (0xa, 0xb);
    // Settled on b; a frame names a last once: b still, a waits.
    assert_eq!(settle(Some(b), None, &[b, a], a), (b, Some(b), Some(a)));
    // The next frame names b last again: b, nothing waits.
    assert_eq!(settle(Some(b), Some(a), &[a, b], b), (b, Some(b), None));
    // a named last a second frame in a row: a from now on.
    assert_eq!(settle(Some(b), Some(a), &[b, a], a), (a, Some(a), None));
    // A frame naming one depth gives it, whatever was settled (a game alternating its one).
    assert_eq!(settle(Some(b), None, &[a], a), (a, Some(a), None));
    // Nothing settled yet, or the settled texture not named: the last.
    assert_eq!(settle(None, None, &[b, a], a), (a, Some(a), None));
    assert_eq!(settle(Some(0xc), None, &[b, a], a), (a, Some(a), None));
}

#[test]
fn the_rect_is_the_render_size_part() {
    assert_eq!(rect((1920, 1080), Some((1920, 1080))), None, "the whole texture");
    assert_eq!(rect((1920, 1080), None), None, "render size not known");
    assert_eq!(rect((2048, 2048), Some((1440, 1600))), Some([0, 0, 1440, 1600]), "a larger depth texture");
    assert_eq!(rect((1440, 1600), Some((2160, 2160))), None, "never past the texture");
    assert_eq!(rect((1440, 1600), Some((0, 1600))), None, "no size");
}

#[test]
fn the_motion_vectors_are_read_as_their_float_member() {
    assert_eq!(float_format(DXGI_FORMAT_R16G16_TYPELESS), Some(DXGI_FORMAT_R16G16_FLOAT));
    assert_eq!(float_format(DXGI_FORMAT_R16G16B16A16_FLOAT), Some(DXGI_FORMAT_R16G16B16A16_FLOAT));
    assert_eq!(float_format(DXGI_FORMAT_R32G32_TYPELESS), Some(DXGI_FORMAT_R32G32_FLOAT));
    assert_eq!(float_format(DXGI_FORMAT_R8G8B8A8_UNORM), None);
}

#[test]
fn the_previous_frame_is_said_once_and_carried_by_the_inputs() {
    assert_eq!(previous(), PreviousFrame::OtherEye, "the game's own previous frame unless said");
    set_previous(PreviousFrame::OwnEye { frames_back: 2 });
    assert_eq!(previous(), PreviousFrame::OwnEye { frames_back: 2 });
    assert_eq!(previous().frames_back(), 2);
    set_previous(PreviousFrame::OwnEye { frames_back: 0 });
    assert_eq!(previous(), PreviousFrame::OwnEye { frames_back: 1 }, "at least one frame back");
    set_previous(PreviousFrame::OtherEye);
    assert_eq!(previous().frames_back(), 1);
}

#[test]
fn the_chain_holds_over_the_other_eyes_frame_of_the_same_size() {
    let size = (1440, 1600);
    assert!(chain_holds(Some(1), 0, size, size));
    assert!(chain_holds(Some(0), 1, size, size));
    assert!(!chain_holds(Some(0), 0, size, size), "the same eye twice: a frame went missing");
    assert!(!chain_holds(None, 0, size, size), "no frame before");
    assert!(!chain_holds(Some(1), 0, (1280, 1600), size), "the size changed");
}

/// The Beast's DLSS view to clip (row vectors): reverse-Z infinite, near 0.05.
const VIEW_TO_CLIP: [f32; 16] = [1.036, 0.0, 0.0, 0.0, 0.0, 1.84, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.05, 0.0];
const PLACE: Place = Place { position: [3.0, 1.7, -8.0], right: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0], forward: [-1.0, 0.0, 0.0] };

fn put(bytes: &mut [u8], offset: usize, values: &[f32]) {
    for (i, v) in values.iter().enumerate() {
        bytes[offset + 4 * i..][..4].copy_from_slice(&v.to_le_bytes());
    }
}

/// The game's constants with the camera's projection and `reset`.
fn constants(reset: bool) -> Vec<u8> {
    let mut bytes = vec![0u8; at::BYTES];
    put(&mut bytes, at::CAMERA_VIEW_TO_CLIP, &VIEW_TO_CLIP);
    put(&mut bytes, at::NEAR, &[0.05]);
    bytes[at::RESET] = reset as u8;
    bytes
}

/// The same frame as DLSS's constants and as FSR's dispatch (2560x1440, vectors in pixels).
fn both_sources() -> (Constants, GameDispatch) {
    let (width, height) = (2560u32, 1440u32);
    let mut bytes = constants(false);
    put(&mut bytes, at::POSITION, &PLACE.position);
    put(&mut bytes, at::UP, &PLACE.up);
    put(&mut bytes, at::RIGHT, &PLACE.right);
    put(&mut bytes, at::FORWARD, &PLACE.forward);
    put(&mut bytes, at::JITTER, &[0.25, -0.125]);
    put(&mut bytes, at::MVEC_SCALE, &[1.0 / width as f32, 1.0 / height as f32]);
    put(&mut bytes, at::FAR, &[4000.0]);
    put(&mut bytes, at::FOV, &[2.0 * (1.0f32 / 1.84).atan()]);
    let resource = GameResource { resource: 0, format: 0, width, height, state: 0 };
    let dispatch = GameDispatch {
        command_list: 0,
        color: resource,
        depth: resource,
        motion_vectors: resource,
        reactive: resource,
        transparency_and_composition: resource,
        output: resource,
        jitter: Float2 { x: 0.25, y: -0.125 },
        motion_vector_scale: Float2 { x: 1.0, y: 1.0 },
        render_size: Size2 { width, height },
        upscale_size: Size2 { width: 3840, height: 2160 },
        sharpening: None,
        frame_time_ms: 16.0,
        pre_exposure: 1.0,
        reset: false,
        near: 0.05,
        far: 4000.0,
        fov_vertical: 2.0 * (1.0f32 / 1.84).atan(),
        view_space_to_meters: 1.0,
        flags: 0,
    };
    (Constants::read(bytes.as_ptr()).unwrap(), dispatch)
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 1e-5 * a.abs().max(1.0))
}

#[test]
fn fsrs_dispatch_tells_the_features_what_dlss_constants_do() {
    let (constants, dispatch) = both_sources();
    let dlss = Rendering::from_constants(&constants);
    let frustum = Frustum::centred([1.0 / 1.036, 1.0 / 1.84]).unwrap();
    let flags = create::DEPTH_INVERTED | create::DEPTH_INFINITE;
    let fsr = Rendering::from_dispatch(&dispatch, PLACE, &frustum, flags).unwrap();
    assert!(close(&fsr.view_to_clip, &dlss.view_to_clip), "{:?} {:?}", fsr.view_to_clip, dlss.view_to_clip);
    let (a, b) = (fsr.camera, dlss.camera);
    assert!(close(&[a.motion_vector_scale.x, a.motion_vector_scale.y], &[b.motion_vector_scale.x, b.motion_vector_scale.y]), "to the screen");
    assert_eq!((a.jitter, a.near, a.far, a.position, a.right, a.up, a.forward), (b.jitter, b.near, b.far, b.position, b.right, b.up, b.forward));
    assert!((a.fov_vertical - b.fov_vertical).abs() < 1e-6);
    let size = Size2 { width: 2560, height: 1440 };
    let (a, b) = (a.for_render(size), b.for_render(size));
    assert!(close(&[a.motion_vector_scale.x, a.motion_vector_scale.y], &[1.0, 1.0]) && close(&[b.motion_vector_scale.x, b.motion_vector_scale.y], &[1.0, 1.0]), "pixels for frame generation");

    // The other depth conventions: depth 1 at the near plane when reversed, 0 when not; the far
    // plane at the other end unless infinite.
    let depth = |p: &[f32; 16], distance: f32| {
        // Row vectors: a point `distance` ahead (view space looks down -z).
        let (z, w) = (-distance * p[10] + p[14], -distance * p[11] + p[15]);
        z / w
    };
    for (flags, near, far) in [(create::DEPTH_INVERTED, 1.0, 0.0), (0, 0.0, 1.0), (create::DEPTH_INFINITE, 0.0, 1.0)] {
        let r = Rendering::from_dispatch(&dispatch, PLACE, &frustum, flags).unwrap();
        assert!((depth(&r.view_to_clip, 0.05) - near).abs() < 1e-5, "flags {flags}: near");
        let at_far = if flags & create::DEPTH_INFINITE != 0 { 1e9 } else { 4000.0 };
        assert!((depth(&r.view_to_clip, at_far) - far).abs() < 1e-4, "flags {flags}: far");
    }
    // No far plane given: infinite. No render size or near plane: nothing to go on.
    let endless = Rendering::from_dispatch(&GameDispatch { far: f32::INFINITY, ..dispatch }, PLACE, &frustum, create::DEPTH_INVERTED).unwrap();
    assert!(close(&endless.view_to_clip, &dlss.view_to_clip));
    assert!(Rendering::from_dispatch(&GameDispatch { render_size: Size2::default(), ..dispatch }, PLACE, &frustum, flags).is_none());
    assert!(Rendering::from_dispatch(&GameDispatch { near: 0.0, ..dispatch }, PLACE, &frustum, flags).is_none());
}

fn device() -> ID3D12Device {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: creates a device on the default adapter.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    device.unwrap()
}

/// The game's tag array for `textures` ((buffer type, texture)), with the `sl::Resource`s it
/// points at (kept alive beside it).
fn tag_array(textures: &[(u32, &ID3D12Resource)]) -> (Vec<u8>, Vec<[u8; 128]>) {
    let resources: Vec<[u8; 128]> = textures
        .iter()
        .map(|(_, texture)| {
            let mut resource = [0u8; 128];
            resource[32] = RESOURCE_TEX2D;
            resource[40..48].copy_from_slice(&(texture.as_raw() as usize).to_le_bytes());
            resource
        })
        .collect();
    let mut tags = vec![0u8; textures.len() * TAG_BYTES];
    for (i, (kind, _)) in textures.iter().enumerate() {
        tags[i * TAG_BYTES + 32..][..8].copy_from_slice(&(resources[i].as_ptr() as usize).to_le_bytes());
        tags[i * TAG_BYTES + 40..][..4].copy_from_slice(&kind.to_le_bytes());
    }
    (tags, resources)
}

/// The consumers' asks for the steps that follow.
fn ask(motion: Motion, accept: Option<fn(&Rendering) -> bool>) {
    disarm();
    want(motion, accept);
    arm();
}

/// The store is the run's: one test walks it.
#[test]
fn inputs_come_fresh_until_their_eyes_present_takes_them() {
    clear();
    let device = device();
    // SAFETY: creates a direct queue on a live device.
    let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }) }.unwrap();
    let none = D3D12_RESOURCE_FLAG_NONE;
    let depth = d3d12::texture(&device, 16, 8, DXGI_FORMAT_R32_FLOAT, none).unwrap();
    let motion = d3d12::texture(&device, 16, 8, DXGI_FORMAT_R16G16_FLOAT, none).unwrap();
    let colour = d3d12::texture(&device, 12, 6, DXGI_FORMAT_R8G8B8A8_UNORM, none).unwrap();
    let mut recorder = Recorder::new(&device, 1).unwrap();
    let list = recorder.begin().unwrap().unwrap();
    let common = D3D12_RESOURCE_STATE_COMMON;

    // Known at each call: each input fresh once tagged, both taken at the present.
    tagged(&list, 0, Input::Depth, &depth, common, false);
    assert_eq!(fresh(0), Fresh { depth: true, motion: false });
    tagged(&list, 0, Input::MotionVectors, &motion, common, false);
    let (inputs, _) = take_late(0).unwrap();
    assert!(inputs.fresh.both() && inputs.depth.is_some() && inputs.motion.is_some());
    assert_eq!(inputs.motion_format, DXGI_FORMAT_R16G16_FLOAT, "in their own format");
    assert_eq!(fresh(0), Fresh::default(), "taken");
    // Late: kept at the tag, fresh, copied at the present into the same copies.
    tagged(&list, 0, Input::Depth, &depth, common, true);
    tagged(&list, 0, Input::MotionVectors, &motion, common, true);
    let (late_inputs, late) = take_late(0).unwrap();
    assert!(late_inputs.fresh.both());
    assert!(late.depth.as_ref().is_some_and(|(.., target)| Some(target) == inputs.depth.as_ref()));
    late.record(&list);
    assert!(take_late(0).unwrap().1.depth.is_none(), "copied once");
    assert!(take(1).is_none(), "the other eye has none");

    // Labelled at the constants, copied at the evaluate.
    let (tags, _resources) = tag_array(&[(buffer::DEPTH, &depth), (buffer::MOTION_VECTORS, &motion), (buffer::SCALING_INPUT_COLOR, &colour)]);
    let tags = read_tags(tags.as_ptr(), 3);
    // The render size: the input colour's, else the depth's.
    sizes_tagged(&tags);
    assert_eq!(render_sizes(), Some(((12, 6), None)));
    let (depth_only, _kept) = tag_array(&[(buffer::DEPTH, &depth)]);
    sizes_tagged(&read_tags(depth_only.as_ptr(), 1));
    assert_eq!(render_sizes(), Some(((16, 8), None)));
    let still = constants(false);
    let frame = |eye: Option<usize>| {
        at_constants(still.as_ptr(), eye);
        at_tags(&tags);
        at_evaluate(Some(list.clone()));
    };
    let copied = copies()[1];
    ask(Motion::Float, None);
    frame(Some(1));
    let inputs = peek(1).unwrap();
    assert_eq!(inputs.fresh, Fresh { depth: true, motion: true });
    assert_eq!((inputs.motion_format, inputs.rect, inputs.reset), (DXGI_FORMAT_R16G16_FLOAT, Some([0, 0, 12, 6]), false));
    assert!(inputs.camera().is_some_and(|c| c.near == 0.05));
    assert_eq!(inputs.rendering.map(|r| r.view_to_clip), Some(VIEW_TO_CLIP));
    assert_eq!(copies()[1], copied + 1);
    assert!(take(1).unwrap().fresh.both());
    assert!(!take(1).unwrap().fresh.depth, "taken once");
    // A frame not labelled, or refused by any consumer, leaves the eye's inputs as they were.
    frame(None);
    want(Motion::None, Some(|_| true));
    want(Motion::None, Some(|_| false));
    frame(Some(1));
    assert_eq!(fresh(1), Fresh::default());
    // The evaluate takes the labelled frame once.
    ask(Motion::Float, None);
    at_evaluate(Some(list.clone()));
    assert_eq!(fresh(1), Fresh::default());
    // Without the motion vectors: the depth alone.
    ask(Motion::None, Some(|_| true));
    frame(Some(1));
    let inputs = take(1).unwrap();
    assert_eq!(inputs.fresh, Fresh { depth: true, motion: false });
    assert!(inputs.motion.is_none());
    // The game's reset.
    let cut = constants(true);
    at_constants(cut.as_ptr(), Some(0));
    at_tags(&tags);
    at_evaluate(Some(list.clone()));
    assert!(take(0).unwrap().reset);

    // Chained (the most asked wins): the chain holds over the other eye's frame just before.
    ask(Motion::Float, None);
    want(Motion::Chained, None);
    want(Motion::None, None);
    assert!(collecting());
    frame(Some(0));
    assert!(take(0).unwrap().reset, "no frame before");
    frame(Some(1));
    let inputs = take(1).unwrap();
    assert!(!inputs.reset && inputs.fresh.both(), "the left eye's frame before");
    assert_eq!(inputs.motion_format, DXGI_FORMAT_R16G16B16A16_FLOAT);
    frame(Some(1));
    assert!(take(1).unwrap().reset, "the same eye twice");
    frame(Some(0));
    assert!(!take(0).unwrap().reset);
    // An evaluate without a labelled frame, or a frame without its tags, breaks the chain.
    at_evaluate(Some(list.clone()));
    frame(Some(1));
    assert!(take(1).unwrap().reset, "an evaluate with no labelled frame");
    frame(Some(0));
    assert!(!take(0).unwrap().reset, "it holds again");
    // A frame rendered against its eye's own previous frame: its vectors copied as they are, in
    // their own format, and the chain does not run over it.
    set_previous(PreviousFrame::OwnEye { frames_back: 2 });
    frame(Some(1));
    let own = take(1).unwrap();
    assert_eq!((own.reset, own.previous, own.motion_format), (false, PreviousFrame::OwnEye { frames_back: 2 }, DXGI_FORMAT_R16G16_FLOAT));
    set_previous(PreviousFrame::OtherEye);
    frame(Some(0));
    let after = take(0).unwrap();
    assert!(after.reset && after.previous == PreviousFrame::OtherEye, "no chain over the own-eye frame");
    frame(Some(1));
    frame(Some(0));
    assert!(!take(0).unwrap().reset, "it holds again");
    at_constants(still.as_ptr(), Some(1));
    at_evaluate(Some(list.clone()));
    frame(Some(0));
    assert!(take(0).unwrap().reset, "a frame without its tags");
    // No native list breaks it too.
    frame(Some(0));
    at_constants(still.as_ptr(), Some(1));
    at_tags(&tags);
    at_evaluate(None);
    frame(Some(1));
    assert!(take(1).unwrap().reset, "no native list");

    // FSR's dispatch: the same store, the same chain (the vectors scaled to render pixels by
    // the game's scale), whichever source gave the frame before.
    let (_, dispatch) = both_sources();
    let frustum = Frustum::centred([1.0 / 1.036, 1.0 / 1.84]).unwrap();
    let rendering = Rendering::from_dispatch(&dispatch, PLACE, &frustum, create::DEPTH_INVERTED | create::DEPTH_INFINITE).unwrap();
    let fsr = |eye: usize, reset: bool| Upscaled { eye, rendering, reset, depth: (&depth, common), motion: (&motion, common), render: Some((12, 6)), chain_scale: [16.0, 8.0] };
    upscaled(&list, Some(fsr(0, false)));
    let inputs = take(0).unwrap();
    assert!(!inputs.reset && inputs.fresh.both(), "chained over the DLSS frame just before");
    assert_eq!((inputs.rendering, inputs.rect), (Some(rendering), Some([0, 0, 12, 6])));
    upscaled(&list, Some(fsr(1, true)));
    assert!(take(1).unwrap().reset, "the game's reset");
    upscaled(&list, None);
    upscaled(&list, Some(fsr(0, false)));
    assert!(take(0).unwrap().reset, "a dispatch of no eye breaks the chain");
    upscaled(&list, Some(fsr(1, false)));
    assert!(!take(1).unwrap().reset);
    ask(Motion::Float, None);
    upscaled(&list, Some(fsr(0, false)));
    let inputs = take(0).unwrap();
    assert_eq!((inputs.fresh, inputs.motion_format), (Fresh { depth: true, motion: true }, DXGI_FORMAT_R16G16_FLOAT));
    ask(Motion::None, Some(|r| r.camera.near > 1.0));
    upscaled(&list, Some(fsr(0, false)));
    assert_eq!(fresh(0), Fresh::default(), "refused");
    sizes_known((2560, 1440), Some((3840, 2160)));
    assert_eq!(render_sizes(), Some(((2560, 1440), Some((3840, 2160)))));
    sizes_known((0, 1440), None);
    assert_eq!(render_sizes(), Some(((2560, 1440), Some((3840, 2160)))), "no size");

    recorder.submit(&queue).unwrap();
    assert!(recorder.idle(2000));
    disarm();
    assert!(!collecting());
    clear();
    assert!(take(0).is_none() && take(1).is_none());
}
