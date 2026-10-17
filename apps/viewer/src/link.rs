//! The viewer's link to the game's producer: the stereo channel, the eye images' textures and
//! swapchains (the producer's size and format), the HUD panels on the hands and the debug views.
//! Everything else (the headset session, the controllers, the head and hand blocks, Monaka's home)
//! outlives it: when the player applies settings in the home, the link is let go, so the
//! producer's channel is free for the one that replaces it, and made again once that one
//! publishes ([`Link::again`]), the headset showing the home all the while.

use crate::{Args, console_feed, debug_views, depth, panel, pose_channel, side_channel, swapchain_format};
use monaka_channel::ChannelName;
use monaka_channel::fingers::FingerChannel;
use monaka_channel::hands::HandChannel;
use monaka_channel::pose::PoseChannel;
use monaka_channel::reticles::ReticleChannel;
use monaka_channel::d3d::{create_texture, simple_texture_desc};
use monaka_channel::fence::ReadDepth;
use monaka_channel::reader::{self, Description, FenceReader};
use monaka_contract::protocol::RenderedPair;
use monaka_openxr::{Posef, Session, Swapchain};
use std::error::Error;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_SHADER_RESOURCE, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

pub struct Link<'s> {
    /// The channel's name: the run's, or the one the launcher gave a producer started again.
    pub name: ChannelName,
    pub producer: Description,
    pub input: FenceReader,
    pub swapchains: [Swapchain<'s>; 2],
    /// The eye swapchains' format.
    pub format: DXGI_FORMAT,
    /// The newest pair, copied out of the channel.
    pub sources: [ID3D11Texture2D; 2],
    pub hand_panels: panel::HandPanels<'s>,
    pub debug_views: debug_views::DebugViews<'s>,
    /// The newest pair's sequence.
    pub seen: u64,
    /// The newest pair's depth, read only when the runtime takes depth, and its swapchains.
    pub depth_chains: Option<depth::DepthChains<'s>>,
    pub pair_depth: Option<ReadDepth>,
    /// The poses the newest pair was rendered with (none: a flat screen's image).
    pub rendered: Option<RenderedPair>,
    /// Where the flat screen hangs while pairs come without poses.
    pub screen: Option<Posef>,
    pub image_aspect: f32,
    /// Whether the log said the depth block would not open.
    pub depth_refused_said: bool,
    /// The blocks beside the channel: the head (in) and the poses of each pair (out), the hands and
    /// the pad, the fingers, where the reticles point, and the probe console.
    pub poses: Option<PoseChannel>,
    pub hands_out: Option<HandChannel>,
    pub fingers_out: Option<FingerChannel>,
    pub reticle_aims: Option<ReticleChannel>,
    pub console_feed: console_feed::ConsoleFeed,
}

/// Which of the blocks beside the channel the viewer writes: the hands with controllers, the
/// fingers where the headset tracks them.
#[derive(Clone, Copy)]
pub struct Wants {
    pub hands: bool,
    pub fingers: bool,
}

impl<'s> Link<'s> {
    /// The link to the producer that publishes `producer` on channel `name`, read through `input`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(name: &ChannelName, producer: Description, input: FenceReader, session: &'s Session<'_>, device: &ID3D11Device, offered: &[i64], args: &Args, wants: Wants) -> Result<Self, Box<dyn Error>> {
        let poses = pose_channel(PoseChannel::open(name))?;
        let hands_out = if wants.hands { side_channel("hand channel", HandChannel::open(name)) } else { None };
        let fingers_out = if wants.fingers { side_channel("finger channel", FingerChannel::open(name)) } else { None };
        // Where the producer says a reticle points, when not along the palm (a bow at an angle in it).
        let reticle_aims = if args.reticles.contains(&true) { side_channel("reticle channel", ReticleChannel::open(name)) } else { None };
        let format = swapchain_format(offered, &producer).ok_or("runtime has no image format matching the game's")?;
        let swapchains = [
            debug_views::eye_swapchain(session, format as i64, producer.width, producer.height)?,
            debug_views::eye_swapchain(session, format as i64, producer.width, producer.height)?,
        ];
        // HUD pieces the game publishes on their own, on the hands where it says.
        let declared = side_channel("HUD panels block", monaka_channel::panels::PanelsChannel::open(name));
        let hand_panels = panel::HandPanels::new(name, format, declared);
        let mut desc = simple_texture_desc(producer.width, producer.height, DXGI_FORMAT(producer.format as i32));
        desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
        let sources = [create_texture(device, &desc)?, create_texture(device, &desc)?];
        let debug_views = debug_views::DebugViews::new(name, DXGI_FORMAT(format as i32), args.captures.clone());
        let image_aspect = producer.height as f32 / (producer.width as f32 * args.pixel_aspect);
        Ok(Self {
            name: name.clone(),
            producer,
            input,
            swapchains,
            format: DXGI_FORMAT(format as i32),
            sources,
            hand_panels,
            debug_views,
            seen: 0,
            depth_chains: None,
            pair_depth: None,
            rendered: None,
            screen: None,
            image_aspect,
            depth_refused_said: false,
            poses,
            hands_out,
            fingers_out,
            reticle_aims,
            console_feed: console_feed::ConsoleFeed::new(name),
        })
    }

    /// Waits up to `seconds` for the producer's first pair.
    pub fn first_pair(&mut self, context: &ID3D11DeviceContext, seconds: u64) -> Result<(), String> {
        let ready_by = Instant::now() + Duration::from_secs(seconds);
        loop {
            let eyes = [&self.sources[0], &self.sources[1]];
            if let Some(pair) = self.input.read(context, eyes, 0).map_err(|e| e.to_string())? {
                self.seen = pair.sequence;
                return Ok(());
            }
            if Instant::now() >= ready_by {
                return Err("the producer has not published a stereo pair".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The link made again once the producer that replaces the last one publishes on `name`:
    /// `None` while it is not there yet (asked again later), an error when it is there but cannot
    /// be read (another version of the channel, or no image format for it).
    pub fn again(name: &ChannelName, session: &'s Session<'_>, device: &ID3D11Device, offered: &[i64], args: &Args, wants: Wants) -> Option<Result<Self, String>> {
        let producer = match reader::describe_any(name)? {
            Ok(producer) => producer,
            Err(mismatch) => return Some(Err(mismatch.to_string())),
        };
        let input = FenceReader::open(name, device).ok()?;
        Some(Self::new(name, producer, input, session, device, offered, args, wants).map_err(|e| e.to_string()))
    }
}
