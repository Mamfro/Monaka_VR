use super::*;
use crate::providers::EyeGeometry;

#[test]
fn a_scissor_moves_with_its_viewport() {
    let from = Rect::sized(3840.0, 2160.0);
    let wide = monaka_core::camera::Frustum { left: -16.0 / 9.0, right: 16.0 / 9.0, up: 1.0, down: -1.0 };
    let to = HudPlacement::new(wide, 0, 0.0, 2.0).rect(from, from.aspect(), 0.5);
    // A text box right of the centre: halved, around the centre.
    let text = RECT { left: 2000, top: 1100, right: 2400, bottom: 1200 };
    assert_eq!(map_rect(&text, &from, &to), RECT { left: 1960, top: 1090, right: 2160, bottom: 1140 });
    let whole = RECT { left: 0, top: 0, right: 3840, bottom: 2160 };
    assert_eq!(map_rect(&whole, &from, &to), RECT { left: 960, top: 540, right: 2880, bottom: 1620 });
}

#[test]
fn a_layout_keeps_its_place_in_the_placed_viewport() {
    let viewport = |x: f32, y: f32, w: f32, h: f32| D3D11_VIEWPORT { TopLeftX: x, TopLeftY: y, Width: w, Height: h, MinDepth: 0.0, MaxDepth: 1.0 };
    let base = viewport(0.0, 0.0, 3840.0, 2160.0);
    let rect = Rect::new(960.0, 540.0, 1920.0, 1080.0);
    // The whole viewport as the layout: the placed rectangle itself.
    let whole = laid_out(base, base, rect);
    assert_eq!((whole.TopLeftX, whole.TopLeftY, whole.Width, whole.Height), (960.0, 540.0, 1920.0, 1080.0));
    assert_eq!(whole.MaxDepth, 1.0, "depth range kept");
    // A 16:9 band of a 4:3 viewport: halved with it, at its place in it.
    let band = laid_out(viewport(0.0, 0.0, 1440.0, 1080.0), viewport(0.0, 135.0, 1440.0, 810.0), Rect::new(360.0, 270.0, 720.0, 540.0));
    assert_eq!((band.TopLeftX, band.TopLeftY, band.Width, band.Height), (360.0, 337.5, 720.0, 405.0));
}

/// A game whose frames are labelled from the pose alone: each eye drawn with the pose's field
/// of view (none for a pose not tracked), the eyes the pose's eye distance apart, the frame
/// being drawn the right eye of a turned head.
struct Labelled;

impl monaka_producer::game::Game for Labelled {
    const MANIFESTS: &'static [monaka_contract::manifest::Manifest] = &[];
    const ADAPTER: monaka_producer::game::Adapter<Self> = monaka_producer::game::Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_contract::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), Rejection> {
        Ok(())
    }
}

impl FrameLabels for Labelled {
    fn labelled() -> Option<crate::driver::Pending> {
        Some(crate::driver::Pending { eye: 1, pose: turned() })
    }
    fn presenting() -> Option<crate::driver::Pending> {
        None
    }
    fn running() -> bool {
        true
    }
}

impl EyeGeometry for Labelled {
    fn frustum(pose: &HeadPose, eye: usize) -> Option<monaka_core::camera::Frustum> {
        (pose.valid != 0).then(|| monaka_core::camera::Frustum::from_fov(pose.fov[eye])).flatten()
    }
    fn separation(pose: &HeadPose) -> Option<f32> {
        Some(pose.ipd)
    }
}

fn turned() -> HeadPose {
    HeadPose::synthetic(30.0, -5.0, [[-0.9, 0.75, 0.82, -0.88], [-0.75, 0.9, 0.82, -0.88]], 0.066)
}

/// The HUD goes where the frame labels' frustum and separation put it, in each eye, at the
/// HUD's distance and size; the whole image without a place.
#[test]
fn the_hud_is_placed_by_the_frame_labels() {
    let poses = [turned(), HeadPose::synthetic(0.0, 0.0, HeadPose::SYNTHETIC_FOV, 0.064), HeadPose::default()];
    let (scale, distance) = (0.4, 1.5);
    for pose in poses {
        for (width, height) in [(1920, 1080), (2560, 1440), (1440, 1600)] {
            let base = Rect::sized(width as f32, height as f32);
            let placed = rects::<Labelled>(Some((scale, distance)), &pose, width, height);
            for eye in [0, 1] {
                let at = Labelled::frustum(&pose, eye).map(|frustum| HudPlacement::new(frustum, eye, pose.ipd, distance));
                assert_eq!(crate::hud_layer::place_eye::<Labelled>(&pose, eye, distance), at);
                let expected = at.map_or(base.array(), |at| at.rect(base, base.aspect(), scale).array());
                assert_eq!(placed[eye], expected, "eye {eye}, {width}x{height}");
            }
            assert_eq!(rects::<Labelled>(None, &pose, width, height), [base.array(); 2], "not placed: the whole image");
        }
    }
    // The frame being drawn: its label's eye and pose.
    assert_eq!(placement::<Labelled>(distance), crate::hud_layer::place_eye::<Labelled>(&turned(), 1, distance));
    assert!(placement::<Labelled>(distance).is_some());
    // Not installed: the whole image.
    assert_eq!(eye_rects::<Labelled>(&turned(), 800, 600), [Rect::sized(800.0, 600.0).array(); 2]);
}
