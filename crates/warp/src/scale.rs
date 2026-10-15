//! An image resampled to another size (`scale.hlsl`), its aspect kept: a game's flat frames (a
//! movie, a loading screen) rendered smaller than the eye size, sent at the eye size.

use crate::{ConstantBuffer, Readable, Result, SavedCompute, color_formats, compute_shader, readable, uav_texture};
use monaka_channel::d3d;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::core::s;

pub const SCALE_SOURCE: &str = include_str!("scale.hlsl");

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    rect: [f32; 4],
}

pub struct Scaler {
    device: ID3D11Device,
    shader: ID3D11ComputeShader,
    constants: ConstantBuffer<Constants>,
    source: Option<Readable>,
    output: Option<(ID3D11Texture2D, ID3D11UnorderedAccessView)>,
}

/// Where a `source`-sized image goes in a `size` output: as large as fits, centred, aspect kept.
pub fn fit(source: (u32, u32), size: (u32, u32)) -> [f32; 4] {
    let (sw, sh) = (source.0.max(1) as f32, source.1.max(1) as f32);
    let (w, h) = (size.0 as f32, size.1 as f32);
    let scale = (w / sw).min(h / sh);
    let (fw, fh) = (sw * scale, sh * scale);
    [(w - fw) / 2.0, (h - fh) / 2.0, fw, fh]
}

impl Scaler {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        Ok(Self {
            device: device.clone(),
            shader: compute_shader(device, SCALE_SOURCE, "scale.hlsl", s!("Scale"))?,
            constants: ConstantBuffer::new(device)?,
            source: None,
            output: None,
        })
    }

    /// `source` (an RGBA8, BGRA8 or R10G10B10A2 image, any binding: a back buffer) resampled to
    /// `width` x `height` with its aspect kept, black around it, into an RGBA8 image kept here.
    pub fn run(&mut self, context: &ID3D11DeviceContext, source: &ID3D11Texture2D, size: (u32, u32)) -> Result<&ID3D11Texture2D> {
        let desc = d3d::texture_desc(source);
        self.run_into(context, source, size, fit((desc.Width, desc.Height), size))
    }

    /// `source` resampled into `rect` (x, y, width, height in output pixels) of a `width` x
    /// `height` output, black around it: an image drawn stretched to another shape put back in
    /// its own.
    pub fn run_into(&mut self, context: &ID3D11DeviceContext, source: &ID3D11Texture2D, (width, height): (u32, u32), rect: [f32; 4]) -> Result<&ID3D11Texture2D> {
        let desc = d3d::texture_desc(source);
        if desc.SampleDesc.Count != 1 {
            return Err(crate::Error::Unsupported("multisampled image"));
        }
        let formats = color_formats(desc.Format).ok_or(crate::Error::Unsupported("image format"))?;
        let copy = readable(&self.device, self.source.take(), &desc, formats)?;
        let resized = self.output.as_ref().is_none_or(|(t, _)| {
            let d = d3d::texture_desc(t);
            (d.Width, d.Height) != (width, height)
        });
        if resized {
            self.output = Some(uav_texture(&self.device, width, height, DXGI_FORMAT_R8G8B8A8_UNORM)?);
        }
        let (output, uav) = self.output.as_ref().expect("made above");
        let constants = Constants { width, height, source_width: desc.Width, source_height: desc.Height, rect };
        let saved = SavedCompute::take(context);
        self.constants.update(context, &constants);
        self.constants.bind(context);
        // SAFETY: copies the image into our same-shaped texture, binds our shader and views on the
        // game's context, dispatches once, unbinds and restores what was bound.
        unsafe {
            context.CopyResource(&copy.texture, source);
            context.CSSetShaderResources(0, Some(&[Some(copy.view.clone())]));
            context.CSSetUnorderedAccessViews(0, 1, Some([Some(uav.clone())].as_ptr()), None);
            context.CSSetShader(&self.shader, None);
            context.Dispatch(width.div_ceil(8), height.div_ceil(8), 1);
            context.CSSetUnorderedAccessViews(0, 1, Some([None].as_ptr()), None);
            context.CSSetShaderResources(0, Some(&[None]));
        }
        saved.restore(context);
        self.source = Some(copy);
        Ok(output)
    }
}

#[cfg(test)]
mod tests;
