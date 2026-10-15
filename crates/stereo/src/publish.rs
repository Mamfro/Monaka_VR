//! What every publisher does around one pair: the pose record first, then the images, then the
//! tally; and the private copies a D3D11 publisher keeps of its frames.

use crate::alternate::Step;
use monaka_channel::d3d;
use monaka_channel::pose::HeadSource;
use monaka_contract::protocol::HeadPose;
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

/// The poses of a flat frame: none. The viewer shows a pair whose record has an eye without a valid
/// pose on its flat screen. The record still goes out: a channel made again starts its sequences
/// over, and a record left in the ring from before must not be taken for this frame's.
pub(crate) const FLAT: [HeadPose; 2] = [HeadPose::EMPTY; 2];

/// Writes the pose record of the pair the channel publishes next, as `sequence`, before its images:
/// a viewer that sees the images always finds their poses. `report` adjusts each eye that has a
/// pose (the field of view actually rendered). The poses written.
pub(crate) fn record_first(head: &HeadSource, sequence: u64, poses: [HeadPose; 2], report: impl Fn(&mut HeadPose)) -> [HeadPose; 2] {
    let mut poses = poses;
    poses.iter_mut().filter(|p| p.valid != 0).for_each(report);
    head.write_pair(sequence, poses);
    poses
}

/// Pairs published and failures, for the summaries.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tally {
    pub pairs: u64,
    pub failures: u64,
    /// Pairs published with each eye's depth.
    pub with_depth: u64,
}

impl Tally {
    pub const NEW: Tally = Tally { pairs: 0, failures: 0, with_depth: 0 };

    pub fn failed(&mut self) -> Step {
        self.failures += 1;
        Step::Failed
    }

    /// Counts a channel's publish: published, or no slot free (a failure). An error goes back to
    /// the caller, which counts it its own way.
    pub fn count(&mut self, published: monaka_channel::Result<bool>) -> monaka_channel::Result<Step> {
        match published? {
            true => {
                self.pairs += 1;
                Ok(Step::Published)
            }
            false => Ok(self.failed()),
        }
    }
}

/// Copies `source` into `copy`, a private texture made from `desc` when there is none or its size
/// is not `desc`'s. The copy, or none when it cannot be made.
pub(crate) fn copy_into<'a>(
    copy: &'a mut Option<ID3D11Texture2D>,
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    source: &ID3D11Texture2D,
    desc: &D3D11_TEXTURE2D_DESC,
) -> Option<&'a ID3D11Texture2D> {
    if copy.as_ref().is_none_or(|c| {
        let made = d3d::texture_desc(c);
        (made.Width, made.Height) != (desc.Width, desc.Height)
    }) {
        *copy = d3d::create_texture(device, desc).ok();
    }
    let target = copy.as_ref()?;
    // SAFETY: copies the image into a texture of its shape and format family on its own device.
    unsafe { context.CopyResource(target, source) };
    Some(target)
}

#[cfg(test)]
mod tests;
