//! HUD pieces the game publishes on their own (`<channel>-panel-<piece>`, premultiplied RGBA),
//! shown on the hands: each a quad in its controller's grip space, so the runtime moves it with
//! the hand at the headset's own rate, on the back of the hand or above the palm. The producer
//! declares its pieces and where each goes ([`monaka_contract::panels`]); the declaration and each
//! piece's channel are looked for now and then until they appear.

use monaka_channel::d3d::{self, create_texture, simple_texture_desc};
use monaka_channel::panels::PanelsChannel;
use monaka_channel::reader::{self, FenceReader};
use monaka_channel::ChannelName;
use monaka_contract::panels::{Place, Side};
use monaka_core::math::{Quat, Vec3, add, conjugate, from_axes, multiply, rotate, sub};
use monaka_contract::protocol::{HandPose, HeadPose};
use monaka_contract::protocol::{RIGHT_HAND, dxgi_format};
use monaka_openxr::{Extent2Df, Layer, Posef, Session, Space, Swapchain};
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_SHADER_RESOURCE, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Frames between looks for the channel while it is not there.
const LOOK_EVERY: u64 = 90;
/// A panel is at least this tall (a wide strip, Dying Light 2's compass, grows wider for it) and
/// at most this wide.
const LEAST_HEIGHT: f32 = 0.035;
const WIDEST: f32 = 0.24;
/// The gap between panels side by side, in metres.
const GAP: f32 = 0.005;
/// Each panel lies flat against its side of the hand, turned in that plane so its top is where
/// the head's up points ([`facing_up`]): it reads upright however the hand is turned. With the
/// head's up across the hand (too short there to say), its top goes toward the knuckles.
const SHORTEST_UP: f32 = 0.15;

/// The way a panel on `side` faces along `hand`'s grip x axis (`LEFT_HAND` or `RIGHT_HAND`). In a
/// grip space -Z runs along the pointing finger and +Y toward the thumb, and +X leaves the hand
/// through the back of the right hand but the palm of the left (OpenXR's grip pose: both point
/// right).
fn facing(side: Side, hand: usize) -> f32 {
    let back = if hand == RIGHT_HAND { 1.0 } else { -1.0 };
    match side {
        Side::BackOfHand => back,
        Side::Palm => -back,
    }
}

/// Where a panel on `side` sits in `hand`'s grip space (metres): a little out from the handle on
/// the back of the hand, just above the palm (the fingers curl round the handle there).
fn offset(side: Side, hand: usize) -> [f32; 3] {
    let out = facing(side, hand);
    match side {
        Side::BackOfHand => [0.055 * out, 0.0, 0.01],
        Side::Palm => [0.04 * out, 0.0, 0.0],
    }
}

/// The hand panels of the pieces the producer declared, made when the declaration appears.
pub struct HandPanels<'s> {
    channel: ChannelName,
    format: u32,
    declared: Option<PanelsChannel>,
    panels: Vec<HandPanel<'s>>,
    frames: u64,
}

impl<'s> HandPanels<'s> {
    /// The panels of game channel `channel`, declared in `declared`, shown in swapchains of `format`
    /// (the eyes' own).
    pub fn new(channel: &ChannelName, format: u32, declared: Option<PanelsChannel>) -> Self {
        Self { channel: channel.clone(), format, declared, panels: Vec::new(), frames: 0 }
    }

    /// Once per frame: the declared pieces made into panels (looked for until they appear), and
    /// each panel updated.
    pub fn update(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext) {
        self.frames += 1;
        if self.panels.is_empty()
            && self.frames % LOOK_EVERY == 1
            && let Some(declared) = self.declared.as_mut()
        {
            self.panels = declared.pieces().into_iter().map(|(piece, place)| HandPanel::new(&self.channel, self.format, piece, place)).collect();
        }
        for panel in &mut self.panels {
            panel.update(session, device, context);
        }
    }

    /// Each panel on its hand ([`HandPanel::layer`]), a panel beside another placed by the main
    /// panel of its hand and side (the first there not beside one).
    pub fn layers<'a>(&'a self, local: &'a Space<'a>, grips: &[HandPose; 2], head: Option<&HeadPose>, nearest: f32) -> impl Iterator<Item = Layer<'a>> + 'a {
        let grips = *grips;
        let head = head.copied();
        self.panels.iter().filter_map(move |panel| {
            let main = self.panels.iter().find(|p| !p.place.beside && p.place.hand == panel.place.hand && p.place.side == panel.place.side);
            panel.layer(local, &grips, head.as_ref(), nearest, main.map_or(0.0, |m| m.shown_size().0))
        })
    }
}

pub struct HandPanel<'s> {
    piece: String,
    place: Place,
    name: Option<ChannelName>,
    format: u32,
    reader: Option<FenceReader>,
    /// The panel as last read (one image: the channel carries one).
    texture: Option<ID3D11Texture2D>,
    swapchain: Option<Swapchain<'s>>,
    size: (u32, u32),
    seen: u64,
    frames: u64,
}

impl<'s> HandPanel<'s> {
    /// The piece `piece` of the game channel `channel` at `place`, shown in swapchains of `format`
    /// (the eyes' own).
    pub fn new(channel: &ChannelName, format: u32, piece: String, place: Place) -> Self {
        // The channel's texels are RGBA8 and are copied as they are.
        let rgba = matches!(format, dxgi_format::R8G8B8A8_UNORM | dxgi_format::R8G8B8A8_UNORM_SRGB);
        let name = channel.panel(&piece).filter(|_| rgba);
        Self { piece, place, name, format, reader: None, texture: None, swapchain: None, size: (0, 0), seen: 0, frames: 0 }
    }

    /// Once per frame: opens the channel when it appears and takes a new panel into the swapchain.
    pub fn update(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext) {
        self.frames += 1;
        if self.reader.is_none() {
            if !self.frames.is_multiple_of(LOOK_EVERY) {
                return;
            }
            let Some(name) = &self.name else { return };
            let Some(Ok(description)) = reader::describe_any(name) else { return };
            let Ok(reader) = FenceReader::open(name, device) else { return };
            let size = (description.width, description.height);
            // The staging texture in the channel's own format (the reader wants an exact match;
            // the eyes' format, sRGB or not, is of the same family, so the copy into the
            // swapchain is allowed).
            let mut desc = simple_texture_desc(size.0, size.1, DXGI_FORMAT(description.format as i32));
            desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
            let Ok(texture) = create_texture(device, &desc) else { return };
            match session.create_swapchain(self.format as i64, size.0, size.1) {
                Ok(swapchain) => {
                    println!("HUD panel {} on the {} hand, {:?} ({}x{})", self.piece, if self.place.hand == RIGHT_HAND { "right" } else { "left" }, self.place.side, size.0, size.1);
                    self.reader = Some(reader);
                    self.texture = Some(texture);
                    self.swapchain = Some(swapchain);
                    self.size = size;
                }
                Err(e) => {
                    println!("No HUD panel {}: {e}", self.piece);
                    self.name = None;
                }
            }
            return;
        }
        let (Some(reader), Some(texture), Some(swapchain)) = (&mut self.reader, &self.texture, &self.swapchain) else { return };
        match reader.read_one(context, texture, self.seen) {
            Ok(Some(pair)) => {
                self.seen = pair.sequence;
                let _ = swapchain.fill(|image| {
                    d3d::copy_texture(context, image, texture);
                });
            }
            Ok(None) => {}
            Err(e) => {
                println!("HUD panel {} lost: {e}", self.piece);
                self.reader = None;
                self.texture = None;
                self.swapchain = None;
                self.seen = 0;
            }
        }
    }

    /// The panel on its hand, once one has arrived; `grips` (by hand) and `head` are the grips'
    /// and the head's poses in `local`, this frame.
    pub fn layer<'a>(&'a self, local: &'a Space<'a>, grips: &[HandPose; 2], head: Option<&HeadPose>, nearest: f32, main_width: f32) -> Option<Layer<'a>> {
        let grip = &grips[self.place.hand];
        if grip.valid == 0 {
            return None;
        }
        let swapchain = self.swapchain.as_ref().filter(|_| self.seen != 0)?;
        let (width, height) = self.shown_size();
        let head_up = match head {
            Some(head) => rotate(conjugate(grip.orientation), rotate(head.orientation, [0.0, 1.0, 0.0])),
            None => [0.0, 0.0, -1.0],
        };
        let orientation = facing_up(head_up, facing(self.place.side, self.place.hand));
        let mut position = offset(self.place.side, self.place.hand);
        if self.place.beside {
            // Moved in the panels' own plane, so the two turn together.
            let other = main_width;
            let shift = rotate(orientation, [-(other * 0.5 + GAP + width * 0.5), other * 0.5 - height * 0.5, 0.0]);
            position = [position[0] + shift[0], position[1] + shift[1], position[2] + shift[2]];
        }
        // Into the local space, and no nearer the head than `nearest`: a game that shows its
        // hands no nearer than that (depth stereo) keeps the panel on the hand it draws, the panel
        // moved out along the line of sight and grown to look the same.
        let orientation = multiply(grip.orientation, orientation);
        let mut position = add(grip.position, rotate(grip.orientation, position));
        let mut scale = 1.0;
        if let Some(head) = head
            && nearest > 0.0
        {
            let line = sub(position, head.position);
            let distance = (line[0] * line[0] + line[1] * line[1] + line[2] * line[2]).sqrt();
            if distance > 0.001 && distance < nearest {
                scale = nearest / distance;
                position = add(head.position, [line[0] * scale, line[1] * scale, line[2] * scale]);
            }
        }
        Some(Layer::Overlay { space: local, swapchain, pose: Posef::new(orientation, position), size: Extent2Df { width: width * scale, height: height * scale } })
    }
}

impl HandPanel<'_> {
    /// The panel's width and height in metres: its width, widened for a strip so it is at least
    /// [`LEAST_HEIGHT`] tall, [`WIDEST`] at most.
    pub fn shown_size(&self) -> (f32, f32) {
        let aspect = self.size.0 as f32 / self.size.1.max(1) as f32;
        let width = if self.size.0 == 0 { self.place.width } else { self.place.width.max(LEAST_HEIGHT * aspect).min(WIDEST) };
        (width, width / aspect.max(0.01))
    }
}

/// A panel's rotation in the right grip space: facing `facing` along x (+1 out of the back of the
/// hand, -1 out of the palm), its top along `head_up` (the head's up, in the grip space) laid
/// onto the hand, else toward the knuckles (-Z); its right side follows (right = up x facing,
/// unmirrored).
fn facing_up(head_up: Vec3, facing: f32) -> Quat {
    let (y, z) = (head_up[1], head_up[2]);
    let length = (y * y + z * z).sqrt();
    let up = if length > SHORTEST_UP { [0.0, y / length, z / length] } else { [0.0, 0.0, -1.0] };
    let right = [0.0, up[2] * facing, -up[1] * facing];
    from_axes(right, up, [facing, 0.0, 0.0])
}

#[cfg(test)]
mod tests;
