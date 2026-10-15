//! Rendered frames redrawn from another camera (`reproject.hlsl`, shared with
//! [`crate::reproject12`], which runs it on D3D12). For alternate-eye rendering with depth: each
//! presented frame is one real eye; the other eye of the same moment is this frame reprojected (the
//! primary), and what this frame could not see is filled from that eye's own previous real frame
//! reprojected to now (the fill), so gaps get rendered pixels, not stretched ones.
//!
//! What both APIs share besides the shader: its constants ([`Constants`]) and the matrix taking a
//! source frame into the view ([`source_matrix`]).

use crate::{ConstantBuffer, Error, Readable, Result, SavedCompute, color_formats, compute_shader, depth_formats, readable, shader_view, uav_texture};
use monaka_channel::d3d;
use monaka_core::camera::{Mat34, Mat44};
use monaka_core::depth::{DepthMapping, invert, reprojection};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT};
use windows::core::s;

#[cfg(test)]
mod tests;

/// The shader on D3D11: its images written as UNORM (the views' own format, as D3D11 checks).
pub const REPROJECT_SOURCE: &str = concat!("#define IMAGE unorm float4\n", include_str!("reproject.hlsl"));

/// A frame's depth plane as R32 ([`Reprojector::plain_depth`]).
const PLAIN_DEPTH: &str = r"
Texture2D<float> Depth : register(t0);
RWTexture2D<float> Out : register(u0);
[numthreads(8, 8, 1)] void PlainDepth(uint3 id : SV_DispatchThreadID) {
    uint w, h;
    Out.GetDimensions(w, h);
    if (id.x < w && id.y < h) Out[id.xy] = Depth.Load(int3(id.xy, 0));
}
";

/// The layout of the HLSL `cbuffer Reproject`, the same on both APIs (D3D11's constant buffer,
/// D3D12's root constants).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Constants {
    pub m: [f32; 16],
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub depth_width: u32,
    pub depth_height: u32,
    pub out_depth_width: u32,
    pub out_depth_height: u32,
    pub near_a: f32,
    pub near_b: f32,
    /// The source depth's jitter, in its pixels (what its content moved by).
    pub jitter: [f32; 2],
    /// Where the area that lines up with the image starts in the source depth and the output depth.
    pub depth_origin: [u32; 2],
    /// The fill where it shows the primary's surface (`monaka_stereo::afw::Rules::fill_first`).
    pub fill_first: u32,
    /// A debug view instead of the image ([`Reprojector::sources`]).
    pub sources: u32,
    /// Resolves: the target's clip to the primary's (`reproject.hlsl`'s `Seen`).
    pub back: [f32; 16],
    /// Resolves: the primary's depth area (origin, size; size 0: none).
    pub primary_depth: [u32; 4],
}

const _: () = assert!(size_of::<Constants>() == 52 * 4);

/// The part of a depth texture that lines up with the image: left, top, width, height in texels
/// (`None`: all of it), as `monaka_framegen::inputs::rect` gives it.
pub type DepthRect = Option<[u32; 4]>;

/// `rect`'s origin and size for a depth texture of `size`.
pub(crate) fn depth_area(rect: DepthRect, size: (u32, u32)) -> ([u32; 2], (u32, u32)) {
    match rect {
        Some([left, top, width, height]) if width > 0 && height > 0 && left + width <= size.0 && top + height <= size.1 => ([left, top], (width, height)),
        _ => ([0, 0], size),
    }
}

impl Constants {
    /// A redraw into `view`, its depth written at `out_depth` (width, height) when asked, from
    /// `origin` in the output texture, the fill first where it agrees (`fill_first`): the constants
    /// of its clears and resolves.
    pub(crate) fn redraw(view: &View, out_depth: Option<(u32, u32)>, origin: [u32; 2], fill_first: bool) -> Self {
        let (out_depth_width, out_depth_height) = out_depth.unwrap_or((0, 0));
        Self {
            width: view.width,
            height: view.height,
            out_depth_width,
            out_depth_height,
            near_a: view.mapping.a,
            near_b: view.mapping.b,
            depth_origin: origin,
            fill_first: fill_first as u32,
            ..Default::default()
        }
    }

    /// The splat of a source frame into this redraw's `view`: its colour of `color` size, its depth
    /// `depth` (the origin and size of the area that lines up with the image) rendered with `jitter`,
    /// `projection` and `camera`. `None` when its cameras make no reprojection (left out, as a
    /// missing fill).
    pub(crate) fn splat(&self, view: &View, color: (u32, u32), depth: ([u32; 2], (u32, u32)), jitter: [f32; 2], projection: &Mat44, camera: &Mat34) -> Option<Self> {
        let m = source_matrix(view, projection, camera)?;
        let (origin, (depth_width, depth_height)) = depth;
        Some(Self { m, source_width: color.0, source_height: color.1, depth_width, depth_height, jitter, depth_origin: origin, ..*self })
    }

    /// This redraw's resolves, asking the primary's eye about the fill (`primary`: its splat's
    /// constants): the way back into its frame and its depth area. Without it, nothing to ask.
    pub(crate) fn resolve(&self, primary: Option<&Self>) -> Self {
        let back = primary.and_then(|p| Some((invert(&p.m)?, p)));
        match back {
            Some((back, p)) => Self { back, primary_depth: [p.depth_origin[0], p.depth_origin[1], p.depth_width, p.depth_height], ..*self },
            None => *self,
        }
    }
}

/// The matrix taking a source frame rendered with `projection` and `camera` into `view`: both
/// cameras moved by the view's position, so it is built from small numbers far from the origin.
pub fn source_matrix(view: &View, projection: &Mat44, camera: &Mat34) -> Option<Mat44> {
    let to_origin = |camera: &Mat34| {
        let mut c = *camera;
        (c[3], c[7], c[11]) = (c[3] - view.camera[3], c[7] - view.camera[7], c[11] - view.camera[11]);
        c
    };
    reprojection(projection, &to_origin(camera), &view.projection, &to_origin(&view.camera))
}

/// A rendered frame kept for reprojection: private copies of its colour and depth, and the
/// canonical camera and projection it was rendered with.
pub struct Frame {
    color: Readable,
    depth: Readable,
    pub projection: Mat44,
    pub camera: Mat34,
    /// How far the depth's content is shifted by a jittered projection (its pixels, x right, y
    /// down; the colour unjittered, as an upscaler gives it). 0 for a frame whose colour and depth
    /// were drawn alike.
    pub jitter: [f32; 2],
}

impl Frame {
    /// Its depth copy (typeless, the depth's own size and format), valid while the frame is kept.
    pub fn depth(&self) -> &ID3D11Texture2D {
        &self.depth.texture
    }
}

/// The camera to draw into.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub projection: Mat44,
    pub camera: Mat34,
    /// The target projection's depth-to-nearness mapping (for the depth test).
    pub mapping: DepthMapping,
    pub width: u32,
    pub height: u32,
}

/// One layer's depth-test keys and painted image.
struct Layer {
    keys: ID3D11Texture2D,
    keys_uav: ID3D11UnorderedAccessView,
    keys_srv: ID3D11ShaderResourceView,
    image: ID3D11Texture2D,
    image_uav: ID3D11UnorderedAccessView,
    image_srv: ID3D11ShaderResourceView,
}

pub struct Reprojector {
    /// The fill first where its depth agrees with the primary's (`reproject.hlsl`).
    pub fill_first: bool,
    /// A debug view instead of the image (`reproject.hlsl`'s `Shown`): 0 none, 1 each pixel tinted
    /// by where it came from (the fill green, a neighbour solid blue, nothing magenta), 2 its nearness as
    /// grey.
    pub sources: u32,
    device: ID3D11Device,
    clear: ID3D11ComputeShader,
    splat: ID3D11ComputeShader,
    paint: ID3D11ComputeShader,
    resolve: ID3D11ComputeShader,
    /// The view's depth pass, made at the first [`Reprojector::run_with_depth`].
    resolve_depth: Option<ID3D11ComputeShader>,
    constants: ConstantBuffer<Constants>,
    layers: Option<[Layer; 2]>,
    output: Option<(ID3D11Texture2D, ID3D11UnorderedAccessView)>,
    /// The view's depth ([`Reprojector::run_with_depth`]) and its size.
    depth_output: Option<(ID3D11Texture2D, ID3D11UnorderedAccessView, (u32, u32))>,
    size: (u32, u32),
    /// [`Reprojector::plain_depth`]'s pass and its output, made at its first call.
    plain_depth: Option<ID3D11ComputeShader>,
    plain_output: Option<(ID3D11Texture2D, ID3D11UnorderedAccessView, (u32, u32))>,
}

impl Reprojector {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let shader = |entry| compute_shader(device, REPROJECT_SOURCE, "reproject.hlsl", entry);
        Ok(Self {
            fill_first: false,
            sources: 0,
            device: device.clone(),
            clear: shader(s!("Clear"))?,
            splat: shader(s!("Splat"))?,
            paint: shader(s!("Paint"))?,
            resolve: shader(s!("Resolve"))?,
            resolve_depth: None,
            constants: ConstantBuffer::new(device)?,
            layers: None,
            output: None,
            depth_output: None,
            size: (0, 0),
            plain_depth: None,
            plain_output: None,
        })
    }

    /// Keeps a frame: copies `color` and `depth` (as they are now on the GPU timeline) with the
    /// camera and projection they were rendered with. `reuse` hands back an older frame's
    /// textures when the sizes match.
    pub fn capture(
        &self,
        context: &ID3D11DeviceContext,
        color: &ID3D11Texture2D,
        depth: &ID3D11Texture2D,
        projection: Mat44,
        camera: Mat34,
        reuse: Option<Frame>,
    ) -> Result<Frame> {
        let (color_desc, depth_desc) = (d3d::texture_desc(color), d3d::texture_desc(depth));
        if color_desc.SampleDesc.Count != 1 || depth_desc.SampleDesc.Count != 1 {
            return Err(Error::Unsupported("multisampled frame"));
        }
        let (old_color, old_depth) = reuse.map(|f| (Some(f.color), Some(f.depth))).unwrap_or((None, None));
        let color_copy = readable(&self.device, old_color, &color_desc, color_formats(color_desc.Format).ok_or(Error::Unsupported("colour format"))?)?;
        let depth_copy = readable(&self.device, old_depth, &depth_desc, depth_formats(depth_desc.Format).ok_or(Error::Unsupported("depth format"))?)?;
        // SAFETY: copies between same-shaped textures of one typeless family on this device.
        unsafe {
            context.CopyResource(&color_copy.texture, color);
            context.CopyResource(&depth_copy.texture, depth);
        }
        Ok(Frame { color: color_copy, depth: depth_copy, projection, camera, jitter: [0.0; 2] })
    }

    fn targets(&mut self, width: u32, height: u32) -> Result<()> {
        if self.size == (width, height) && self.layers.is_some() && self.output.is_some() {
            return Ok(());
        }
        let device = &self.device;
        let layer = || -> Result<Layer> {
            let (keys, keys_uav) = uav_texture(device, width, height, DXGI_FORMAT_R32_UINT)?;
            let (image, image_uav) = uav_texture(device, width, height, DXGI_FORMAT_R8G8B8A8_UNORM)?;
            let (keys_srv, image_srv) = (shader_view(device, &keys, DXGI_FORMAT_R32_UINT)?, shader_view(device, &image, DXGI_FORMAT_R8G8B8A8_UNORM)?);
            Ok(Layer { keys, keys_uav, keys_srv, image, image_uav, image_srv })
        };
        self.layers = Some([layer()?, layer()?]);
        self.output = Some(uav_texture(device, width, height, DXGI_FORMAT_R8G8B8A8_UNORM)?);
        self.size = (width, height);
        Ok(())
    }

    /// Draws `view` from `primary`, filling its gaps from `fill`, then a neighbour. The image is
    /// RGBA8, the view's size, and stays valid until the next run.
    pub fn run(&mut self, context: &ID3D11DeviceContext, view: &View, primary: &Frame, fill: Option<&Frame>) -> Result<&ID3D11Texture2D> {
        self.draw(context, view, primary, fill, None)?;
        Ok(&self.output.as_ref().expect("made by the draw").0)
    }

    /// [`Reprojector::run`], also writing the view's depth at `depth_size` (R32_FLOAT, in the
    /// view projection's own convention, from the same pixels as the image), as
    /// [`crate::reproject12`] writes it. Both stay valid until the next run.
    pub fn run_with_depth(&mut self, context: &ID3D11DeviceContext, view: &View, primary: &Frame, fill: Option<&Frame>, depth_size: (u32, u32)) -> Result<(&ID3D11Texture2D, &ID3D11Texture2D)> {
        if !(view.mapping.a.is_finite() && view.mapping.a != 0.0 && view.mapping.b.is_finite()) {
            return Err(Error::Unsupported("depth convention"));
        }
        if depth_size.0 == 0 || depth_size.1 == 0 {
            return Err(Error::Unsupported("depth size"));
        }
        if self.resolve_depth.is_none() {
            self.resolve_depth = Some(compute_shader(&self.device, REPROJECT_SOURCE, "reproject.hlsl", s!("ResolveDepth"))?);
        }
        if self.depth_output.as_ref().is_none_or(|(_, _, size)| *size != depth_size) {
            let (texture, uav) = uav_texture(&self.device, depth_size.0, depth_size.1, DXGI_FORMAT_R32_FLOAT)?;
            self.depth_output = Some((texture, uav, depth_size));
        }
        self.draw(context, view, primary, fill, Some(depth_size))?;
        let image = &self.output.as_ref().expect("made by the draw").0;
        let depth = &self.depth_output.as_ref().expect("made above").0;
        Ok((image, depth))
    }

    /// `frame`'s depth plane as R32_FLOAT at its own size (the stencil of a depth-stencil format
    /// left out), as the fence channel carries depth. Valid until the next call.
    pub fn plain_depth(&mut self, context: &ID3D11DeviceContext, frame: &Frame) -> Result<&ID3D11Texture2D> {
        if self.plain_depth.is_none() {
            self.plain_depth = Some(compute_shader(&self.device, PLAIN_DEPTH, "plain depth", s!("PlainDepth"))?);
        }
        let size = (frame.depth.width, frame.depth.height);
        if self.plain_output.as_ref().is_none_or(|(_, _, made)| *made != size) {
            let (texture, uav) = uav_texture(&self.device, size.0, size.1, DXGI_FORMAT_R32_FLOAT)?;
            self.plain_output = Some((texture, uav, size));
        }
        let (texture, uav, _) = self.plain_output.as_ref().expect("made above");
        let saved = SavedCompute::take(context);
        // SAFETY: binds our own shader and views on the game's context, dispatches, and restores
        // what was bound.
        unsafe {
            context.CSSetShaderResources(0, Some(&[Some(frame.depth.view.clone())]));
            context.CSSetUnorderedAccessViews(0, 1, Some([Some(uav.clone())].as_ptr()), None);
            context.CSSetShader(self.plain_depth.as_ref(), None);
            context.Dispatch(size.0.div_ceil(8), size.1.div_ceil(8), 1);
            context.CSSetUnorderedAccessViews(0, 1, Some([None].as_ptr()), None);
            context.CSSetShaderResources(0, Some(&[None]));
        }
        saved.restore(context);
        Ok(texture)
    }

    /// The image of `view` and, with `depth_size` (its target made by the caller), its depth.
    fn draw(&mut self, context: &ID3D11DeviceContext, view: &View, primary: &Frame, fill: Option<&Frame>, depth_size: Option<(u32, u32)>) -> Result<()> {
        self.targets(view.width, view.height)?;
        let layers = self.layers.as_ref().expect("made above");
        let (_, output_uav) = self.output.as_ref().expect("made above");
        // The depth pass and its target, when asked.
        let depth = depth_size.and_then(|_| Some((self.resolve_depth.as_ref()?, &self.depth_output.as_ref()?.1)));
        let groups = |w: u32, h: u32| (w.div_ceil(8), h.div_ceil(8));
        let saved = SavedCompute::take(context);
        // The depth target's slot is beyond what SavedCompute keeps: its binding is kept here.
        let mut slot3: [Option<ID3D11UnorderedAccessView>; 1] = [None];
        if depth.is_some() {
            // SAFETY: reads one of the context's current bindings into a local.
            unsafe { context.CSGetUnorderedAccessViews(3, Some(&mut slot3)) };
        }
        self.constants.bind(context);
        let redraw = Constants { sources: self.sources, ..Constants::redraw(view, depth.and(depth_size), [0, 0], self.fill_first) };
        let splat_of = |frame: &Frame| redraw.splat(view, (frame.color.width, frame.color.height), ([0, 0], (frame.depth.width, frame.depth.height)), frame.jitter, &frame.projection, &frame.camera);
        let resolve = redraw.resolve(splat_of(primary).as_ref());
        // SAFETY: binds our own shaders and views on the game's context, dispatches, and restores
        // what was bound.
        unsafe {
            for (layer, frame) in layers.iter().zip([Some(primary), fill]) {
                let uavs = [Some(layer.keys_uav.clone()), Some(layer.image_uav.clone())];
                context.CSSetUnorderedAccessViews(0, 2, Some(uavs.as_ptr()), None);
                self.constants.update(context, &redraw);
                let (gx, gy) = groups(view.width, view.height);
                context.CSSetShader(&self.clear, None);
                context.Dispatch(gx, gy, 1);
                let Some(frame) = frame else { continue };
                let Some(splat) = splat_of(frame) else { continue };
                self.constants.update(context, &splat);
                context.CSSetShaderResources(0, Some(&[Some(frame.color.view.clone()), Some(frame.depth.view.clone())]));
                let (sx, sy) = groups(frame.color.width, frame.color.height);
                for shader in [&self.splat, &self.paint] {
                    context.CSSetShader(shader, None);
                    context.Dispatch(sx, sy, 1);
                }
            }
            self.constants.update(context, &resolve);
            context.CSSetUnorderedAccessViews(0, 3, Some([None, None, Some(output_uav.clone())].as_ptr()), None);
            // The primary's depth, for asking its eye about the fill.
            let resources = [
                None,
                Some(primary.depth.view.clone()),
                Some(layers[0].image_srv.clone()),
                Some(layers[1].image_srv.clone()),
                Some(layers[0].keys_srv.clone()),
                Some(layers[1].keys_srv.clone()),
            ];
            context.CSSetShaderResources(0, Some(&resources));
            context.CSSetShader(&self.resolve, None);
            let (gx, gy) = groups(view.width, view.height);
            context.Dispatch(gx, gy, 1);
            context.CSSetUnorderedAccessViews(0, 3, Some([None, None, None].as_ptr()), None);
            // The view's depth from the same keys, at its own size (`reproject.hlsl`'s ResolveDepth).
            if let (Some((shader, depth_uav)), Some((width, height))) = (depth, depth_size) {
                context.CSSetUnorderedAccessViews(3, 1, Some([Some(depth_uav.clone())].as_ptr()), None);
                context.CSSetShader(shader, None);
                let (dx, dy) = groups(width, height);
                context.Dispatch(dx, dy, 1);
                context.CSSetUnorderedAccessViews(3, 1, Some(slot3.as_ptr()), None);
            }
            context.CSSetShaderResources(0, Some(&[None, None, None, None, None, None]));
        }
        saved.restore(context);
        // The layer textures are only reached through their views.
        let _ = (&layers[0].keys, &layers[0].image);
        Ok(())
    }
}
