//! Motion vectors for temporal upscaling (`motion.hlsl`): from a frame's depth and the camera it
//! was rendered from against the same eye's previous camera, plus the game's own object motion
//! when it writes one. The game crate gives both cameras in canonical form with the projections the
//! depth buffer really uses (unjittered).

use crate::{ConstantBuffer, Error, Readable, Result, SavedCompute, compute_shader, depth_formats, readable, shader_view};
use monaka_channel::d3d;
use monaka_core::camera::{Mat34, Mat44};
use monaka_core::depth::reprojection;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16_FLOAT;
use windows::core::{Interface, s};

pub const MOTION_SOURCE: &str = include_str!("motion.hlsl");

/// The layout of the HLSL `cbuffer Motion`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    m: [f32; 16],
    width: u32,
    height: u32,
    depth_width: u32,
    depth_height: u32,
    object_scale: [f32; 2],
    has_object: u32,
    pad: u32,
}

/// One eye's camera for a frame: canonical camera-to-world and the depth buffer's projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeView {
    pub camera: Mat34,
    pub projection: Mat44,
}

/// The game's object motion: its texture (R16G16_FLOAT, render size) and the scale to UV units.
pub struct ObjectMotion<'a> {
    pub texture: &'a ID3D11Texture2D,
    pub scale: [f32; 2],
}

pub struct MotionPass {
    device: ID3D11Device,
    shader: ID3D11ComputeShader,
    constants: ConstantBuffer<Constants>,
    depth: Option<Readable>,
    /// A view of the game's object-motion texture, kept while it is the same texture.
    object: Option<(usize, ID3D11ShaderResourceView)>,
}

impl MotionPass {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        Ok(Self {
            device: device.clone(),
            shader: compute_shader(device, MOTION_SOURCE, "motion.hlsl", s!("Main"))?,
            constants: ConstantBuffer::new(device)?,
            depth: None,
            object: None,
        })
    }

    fn object_view(&mut self, texture: &ID3D11Texture2D) -> Option<ID3D11ShaderResourceView> {
        let key = texture.as_raw() as usize;
        if let Some((known, view)) = &self.object
            && *known == key
        {
            return Some(view.clone());
        }
        let desc = d3d::texture_desc(texture);
        if desc.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32 == 0 {
            return None;
        }
        // The game's live texture allows shader reads (checked above).
        let view = shader_view(&self.device, texture, DXGI_FORMAT_R16G16_FLOAT).ok()?;
        self.object = Some((key, view.clone()));
        Some(view)
    }

    /// Writes this frame's motion vectors (`motion`, an R16G16_FLOAT view) and depth (`depth_out`,
    /// R32_FLOAT) at `width` x `height` from the game's `depth` texture. Without a `previous` view
    /// (the eye's first frame) the camera adds no motion.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        context: &ID3D11DeviceContext,
        depth: &ID3D11Texture2D,
        current: &EyeView,
        previous: Option<&EyeView>,
        object: Option<ObjectMotion<'_>>,
        motion: &ID3D11UnorderedAccessView,
        depth_out: &ID3D11UnorderedAccessView,
        (width, height): (u32, u32),
    ) -> Result<()> {
        let depth_desc = d3d::texture_desc(depth);
        if depth_desc.SampleDesc.Count != 1 {
            return Err(Error::Unsupported("multisampled depth"));
        }
        let copy = readable(&self.device, self.depth.take(), &depth_desc, depth_formats(depth_desc.Format).ok_or(Error::Unsupported("depth format"))?)?;
        let m = match previous {
            Some(previous) => reprojection(&current.projection, &current.camera, &previous.projection, &previous.camera).ok_or(Error::Unsupported("singular camera"))?,
            None => reprojection(&current.projection, &current.camera, &current.projection, &current.camera).ok_or(Error::Unsupported("singular camera"))?,
        };
        let object_view = object.as_ref().and_then(|o| self.object_view(o.texture));
        let constants = Constants {
            m,
            width,
            height,
            depth_width: depth_desc.Width,
            depth_height: depth_desc.Height,
            object_scale: object.as_ref().map_or([0.0; 2], |o| o.scale),
            has_object: u32::from(object_view.is_some()),
            pad: 0,
        };
        let saved = SavedCompute::take(context);
        // SAFETY: copies the game's depth into our same-shaped typeless copy.
        unsafe { context.CopyResource(&copy.texture, depth) };
        self.constants.update(context, &constants);
        self.constants.bind(context);
        // SAFETY: runs our shader on the game's context with our own resources; the game's compute
        // bindings go back after.
        unsafe {
            context.CSSetShader(&self.shader, None);
            context.CSSetShaderResources(0, Some(&[Some(copy.view.clone()), object_view]));
            context.CSSetUnorderedAccessViews(0, 2, Some([Some(motion.clone()), Some(depth_out.clone())].as_ptr()), None);
            context.Dispatch(width.div_ceil(8), height.div_ceil(8), 1);
            context.CSSetUnorderedAccessViews(0, 2, Some([None, None].as_ptr()), None);
            context.CSSetShaderResources(0, Some(&[None, None]));
        }
        saved.restore(context);
        self.depth = Some(copy);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_channel::d3d::{create_device, create_texture, create_texture_with, read_image};
    use monaka_core::half;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_FLOAT};

    fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bind: D3D11_BIND_FLAG, bytes: Option<&[u8]>) -> ID3D11Texture2D {
        let desc = D3D11_TEXTURE2D_DESC { BindFlags: bind.0 as u32, ..d3d::simple_texture_desc(w, h, format) };
        match bytes {
            Some(bytes) => create_texture_with(device, &desc, bytes),
            None => create_texture(device, &desc),
        }
        .unwrap()
    }

    fn uav(device: &ID3D11Device, texture: &ID3D11Texture2D) -> ID3D11UnorderedAccessView {
        let mut view = None;
        // SAFETY: test setup on our own texture.
        unsafe { device.CreateUnorderedAccessView(texture, None, Some(&mut view)).unwrap() };
        view.unwrap()
    }

    #[test]
    fn shader_compiles() {
        crate::compile(MOTION_SOURCE, "motion.hlsl", s!("Main")).unwrap();
    }

    /// The camera steps 0.5 m right between frames, a wall 2 m ahead fills the left half and the
    /// sky (reversed infinite depth: 0) the right half. The wall was a quarter of the view further
    /// right last frame (1/8 in UV); the sky did not move.
    #[test]
    fn a_sideways_step_moves_the_wall_and_not_the_sky() {
        let (device, context, _) = create_device(None).unwrap();
        let mut pass = MotionPass::new(&device).unwrap();
        let (w, h) = (16u32, 4u32);
        let near = 0.1f32;
        let mut projection = [0.0f32; 16];
        projection[0] = 1.0;
        projection[5] = 1.0;
        projection[11] = near;
        projection[14] = -1.0;
        let wall = near / 2.0;
        let depth: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(move |x| if x < w / 2 { wall } else { 0.0 }.to_le_bytes())).collect();
        let depth = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, D3D11_BIND_SHADER_RESOURCE, Some(&depth));
        let motion = texture(&device, w, h, DXGI_FORMAT_R16G16_FLOAT, D3D11_BIND_UNORDERED_ACCESS, None);
        let depth_out = texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, D3D11_BIND_UNORDERED_ACCESS, None);
        let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
        let current = EyeView { camera: camera(0.5), projection };
        let previous = EyeView { camera: camera(0.0), projection };
        pass.run(&context, &depth, &current, Some(&previous), None, &uav(&device, &motion), &uav(&device, &depth_out), (w, h)).unwrap();
        let (_, _, texels) = read_image(&device, &context, &motion).unwrap();
        let vector = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [half::to_f32(u16::from_le_bytes([texels[i], texels[i + 1]])), half::to_f32(u16::from_le_bytes([texels[i + 2], texels[i + 3]]))]
        };
        let [wx, wy] = vector(3, 2);
        assert!((wx - 0.125).abs() < 1e-3 && wy.abs() < 1e-3, "wall moved by {wx}, {wy}");
        let [sx, sy] = vector(12, 2);
        assert!(sx.abs() < 1e-3 && sy.abs() < 1e-3, "sky moved by {sx}, {sy}");
        let (_, _, depths) = read_image(&device, &context, &depth_out).unwrap();
        assert_eq!(f32::from_le_bytes([depths[8], depths[9], depths[10], depths[11]]), wall);
    }
}
