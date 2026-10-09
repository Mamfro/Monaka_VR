//! Rendered frames redrawn from another camera (`reproject.hlsl`). For alternate-eye rendering
//! with depth: each presented frame is one real eye; the other eye of the same moment is this
//! frame reprojected (the primary), and what this frame could not see is filled from that eye's own
//! previous real frame reprojected to now (the fill), so gaps get rendered pixels, not stretched
//! ones.

use crate::{ConstantBuffer, Error, Readable, Result, SavedCompute, color_formats, compute_shader, depth_formats, readable, shader_view, uav_texture};
use monaka_channel::d3d;
use monaka_core::camera::{Mat34, Mat44};
use monaka_core::depth::{DepthMapping, reprojection};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32_UINT};
use windows::core::s;

pub const REPROJECT_SOURCE: &str = include_str!("reproject.hlsl");

/// The layout of the HLSL `cbuffer Reproject`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    m: [f32; 16],
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    depth_width: u32,
    depth_height: u32,
    near_a: f32,
    near_b: f32,
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
        // SAFETY: binds our own shaders and views on the game's context, dispatches, and restores
        // what was bound.
        unsafe {
            for (layer, frame) in layers.iter().zip([Some(primary), fill]) {
                let uavs = [Some(layer.keys_uav.clone()), Some(layer.image_uav.clone())];
                context.CSSetUnorderedAccessViews(0, 2, Some(uavs.as_ptr()), None);
                let mut constants = Constants { width: view.width, height: view.height, near_a: view.mapping.a, near_b: view.mapping.b, ..Default::default() };
                self.constants.update(context, &constants);
                let (gx, gy) = groups(view.width, view.height);
                context.CSSetShader(&self.clear, None);
                context.Dispatch(gx, gy, 1);
                let Some(frame) = frame else { continue };
                let Some(m) = reprojection(&frame.projection, &frame.camera, &view.projection, &view.camera) else { continue };
                constants.m = m;
                (constants.source_width, constants.source_height) = (frame.color.width, frame.color.height);
                (constants.depth_width, constants.depth_height) = (frame.depth.width, frame.depth.height);
                self.constants.update(context, &constants);
                context.CSSetShaderResources(0, Some(&[Some(frame.color.view.clone()), Some(frame.depth.view.clone())]));
                let (sx, sy) = groups(frame.color.width, frame.color.height);
                for shader in [&self.splat, &self.paint] {
                    context.CSSetShader(shader, None);
                    context.Dispatch(sx, sy, 1);
                }
            }
            context.CSSetUnorderedAccessViews(0, 2, Some([None, Some(output_uav.clone())].as_ptr()), None);
            let resources =
                [None, None, Some(layers[0].image_srv.clone()), Some(layers[1].image_srv.clone()), Some(layers[0].keys_srv.clone()), Some(layers[1].keys_srv.clone())];
            context.CSSetShaderResources(0, Some(&resources));
            context.CSSetShader(&self.resolve, None);
            let (gx, gy) = groups(view.width, view.height);
            context.Dispatch(gx, gy, 1);
            context.CSSetUnorderedAccessViews(0, 2, Some([None, None].as_ptr()), None);
            context.CSSetShaderResources(0, Some(&[None, None, None, None, None, None]));
        }
        saved.restore(context);
        // The layer textures are only reached through their views.
        let _ = (&layers[0].keys, &layers[0].image);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_channel::d3d::{create_device, create_texture_with, read_pixel};
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_FLOAT};

    fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bytes: &[u8]) -> ID3D11Texture2D {
        create_texture_with(device, &D3D11_TEXTURE2D_DESC { BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..d3d::simple_texture_desc(w, h, format) }, bytes).unwrap()
    }

    #[test]
    fn shaders_compile() {
        for entry in [s!("Clear"), s!("Splat"), s!("Paint"), s!("Resolve")] {
            crate::compile(REPROJECT_SOURCE, "reproject.hlsl", entry).unwrap();
        }
    }

    #[test]
    fn other_eye_from_this_frame_and_gaps_from_its_own_last_frame() {
        let (device, context, _) = create_device(None).unwrap();
        let mut reprojector = Reprojector::new(&device).unwrap();
        let (w, h) = (64u32, 8u32);
        // A standard-depth projection with P00 = P11 = 1 and a wall 2 m away everywhere.
        let (n, f) = (0.1f32, 100.0f32);
        let mut projection = [0.0f32; 16];
        projection[0] = 1.0;
        projection[5] = 1.0;
        projection[10] = f / (n - f);
        projection[11] = n * f / (n - f);
        projection[14] = -1.0;
        let wall = -projection[10] + projection[11] / 2.0;
        let depth: Vec<u8> = (0..w * h).flat_map(|_| wall.to_le_bytes()).collect();
        let depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, &depth);
        let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
        // This frame: the left camera, red = 4 * column. The right eye's last frame: plain green.
        let columns: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| [(x * 4) as u8, 0, 0, 255])).collect();
        let green: Vec<u8> = (0..w * h).flat_map(|_| [0, 255, 0, 255]).collect();
        let left = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &columns);
        let last_right = texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, &green);
        let primary = reprojector.capture(&context, &left, &depth, projection, camera(-0.5), None).unwrap();
        let fill = reprojector.capture(&context, &last_right, &depth, projection, camera(0.5), None).unwrap();
        let mapping = DepthMapping::from_projection(&projection, n).unwrap();
        let view = View { projection, camera: camera(0.5), mapping, width: w, height: h };

        // Cameras 1 m apart, wall 2 m away: the right eye sees each column 16 pixels further left.
        let image = reprojector.run(&context, &view, &primary, Some(&fill)).unwrap().clone();
        assert_eq!(read_pixel(&device, &context, &image, 20, 3).unwrap(), [36 * 4, 0, 0, 255]);
        // The right edge was out of the left camera's view: its own last frame fills it.
        assert_eq!(read_pixel(&device, &context, &image, 55, 3).unwrap(), [0, 255, 0, 255]);
        // Without a fill frame, the gap continues its neighbour instead.
        let image = reprojector.run(&context, &view, &primary, None).unwrap().clone();
        assert_eq!(read_pixel(&device, &context, &image, 55, 3).unwrap(), [63 * 4, 0, 0, 255]);
    }
}
