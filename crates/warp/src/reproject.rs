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
use monaka_core::depth::{DepthMapping, reprojection};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32_UINT};
use windows::core::s;

#[cfg(test)]
mod lift_checks;
#[cfg(test)]
mod tests;

/// The shader on D3D11: its images written as UNORM (the views' own format, as D3D11 checks).
pub const REPROJECT_SOURCE: &str = concat!("#define IMAGE unorm float4\n", include_str!("reproject.hlsl"));

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
    pub pad: [u32; 2],
}

const _: () = assert!(size_of::<Constants>() == 32 * 4);

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
    /// `origin` in the output texture: the constants of its clears and resolves.
    pub(crate) fn redraw(view: &View, out_depth: Option<(u32, u32)>, origin: [u32; 2]) -> Self {
        let (out_depth_width, out_depth_height) = out_depth.unwrap_or((0, 0));
        Self { width: view.width, height: view.height, out_depth_width, out_depth_height, near_a: view.mapping.a, near_b: view.mapping.b, depth_origin: origin, ..Default::default() }
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
    device: ID3D11Device,
    clear: ID3D11ComputeShader,
    splat: ID3D11ComputeShader,
    paint: ID3D11ComputeShader,
    resolve: ID3D11ComputeShader,
    constants: ConstantBuffer<Constants>,
    layers: Option<[Layer; 2]>,
    output: Option<(ID3D11Texture2D, ID3D11UnorderedAccessView)>,
    size: (u32, u32),
}

impl Reprojector {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let shader = |entry| compute_shader(device, REPROJECT_SOURCE, "reproject.hlsl", entry);
        Ok(Self {
            device: device.clone(),
            clear: shader(s!("Clear"))?,
            splat: shader(s!("Splat"))?,
            paint: shader(s!("Paint"))?,
            resolve: shader(s!("Resolve"))?,
            constants: ConstantBuffer::new(device)?,
            layers: None,
            output: None,
            size: (0, 0),
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
        Ok(Frame { color: color_copy, depth: depth_copy, projection, camera })
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
        self.targets(view.width, view.height)?;
        let layers = self.layers.as_ref().expect("made above");
        let (output, output_uav) = self.output.as_ref().expect("made above");
        let groups = |w: u32, h: u32| (w.div_ceil(8), h.div_ceil(8));
        let saved = SavedCompute::take(context);
        self.constants.bind(context);
        let redraw = Constants::redraw(view, None, [0, 0]);
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
                let sizes = ((frame.color.width, frame.color.height), ([0, 0], (frame.depth.width, frame.depth.height)));
                let Some(splat) = redraw.splat(view, sizes.0, sizes.1, [0.0; 2], &frame.projection, &frame.camera) else { continue };
                self.constants.update(context, &splat);
                context.CSSetShaderResources(0, Some(&[Some(frame.color.view.clone()), Some(frame.depth.view.clone())]));
                let (sx, sy) = groups(frame.color.width, frame.color.height);
                for shader in [&self.splat, &self.paint] {
                    context.CSSetShader(shader, None);
                    context.Dispatch(sx, sy, 1);
                }
            }
            self.constants.update(context, &redraw);
            context.CSSetUnorderedAccessViews(0, 3, Some([None, None, Some(output_uav.clone())].as_ptr()), None);
            let resources =
                [None, None, Some(layers[0].image_srv.clone()), Some(layers[1].image_srv.clone()), Some(layers[0].keys_srv.clone()), Some(layers[1].keys_srv.clone())];
            context.CSSetShaderResources(0, Some(&resources));
            context.CSSetShader(&self.resolve, None);
            let (gx, gy) = groups(view.width, view.height);
            context.Dispatch(gx, gy, 1);
            context.CSSetUnorderedAccessViews(0, 3, Some([None, None, None].as_ptr()), None);
            context.CSSetShaderResources(0, Some(&[None, None, None, None, None, None]));
        }
        saved.restore(context);
        // The layer textures are only reached through their views.
        let _ = (&layers[0].keys, &layers[0].image);
        Ok(output)
    }
}
