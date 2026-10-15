//! FSR frame generation for D3D11 games, over the same kind of private D3D12 device as the
//! upscaler ([`crate::Fsr11`]): one frame generation context per eye, each given that eye's images
//! one after another. At each [`FrameGen11::generate`], per eye:
//!
//! 1. on the game's context, the image is copied into a ring of shared textures (frame generation
//!    reads the previous image from the texture it was given, so the last two stay as they were),
//!    and its depth and motion vectors onto the eye's previous image are written into two more
//!    ([`monaka_warp::MotionPass`]: the camera's motion only, from the depth and both cameras), then
//!    the fence is signalled;
//! 2. the D3D12 queue waits for that value, generates the frame halfway between the eye's previous
//!    image and this one into the eye's shared output, and signals the next value;
//! 3. the game's context waits for it before anything reads the outputs.
//!
//! What moves in the world besides the camera has no motion vectors here; frame generation's own
//! optical flow is all it has to go by.

use super::{Beside, Error, Result, Shared, uav};
use monaka_channel::d3d;
use monaka_channel::d3d12::{Recorder, wait_fence};
use monaka_ffx::{Api, Camera, Float2, FrameGen, Size2};
use monaka_warp::{EyeView, MotionPass};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D, ID3D11UnorderedAccessView};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R16G16_FLOAT, DXGI_FORMAT_R32_FLOAT,
};
use windows::core::Interface;

/// Each eye's images kept: the one being written and the two frame generation reads.
const RING: usize = 3;
/// The far plane given with an infinite projection (as the upscaler's).
const FAR: f32 = 100_000.0;

/// Settings for [`FrameGen11::new`].
#[derive(Clone, Copy, Debug)]
pub struct GenSettings {
    /// The size of the depth (and so of the motion vectors) each eye gives.
    pub render: (u32, u32),
    /// The size of each eye's images.
    pub display: (u32, u32),
    /// 1 or -1: whether a depth's jitter is told as the content shift or its opposite (the
    /// upscaler's `jitter_sign`).
    pub jitter_sign: f32,
    /// `monaka_ffx::create` flags for the depth convention (inverted, infinite).
    pub depth_flags: u32,
}

/// One eye's image for [`FrameGen11::generate`].
pub struct GenFrame<'a> {
    /// The image (the display size, RGBA8 of any of the family's formats, one sample).
    pub color: &'a ID3D11Texture2D,
    /// Its depth (any size and depth format the motion pass reads), in `view`'s convention,
    /// rendered with the sub-pixel `jitter` (its pixels; 0 for a depth made unjittered).
    pub depth: &'a ID3D11Texture2D,
    pub jitter: [f32; 2],
    /// The camera and unjittered projection the image was seen with, and the eye's previous
    /// image's (the same view for an eye's first image).
    pub view: EyeView,
    pub previous: EyeView,
    /// The near plane in the camera's units and the vertical field of view (radians).
    pub near: f32,
    pub fov_vertical: f32,
}

struct Eye {
    generator: FrameGen,
    images: [Shared; RING],
    depth: Shared,
    depth_uav: ID3D11UnorderedAccessView,
    vectors: Shared,
    vectors_uav: ID3D11UnorderedAccessView,
    output: Shared,
    frame_id: u64,
    /// The eye's previous image was given (its history runs on).
    primed: bool,
}

pub struct FrameGen11 {
    device: ID3D11Device,
    device12: ID3D12Device,
    queue: ID3D12CommandQueue,
    recorder: Recorder,
    fence12: ID3D12Fence,
    fence11: ID3D11Fence,
    /// The fence's last value: odd once the inputs are written, even once the frames are made.
    value: u64,
    render: (u32, u32),
    display: (u32, u32),
    jitter_sign: f32,
    /// The ring slot the last images went into.
    slot: usize,
    eyes: [Eye; 2],
    /// The motion passes, one per depth format given (each keeps a copy of that format: a real
    /// eye's depth and a warped one take turns in each eye).
    motions: Vec<(DXGI_FORMAT, MotionPass)>,
    version: String,
}

// SAFETY: used under its owner's lock (or by its one thread).
unsafe impl Send for FrameGen11 {}

/// Whether a texture of `format` copies into an RGBA8 UNORM image as stored.
fn rgba8(format: DXGI_FORMAT) -> bool {
    [DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB].contains(&format)
}

impl FrameGen11 {
    /// A D3D12 device on the game device's adapter, the shared textures and one frame generation
    /// context per eye.
    pub fn new(device: &ID3D11Device, api: Api, settings: GenSettings) -> Result<Self> {
        let Beside { device12, queue, fence12, fence11 } = Beside::new(device)?;
        let recorder = Recorder::on(&device12, 3, fence12.clone())?;
        let (render, display) = (settings.render, settings.display);
        let size = |(width, height): (u32, u32)| Size2 { width, height };
        let eye = || -> Result<Eye> {
            let generator = FrameGen::new(api, &device12, size(display), size(render), DXGI_FORMAT_R8G8B8A8_UNORM, settings.depth_flags)?;
            let image = || Shared::new(device, &device12, display, DXGI_FORMAT_R8G8B8A8_UNORM);
            let images = [image()?, image()?, image()?];
            let depth = Shared::new(device, &device12, render, DXGI_FORMAT_R32_FLOAT)?;
            let vectors = Shared::new(device, &device12, render, DXGI_FORMAT_R16G16_FLOAT)?;
            Ok(Eye {
                generator,
                images,
                depth_uav: uav(device, &depth.d11)?,
                depth,
                vectors_uav: uav(device, &vectors.d11)?,
                vectors,
                output: Shared::new(device, &device12, display, DXGI_FORMAT_R8G8B8A8_UNORM)?,
                frame_id: 0,
                primed: false,
            })
        };
        let eyes = [eye()?, eye()?];
        let version = eyes[0].generator.version().to_owned();
        Ok(Self { device: device.clone(), device12, queue, recorder, fence12, fence11, value: 0, render, display, jitter_sign: settings.jitter_sign, slot: 0, eyes, motions: Vec::new(), version })
    }

    /// The provider's version name (for the log).
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn render(&self) -> (u32, u32) {
        self.render
    }

    pub fn display(&self) -> (u32, u32) {
        self.display
    }

    /// Generates, for each eye (left, right), the frame halfway between its previous image and
    /// `eyes`' on the game's `context`, `frame_time_ms` after the last; `reset` (a cut) starts each
    /// eye's history again. `Ok(true)` when both eyes' frames are usable ([`FrameGen11::generated`]),
    /// `Ok(false)` when the histories only started. Either way the images are kept
    /// ([`FrameGen11::kept`]).
    pub fn generate(&mut self, context: &ID3D11DeviceContext, eyes: &[GenFrame<'_>; 2], frame_time_ms: f32, reset: bool) -> Result<bool> {
        for frame in eyes {
            let desc = d3d::texture_desc(frame.color);
            if (desc.Width, desc.Height) != self.display {
                return Err(Error::Unavailable(format!("a {}x{} image for {}x{} frame generation", desc.Width, desc.Height, self.display.0, self.display.1)));
            }
            if !rgba8(desc.Format) || desc.SampleDesc.Count != 1 || desc.MipLevels != 1 || desc.ArraySize != 1 {
                return Err(Error::Unavailable(format!("an image of format {} ({} samples) for frame generation", desc.Format.0, desc.SampleDesc.Count)));
            }
        }
        let context4: ID3D11DeviceContext4 = context.cast()?;
        let Some(list) = self.recorder.begin()? else { return Err(Error::Busy) };
        let slot = (self.slot + 1) % RING;
        // SAFETY: a GPU wait on the shared fence: the last frames' D3D12 work is done with the
        // inputs before they are written again.
        let inputs = unsafe { context4.Wait(&self.fence11, self.value) }.map_err(Error::from).and_then(|()| self.write_inputs(context, eyes, slot));
        self.value += 1;
        let written = self.value;
        // SAFETY: as above; the flush hands the work to the GPU so the D3D12 queue's wait ends.
        let signalled = unsafe { context4.Signal(&self.fence11, written) }.map_err(Error::from);
        // SAFETY: submits the context's queued work.
        unsafe { context.Flush() };
        // Whenever the context signalled, the queue waits for it, inputs or not: its own signal must
        // come after the context's, or the context's would set the fence back below the value the
        // context then waits for.
        let signal_ok = signalled.is_ok();
        let ready = inputs.and(signalled);
        let ready_ok = ready.is_ok();
        let recorded = ready.and_then(|()| self.record(&list, eyes, slot, frame_time_ms, reset));
        // SAFETY: the queue waits on the GPU for a value the context signalled.
        let waited = if signal_ok { unsafe { self.queue.Wait(&self.fence12, written) }.map_err(Error::from) } else { Ok(()) };
        // The list goes in whatever happened (empty without its inputs), so the ring of lists and
        // the fence keep counting.
        self.value = self.recorder.submit_as(&self.queue, written + 1)?;
        // SAFETY: the game's context waits on the GPU for the frames before reading them.
        unsafe { context4.Wait(&self.fence11, self.value)? };
        if ready_ok {
            self.slot = slot;
        } else {
            // The eyes' histories missed these images.
            self.eyes.iter_mut().for_each(|eye| eye.primed = false);
        }
        waited?;
        recorded
    }

    /// Each eye's image and its depth and motion vectors into the shared inputs.
    fn write_inputs(&mut self, context: &ID3D11DeviceContext, eyes: &[GenFrame<'_>; 2], slot: usize) -> Result<()> {
        for (eye, frame) in self.eyes.iter().zip(eyes) {
            // SAFETY: a copy between same-shaped textures of one family (checked) on the game's
            // device.
            unsafe { context.CopyResource(&eye.images[slot].d11, frame.color) };
            let format = d3d::texture_desc(frame.depth).Format;
            let at = match self.motions.iter().position(|(f, _)| *f == format) {
                Some(at) => at,
                None => {
                    self.motions.push((format, MotionPass::new(&self.device)?));
                    self.motions.len() - 1
                }
            };
            self.motions[at].1.run(context, frame.depth, &frame.view, Some(&frame.previous), None, &eye.vectors_uav, &eye.depth_uav, self.render)?;
        }
        Ok(())
    }

    /// Records both eyes' frames on `list`. True when both are usable; on an error neither eye's
    /// history runs on.
    fn record(&mut self, list: &ID3D12GraphicsCommandList, eyes: &[GenFrame<'_>; 2], slot: usize, frame_time_ms: f32, reset: bool) -> Result<bool> {
        let render = Size2 { width: self.render.0, height: self.render.1 };
        let mut outcome = Ok(true);
        for (eye, frame) in self.eyes.iter_mut().zip(eyes) {
            let reset = reset || !eye.primed;
            eye.frame_id += 1;
            let camera = camera(frame, render, self.jitter_sign);
            let id = eye.frame_id;
            let result = eye
                .generator
                .configure(id, None)
                .and_then(|()| eye.generator.prepare(list, id, &camera, eye.depth.resource(DXGI_FORMAT_R32_FLOAT), eye.vectors.resource(DXGI_FORMAT_R16G16_FLOAT), frame_time_ms))
                .and_then(|()| eye.generator.generate(list, id, eye.images[slot].resource(DXGI_FORMAT_R8G8B8A8_UNORM), eye.output.resource(DXGI_FORMAT_R8G8B8A8_UNORM), reset));
            eye.primed = result.is_ok();
            outcome = match (outcome, result) {
                (Err(e), _) => Err(e),
                (Ok(_), Err(e)) => Err(Error::Ffx(e)),
                (Ok(usable), Ok(())) => Ok(usable && !reset),
            };
        }
        if outcome.is_err() {
            self.eyes.iter_mut().for_each(|eye| eye.primed = false);
        }
        outcome
    }

    /// Each eye's generated frame (left, right; the display size, RGBA8 UNORM) of the last
    /// [`FrameGen11::generate`] that said it was usable, valid until the next one.
    pub fn generated(&self) -> [ID3D11Texture2D; 2] {
        [self.eyes[0].output.d11.clone(), self.eyes[1].output.d11.clone()]
    }

    /// Each eye's image as the last [`FrameGen11::generate`] kept it (left, right; RGBA8 UNORM),
    /// valid for the next generate too (the ring's third slot is the one written then).
    pub fn kept(&self) -> [ID3D11Texture2D; 2] {
        [self.eyes[0].images[self.slot].d11.clone(), self.eyes[1].images[self.slot].d11.clone()]
    }
}

/// What frame generation is told of `frame`, its depth and motion vectors at `render`: the
/// jitter as the content shift times `jitter_sign`, the motion vectors from UV to pixels, the
/// camera's place and axes (canonical: x right, y up, z back).
fn camera(frame: &GenFrame<'_>, render: Size2, jitter_sign: f32) -> Camera {
    let c = &frame.view.camera;
    Camera {
        render_size: render,
        jitter: Float2 { x: jitter_sign * frame.jitter[0], y: jitter_sign * frame.jitter[1] },
        motion_vector_scale: Float2 { x: render.width as f32, y: render.height as f32 },
        near: frame.near,
        far: FAR,
        fov_vertical: frame.fov_vertical,
        position: [c[3], c[7], c[11]],
        up: [c[1], c[5], c[9]],
        right: [c[0], c[4], c[8]],
        forward: [-c[2], -c[6], -c[10]],
    }
}

impl Drop for FrameGen11 {
    fn drop(&mut self) {
        // The contexts and shared textures go once the GPU is done with them.
        if !wait_fence(&self.fence12, self.value, 2000) {
            monaka_producer::log!("frame generation: the GPU did not finish within 2 s; releasing anyway");
        }
        let _ = (&self.device, &self.device12);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_channel::d3d::{create_device, create_texture_with, read_pixel};
    use monaka_ffx as ffx;
    use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC};

    fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bytes: &[u8]) -> ID3D11Texture2D {
        create_texture_with(device, &D3D11_TEXTURE2D_DESC { BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..d3d::simple_texture_desc(w, h, format) }, bytes).unwrap()
    }

    #[test]
    fn camera_axes_and_scales() {
        let projection = [0.0f32; 16];
        let camera_matrix = [1.0, 0.0, 0.0, 5.0, 0.0, 0.0, 1.0, 6.0, 0.0, -1.0, 0.0, 7.0];
        let image = (0u8..16).collect::<Vec<_>>();
        let (device, _, _) = create_device(None).unwrap();
        let color = texture(&device, 2, 2, DXGI_FORMAT_R8G8B8A8_UNORM, &image);
        let view = EyeView { camera: camera_matrix, projection };
        let frame = GenFrame { color: &color, depth: &color, jitter: [0.25, -0.5], view, previous: view, near: 0.1, fov_vertical: 1.2 };
        let c = camera(&frame, Size2 { width: 640, height: 360 }, -1.0);
        assert_eq!((c.jitter.x, c.jitter.y), (-0.25, 0.5), "the jitter times its sign");
        assert_eq!((c.motion_vector_scale.x, c.motion_vector_scale.y), (640.0, 360.0), "UV to render pixels");
        assert_eq!(c.position, [5.0, 6.0, 7.0]);
        assert_eq!(c.right, [1.0, 0.0, 0.0]);
        assert_eq!(c.up, [0.0, 0.0, -1.0]);
        assert_eq!(c.forward, [0.0, -1.0, 0.0], "forward is the camera's -z");
        assert_eq!((c.near, c.far, c.fov_vertical), (0.1, FAR, 1.2));
    }

    #[test]
    fn only_rgba8_images_are_taken() {
        assert!(rgba8(DXGI_FORMAT_R8G8B8A8_TYPELESS) && rgba8(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB));
        assert!(!rgba8(windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM));
    }

    /// A still picture at 128x72 for both eyes, a wall 2 m away, rendered at 64x36: after a few
    /// frames each eye's generated frame is the picture, and the kept images are what was given.
    #[test]
    fn generates_both_eyes_of_a_d3d11_device() {
        // A folder with AMD's FidelityFX DLLs (a game installs them: Dying Light 2's are in its
        // `ph\work\bin\x64`).
        let Some(dir) = std::env::var_os("MONAKA_FFX_DIR").map(std::path::PathBuf::from).filter(|d| d.join(ffx::LOADER).exists() && d.join(crate::FRAMEGEN_DLL).exists()) else {
            return eprintln!("MONAKA_FFX_DIR (a folder with AMD's FidelityFX DLLs) is not set; skipped");
        };
        let api = crate::load_with_framegen(&dir).unwrap();
        let (device, context, _) = create_device(None).unwrap();
        let (display, render) = ((128u32, 72u32), (64u32, 36u32));
        let settings = GenSettings { render, display, jitter_sign: 1.0, depth_flags: ffx::create::DEPTH_INFINITE };
        let mut generator = FrameGen11::new(&device, api, settings).unwrap();
        eprintln!("provider {}", generator.version());
        // A standard-depth infinite projection, a wall 2 m away.
        let near = 0.1f32;
        let mut projection = [0.0f32; 16];
        (projection[0], projection[5], projection[10], projection[11], projection[14]) = (1.0, 1.0, -1.0, -2.0 * near, -1.0);
        let wall = 1.0 - near;
        let pixels: Vec<u8> = (0..display.0 * display.1).flat_map(|_| [40u8, 120, 200, 255]).collect();
        let color = texture(&device, display.0, display.1, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &pixels);
        let depth: Vec<u8> = (0..render.0 * render.1).flat_map(|_| wall.to_le_bytes()).collect();
        let depth = texture(&device, render.0, render.1, DXGI_FORMAT_R32_FLOAT, &depth);
        let view = |x: f32| EyeView { camera: [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], projection };
        let frame = |x: f32| GenFrame { color: &color, depth: &depth, jitter: [0.0; 2], view: view(x), previous: view(x), near, fov_vertical: 1.57 };
        let mut usable = Vec::new();
        for n in 0..4 {
            let eyes = [frame(-0.032), frame(0.032)];
            // Faster than any game: wait for a free command list as a frame's time would.
            let made = loop {
                match generator.generate(&context, &eyes, 11.1, n == 0) {
                    Err(Error::Busy) => std::thread::sleep(std::time::Duration::from_millis(1)),
                    other => break other.unwrap(),
                }
            };
            usable.push(made);
        }
        assert_eq!(usable, [false, true, true, true], "the first frame starts the histories");
        for (output, kept) in generator.generated().iter().zip(generator.kept()) {
            for texture in [output, &kept] {
                let pixel = read_pixel(&device, &context, texture, 100, 50).unwrap();
                for (got, want) in pixel.iter().zip([40u8, 120, 200]) {
                    assert!(got.abs_diff(want) <= 3, "{pixel:?}, expected the still picture");
                }
            }
        }
    }
}
