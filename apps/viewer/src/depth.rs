//! Each eye's depth beside its image, for the runtime's reprojection (`XR_KHR_composition_layer_depth`):
//! the producer's depth, as the channel reader copied it, goes into a depth swapchain per eye and
//! rides on the projection layer's views. Only pairs with poses carry it; without the extension, or
//! without depth from the producer, the layer is colour only.

use monaka_channel::d3d;
use monaka_channel::fence::ReadDepth;
use monaka_core::protocol::dxgi_format;
use monaka_openxr::{DepthView, Session, Swapchain};
use windows::Win32::Graphics::Direct3D11::{ID3D11DeviceContext, ID3D11Texture2D};

/// The depth swapchain format the channel's depth (typeless R32 or R16) is copied into: the depth
/// format of its family, if the runtime offers it.
pub fn swapchain_format(offered: &[i64], channel_format: u32) -> Option<u32> {
    let format = match channel_format {
        dxgi_format::R32_TYPELESS => dxgi_format::D32_FLOAT,
        dxgi_format::R16_TYPELESS => dxgi_format::D16_UNORM,
        _ => return None,
    };
    offered.contains(&(format as i64)).then_some(format)
}

/// A distance for the log: metres, or infinite.
fn metres(distance: f32) -> String {
    if distance.is_finite() { format!("{distance} m") } else { "infinite".into() }
}

/// The depth swapchains for one shape of the producer's depth (`None` when they could not be made:
/// that shape goes colour only).
pub struct DepthChains<'s> {
    shape: (u32, u32, u32),
    chains: Option<[Swapchain<'s>; 2]>,
}

impl<'s> DepthChains<'s> {
    /// Swapchains for `depth`'s format and size, reporting what it is.
    pub fn new(session: &'s Session<'_>, offered: &[i64], depth: &ReadDepth) -> Self {
        let shape = (depth.format, depth.width, depth.height);
        let made = swapchain_format(offered, depth.format).ok_or_else(|| format!("the runtime has no depth format for the game's (format {})", depth.format)).and_then(|format| {
            let chain = || session.create_depth_swapchain(format as i64, depth.width, depth.height).map_err(|e| e.to_string());
            Ok([chain()?, chain()?])
        });
        let chains = match made {
            Ok(chains) => {
                let p = depth.params;
                let reversed = if p.reversed != 0 { ", reversed" } else { "" };
                println!("Depth to the runtime: {}x{} format {}, near {}, far {}{reversed}", depth.width, depth.height, depth.format, metres(p.near), metres(p.far));
                Some(chains)
            }
            Err(why) => {
                println!("No depth to the runtime: {why}; colour only");
                None
            }
        };
        Self { shape, chains }
    }

    /// Whether these swapchains are for `depth`'s format and size.
    pub fn fits(&self, depth: &ReadDepth) -> bool {
        self.shape == (depth.format, depth.width, depth.height)
    }

    /// Copies each eye's depth (`sources`, the reader's copies) into its swapchain; the views to
    /// put on the layer's eyes, or `None` (colour only) when there are no swapchains or a fill
    /// fails.
    pub fn fill(&self, context: &ID3D11DeviceContext, sources: [&ID3D11Texture2D; 2], depth: &ReadDepth) -> Option<[DepthView<'_>; 2]> {
        let chains = self.chains.as_ref()?;
        for (chain, source) in chains.iter().zip(sources) {
            chain
                .fill(|image| {
                    d3d::copy_texture(context, image, source);
                    d3d::flush(context);
                })
                .ok()?;
        }
        let (near_z, far_z) = depth.params.distances_at_0_and_1();
        Some([0, 1].map(|eye| DepthView { swapchain: &chains[eye], rect: depth.rects[eye], near_z, far_z }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_goes_to_the_depth_format_of_its_family() {
        use dxgi_format::*;
        let offered = [R8G8B8A8_UNORM_SRGB as i64, D32_FLOAT as i64, D16_UNORM as i64, D24_UNORM_S8_UINT as i64];
        assert_eq!(swapchain_format(&offered, R32_TYPELESS), Some(D32_FLOAT));
        assert_eq!(swapchain_format(&offered, R16_TYPELESS), Some(D16_UNORM));
        assert_eq!(swapchain_format(&[D24_UNORM_S8_UINT as i64], R32_TYPELESS), None, "no copy across families");
        assert_eq!(swapchain_format(&offered, R8G8B8A8_UNORM), None);
    }

    #[test]
    fn distances_read_as_metres() {
        assert_eq!(metres(0.05), "0.05 m");
        assert_eq!(metres(f32::INFINITY), "infinite");
    }
}
