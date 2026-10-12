//! AFW on D3D11 (alternate-eye rendering with depth): every presented frame is one real eye, and
//! the other eye of the same moment is that frame reprojected with its depth
//! (`monaka_warp::Reprojector`). What the
//! real eye could not see is filled from the other eye's own previous real frame, which saw it from
//! the right place, so the gaps get rendered pixels instead of stretched ones. A pair goes out
//! every frame (alternate-eye alone publishes every second frame, a frame apart).
//!
//! The game crate captures each frame's world image (before the HUD) and depth with the cameras
//! it was rendered from ([`Afw11::capture`]), and publishes at present ([`Afw11::publish`]).

pub mod ready;

use crate::alternate::Step;
use crate::publish::{Tally, copy_into, record_first};
use monaka_channel::keyed::LazyWriter;
use monaka_channel::pose::HeadSource;
use monaka_channel::{ChannelName, d3d};
use monaka_core::camera::{Mat34, Mat44};
use monaka_core::depth::DepthMapping;
use monaka_core::protocol::HeadPose;
use monaka_producer::log;
use monaka_warp::{Frame, HudLayer, Reprojector, View};
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_TYPELESS;

/// Both eyes' cameras at one moment (canonical), and the head pose they come from.
#[derive(Clone, Copy, Debug)]
pub struct EyeCameras {
    pub camera: [Mat34; 2],
    pub projection: [Mat44; 2],
    pub mapping: [DepthMapping; 2],
    pub pose: HeadPose,
}

/// One eye's latest real frame.
struct Real {
    frame: Frame,
    cameras: EyeCameras,
}

pub struct Afw11 {
    reprojector: Reprojector,
    real: [Option<Real>; 2],
    /// The eye captured last (this frame's real eye).
    latest: Option<usize>,
    writer: LazyWriter,
    shown: Option<ID3D11Texture2D>,
    /// The HUD as its own layer, laid over both eyes; begun by this frame's first HUD draw.
    hud: Option<HudLayer>,
    hud_begun: bool,
    hud_views: Option<[ID3D11RenderTargetView; 2]>,
    tally: Tally,
}

impl Afw11 {
    pub fn new(device: &ID3D11Device, channel: ChannelName) -> Result<Self, monaka_warp::Error> {
        Ok(Self {
            reprojector: Reprojector::new(device)?,
            real: [None, None],
            latest: None,
            writer: LazyWriter::new(channel),
            shown: None,
            hud: None,
            hud_begun: false,
            hud_views: None,
            tally: Tally::NEW,
        })
    }

    /// Takes the HUD as a layer laid over both eyes ([`Afw11::hud_targets`]) instead of leaving
    /// it in the real eye's image only.
    pub fn enable_hud(&mut self, device: &ID3D11Device) -> Result<(), monaka_warp::Error> {
        self.hud = Some(HudLayer::new(device)?);
        Ok(())
    }

    /// The black and white targets this frame's HUD draws go into (each draw into both), begun
    /// and cleared at the frame's first HUD draw. `None` without a HUD layer.
    pub fn hud_targets(&mut self, context: &ID3D11DeviceContext, width: u32, height: u32) -> Option<[ID3D11RenderTargetView; 2]> {
        if self.hud_begun
            && let Some(views) = &self.hud_views
        {
            return Some(views.clone());
        }
        let layer = self.hud.as_mut()?;
        match layer.begin(context, width, height) {
            Ok(targets) => {
                self.hud_begun = true;
                self.hud_views = Some(targets.clone());
                Some(targets)
            }
            Err(e) => {
                if self.tally.failures == 0 {
                    log!("HUD layer unavailable: {e}");
                }
                self.tally.failures += 1;
                None
            }
        }
    }

    pub fn pairs(&self) -> u64 {
        self.tally.pairs
    }

    pub fn failures(&self) -> u64 {
        self.tally.failures
    }

    /// The frame for eye `eye` is complete before its HUD: keep its world image and depth with
    /// the cameras it was rendered from as that eye's latest real frame.
    pub fn capture(&mut self, context: &ID3D11DeviceContext, eye: usize, color: &ID3D11Texture2D, depth: &ID3D11Texture2D, cameras: &EyeCameras) {
        let reuse = self.real[eye].take().map(|r| r.frame);
        match self.reprojector.capture(context, color, depth, cameras.projection[eye], cameras.camera[eye], reuse) {
            Ok(frame) => {
                self.real[eye] = Some(Real { frame, cameras: *cameras });
                self.latest = Some(eye);
            }
            Err(e) => {
                if self.tally.failures == 0 {
                    log!("frame warping capture failed: {e}");
                }
                self.tally.failures += 1;
                self.latest = None;
            }
        }
    }

    /// At present: publishes this frame's real eye (`final_image`, the finished back buffer) and
    /// the other eye reprojected from it, with the HUD layer laid over each at `hud_rects[eye]`
    /// (x, y, width, height in pixels) when one was drawn this frame. `report` adjusts the pose
    /// record.
    pub fn publish(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        final_image: &ID3D11Texture2D,
        head: &HeadSource,
        hud_rects: [[f32; 4]; 2],
        report: impl Fn(&mut HeadPose),
    ) -> bool {
        let hud_drawn = std::mem::take(&mut self.hud_begun);
        let Some(eye) = self.latest.take() else { return false };
        let other = 1 - eye;
        let desc = d3d::texture_desc(final_image);
        let Some(real) = &self.real[eye] else { return false };
        let cameras = real.cameras;
        let view = View {
            projection: cameras.projection[other],
            camera: cameras.camera[other],
            mapping: cameras.mapping[other],
            width: desc.Width,
            height: desc.Height,
        };
        let fill = self.real[other].as_ref().map(|r| &r.frame);
        let synthesized = match self.reprojector.run(context, &view, &real.frame, fill) {
            Ok(image) => image.clone(),
            Err(e) => {
                if self.tally.failures == 0 {
                    log!("frame warping reprojection failed: {e}");
                }
                self.tally.failures += 1;
                return false;
            }
        };
        // The real eye's finished image, copied so the game can reuse its back buffer (typeless,
        // so an sRGB back buffer is read as stored).
        let copy = D3D11_TEXTURE2D_DESC { Format: DXGI_FORMAT_R8G8B8A8_TYPELESS, ..d3d::private_copy_desc(&desc) };
        let Some(shown) = copy_into(&mut self.shown, device, context, final_image, &copy).cloned() else { return false };
        let mut images = if eye == 0 { [shown, synthesized] } else { [synthesized, shown] };
        if hud_drawn && let Some(layer) = self.hud.as_mut() {
            for (slot, image) in images.iter_mut().enumerate() {
                match layer.composite(context, slot, image, hud_rects[slot]) {
                    Ok(with_hud) => *image = with_hud.clone(),
                    Err(e) => {
                        if self.tally.failures == 0 {
                            log!("HUD composite failed: {e}");
                        }
                        self.tally.failures += 1;
                    }
                }
            }
        }

        // A size the viewer did not open the channel at is refused (logged once), not failed.
        let Some(writer) = self.writer.get(device, desc.Width, desc.Height).ready(|note| log!("{note} (alternate-eye with depth)")) else { return false };
        let mut pose = cameras.pose;
        report(&mut pose);
        record_first(head, writer.next_sequence(), [pose, pose], |_| {});
        match self.tally.count(writer.publish(context, [&images[0], &images[1]], monaka_channel::tick())) {
            Ok(step) => step == Step::Published,
            Err(e) => {
                if self.tally.failures == 0 {
                    log!("publish failed: {e}");
                }
                self.tally.failures += 1;
                false
            }
        }
    }
}
