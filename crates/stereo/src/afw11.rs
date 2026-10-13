//! AFW on D3D11: the GPU work of [`crate::afw`] (which eye is real, the gaps' rules, what goes out
//! at each present: the shared core) over the keyed-mutex channel. Every presented frame is one
//! real eye, and the other eye of the same moment is that frame reprojected with its depth
//! (`monaka_warp::Reprojector`, the shader D3D12's [`crate::afw12`] runs too). What the real eye
//! could not see is filled from the other eye's own previous real frame, which saw it from the
//! right place, so the gaps get rendered pixels instead of stretched ones. A pair goes out every
//! frame (alternate-eye alone publishes every second frame, a frame apart).
//!
//! The game crate captures each frame's world image (before the HUD) and depth with the cameras
//! it was rendered from ([`Afw11::capture`]), and publishes at present ([`Afw11::publish`]).
//!
//! What D3D12 has and D3D11 not yet: each eye's depth to the headset (the keyed-mutex channel
//! carries colour only), and frame generation on top (no D3D11 frame generation).

pub mod ready;

pub use crate::afw::EyeCameras;
pub use ready::frame_warping;
use crate::afw::{Counts, History, Out, Presents, Rules, Shot, in_eye_order};
use crate::alternate::Step;
use crate::driver::Pending;
use crate::publish::{FLAT, Tally, copy_into, record_first};
use monaka_channel::d3d;
use monaka_channel::keyed::LazyWriter;
use monaka_channel::pose::HeadSource;
use monaka_channel::ChannelName;
use monaka_core::protocol::HeadPose;
use monaka_producer::log;
use monaka_warp::{Frame, HudLayer, Reprojector};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_TYPELESS;

/// Dying Light 1's rules: warped pairs only ([`Rules::WARPED_ONLY`]).
pub const RULES: Rules = Rules::WARPED_ONLY;

/// The present a pair goes out at: the loop's head source, the HUD's place in each eye (x, y,
/// width, height in pixels), whether it goes flat, its label, its number.
pub struct At<'a> {
    pub head: &'a HeadSource,
    pub hud_rects: [[f32; 4]; 2],
    pub flat: bool,
    pub label: Option<Pending>,
    pub present: u64,
}

pub struct Afw11 {
    reprojector: Reprojector,
    /// Each eye's latest real frame: copies of its world image and depth (how it was rendered and
    /// when are the core's [`History`]).
    frames: [Option<Frame>; 2],
    history: History,
    presents: Presents,
    /// This frame's real eye, once captured.
    latest: Option<Shot>,
    writer: LazyWriter,
    shown: Option<ID3D11Texture2D>,
    /// Each eye's last frame as presented, for a plain pair (its pose and present are the core's).
    plain: [Option<ID3D11Texture2D>; 2],
    /// The HUD as its own layer, laid over both eyes; begun by this frame's first HUD draw.
    hud: Option<HudLayer>,
    hud_begun: bool,
    hud_views: Option<[ID3D11RenderTargetView; 2]>,
    counts: Counts,
    /// Frames captured per eye.
    captured: [u64; 2],
    tally: Tally,
}

impl Afw11 {
    pub fn new(device: &ID3D11Device, channel: ChannelName) -> Result<Self, monaka_warp::Error> {
        Ok(Self {
            reprojector: Reprojector::new(device)?,
            frames: [None, None],
            history: History::new(RULES),
            presents: Presents::new(RULES),
            latest: None,
            writer: LazyWriter::new(channel),
            shown: None,
            plain: [None, None],
            hud: None,
            hud_begun: false,
            hud_views: None,
            counts: Counts::default(),
            captured: [0; 2],
            tally: Tally::NEW,
        })
    }

    /// Takes the HUD as a layer laid over both eyes ([`Afw11::hud_targets`]) instead of leaving
    /// it in the real eye's image only.
    pub fn enable_hud(&mut self, device: &ID3D11Device) -> Result<(), monaka_warp::Error> {
        self.hud = Some(HudLayer::new(device)?);
        Ok(())
    }

    /// Counts one failure of `what`, logging the first.
    fn failed(&mut self, what: &str, e: impl std::fmt::Display) {
        if self.counts.failures == 0 {
            log!("{what}: {e}");
        }
        self.counts.failures += 1;
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
                self.failed("HUD layer unavailable", e);
                None
            }
        }
    }

    /// The warps so far, and each eye's frames captured.
    pub fn counts(&self) -> (Counts, [u64; 2]) {
        (self.counts, self.captured)
    }

    /// Pairs published, and pairs the channel had no room for.
    pub fn published(&self) -> (u64, u64) {
        (self.tally.pairs, self.tally.failures)
    }

    /// The frame of `shot` is complete before its HUD: keep its world image `color` and `depth` as
    /// its eye's latest real frame.
    pub fn capture(&mut self, context: &ID3D11DeviceContext, shot: &Shot, color: &ID3D11Texture2D, depth: &ID3D11Texture2D) {
        let eye = shot.eye;
        self.history.begin();
        let reuse = self.frames[eye].take();
        match self.reprojector.capture(context, color, depth, shot.rendered.projection, shot.rendered.camera, reuse) {
            Ok(frame) => {
                self.frames[eye] = Some(frame);
                self.history.keep(shot);
                self.latest = Some(*shot);
                self.captured[eye] += 1;
            }
            Err(e) => {
                self.failed("frame warping capture failed", e);
                self.history.lose(eye);
                self.latest = None;
            }
        }
    }

    /// At present `at`: publishes what the core says goes out of this frame (`final_image`, the
    /// finished back buffer): its real eye and the other eye reprojected from it, the HUD layer laid
    /// over each at its place when one was drawn this frame. `report` adjusts the pose record.
    pub fn publish(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, final_image: &ID3D11Texture2D, at: &At, report: impl Fn(&mut HeadPose)) -> bool {
        let hud_drawn = std::mem::take(&mut self.hud_begun);
        let shot = self.latest.take();
        let out = self.presents.out(at.flat, at.label.is_some(), shot.is_some(), Instant::now());
        if out.forgets() {
            self.history.forget();
        }
        match (out, shot, at.label) {
            (Out::Warped, Some(shot), _) => self.publish_warped(device, context, final_image, &shot, at, hud_drawn, report),
            (Out::Flat, ..) => {
                crate::afw::Counters::add(&ready::COUNTERS.flat);
                let whole = whole_frame(final_image);
                let Some(image) = self.shown_image(device, context, final_image, hud_drawn.then_some((0, whole))) else { return false };
                self.send(device, context, [&image, &image], FLAT, at.head, |_| {})
            }
            (Out::BothEyes, ..) => {
                let pose = at.head.current().unwrap_or_default();
                let whole = whole_frame(final_image);
                let Some(image) = self.shown_image(device, context, final_image, hud_drawn.then_some((0, whole))) else { return false };
                self.send(device, context, [&image, &image], [pose; 2], at.head, report)
            }
            (Out::Plain, _, Some(label)) => self.publish_plain(device, context, final_image, label, at, hud_drawn, report),
            (_, shot, _) => {
                if shot.is_none() {
                    crate::afw::Counters::add(&ready::COUNTERS.no_inputs);
                }
                false
            }
        }
    }

    /// The warped pair: the real eye's finished image and the other eye reprojected from its frame,
    /// the gaps from the other eye's own latest real frame while the rules let it fill.
    #[allow(clippy::too_many_arguments)]
    fn publish_warped(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, final_image: &ID3D11Texture2D, shot: &Shot, at: &At, hud_drawn: bool, report: impl Fn(&mut HeadPose)) -> bool {
        let (eye, other) = (shot.eye, shot.other_eye());
        let desc = d3d::texture_desc(final_image);
        let warp = self.history.warp(shot);
        let Some(primary) = &self.frames[eye] else { return false };
        let fill = warp.fill.and(self.frames[other].as_ref());
        let synthesized = match self.reprojector.run(context, &warp.target.view(desc.Width, desc.Height), primary, fill) {
            Ok(image) => image.clone(),
            Err(e) => {
                self.failed("frame warping reprojection failed", e);
                return false;
            }
        };
        self.counts.pairs += 1;
        if fill.is_some() {
            self.counts.filled += 1;
        }
        let Some(shown) = self.shown_image(device, context, final_image, None) else { return false };
        let mut images = in_eye_order(eye, shown, synthesized);
        if hud_drawn {
            for (slot, image) in images.iter_mut().enumerate() {
                if let Some(with_hud) = self.with_hud(context, image, slot, at.hud_rects[slot]) {
                    *image = with_hud;
                }
            }
        }
        // The pose record as it always went: the report applied to the frame's pose, valid or not.
        let mut pose = shot.pose;
        report(&mut pose);
        self.send(device, context, [&images[0], &images[1]], [pose; 2], at.head, |_| {})
    }

    /// A frame not warped: its finished image as its eye's newest, beside the other eye's newest
    /// when that is the frame just before.
    #[allow(clippy::too_many_arguments)]
    fn publish_plain(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, final_image: &ID3D11Texture2D, label: Pending, at: &At, hud_drawn: bool, report: impl Fn(&mut HeadPose)) -> bool {
        let eye = label.eye.min(1);
        self.presents.lose_plain(eye);
        let Some(image) = self.shown_image(device, context, final_image, hud_drawn.then_some((eye, at.hud_rects[eye]))) else { return false };
        let desc = d3d::texture_desc(&image);
        let mut slot = self.plain[eye].take();
        let Some(copy) = copy_into(&mut slot, device, context, &image, &d3d::private_copy_desc(&desc)).cloned() else { return false };
        self.plain[eye] = Some(copy.clone());
        let Some(other_pose) = self.presents.plain(eye, label.pose, at.present) else { return false };
        let Some(other) = self.plain[1 - eye].clone() else { return false };
        crate::afw::Counters::add(&ready::COUNTERS.plain);
        self.send(device, context, in_eye_order(eye, &copy, &other), in_eye_order(eye, label.pose, other_pose), at.head, report)
    }

    /// The finished image copied so the game can reuse its back buffer (typeless, so an sRGB back
    /// buffer is read as stored), with the HUD layer laid over it as eye `hud.0` at `hud.1` when
    /// asked.
    fn shown_image(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, final_image: &ID3D11Texture2D, hud: Option<(usize, [f32; 4])>) -> Option<ID3D11Texture2D> {
        let desc = d3d::texture_desc(final_image);
        let copy = D3D11_TEXTURE2D_DESC { Format: DXGI_FORMAT_R8G8B8A8_TYPELESS, ..d3d::private_copy_desc(&desc) };
        let shown = copy_into(&mut self.shown, device, context, final_image, &copy).cloned()?;
        match hud {
            Some((eye, rect)) => Some(self.with_hud(context, &shown, eye, rect).unwrap_or(shown)),
            None => Some(shown),
        }
    }

    /// `image` with this frame's HUD layer laid over it as eye `eye` at `rect`.
    fn with_hud(&mut self, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, eye: usize, rect: [f32; 4]) -> Option<ID3D11Texture2D> {
        let result = self.hud.as_mut()?.composite(context, eye, image, rect).cloned();
        result.map_err(|e| self.failed("HUD composite failed", e)).ok()
    }

    /// Publishes `images` (left, right) with `poses` (`report` adjusting each eye that has one).
    #[allow(clippy::too_many_arguments)]
    fn send(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, images: [&ID3D11Texture2D; 2], poses: [HeadPose; 2], head: &HeadSource, report: impl Fn(&mut HeadPose)) -> bool {
        let desc = d3d::texture_desc(images[0]);
        // A size the viewer did not open the channel at is refused (logged once), not failed.
        let Some(writer) = self.writer.get(device, desc.Width, desc.Height).ready(|note| log!("{note} (alternate-eye with depth)")) else { return false };
        record_first(head, writer.next_sequence(), poses, report);
        let published = writer.publish(context, images, monaka_channel::tick());
        match self.tally.count(published) {
            Ok(step) => step == Step::Published,
            Err(e) => {
                self.failed("publish failed", e);
                false
            }
        }
    }
}

/// The whole of `image` as a HUD place.
fn whole_frame(image: &ID3D11Texture2D) -> [f32; 4] {
    let desc = d3d::texture_desc(image);
    [0.0, 0.0, desc.Width as f32, desc.Height as f32]
}
