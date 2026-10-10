//! HUD pieces the game publishes on their own (`<channel>-panel-<piece>`, texture channels of
//! either transport, premultiplied RGBA: the minimap, the quests, the weapon widget, health, the
//! quick item), shown on the hands: each a quad in its controller's grip space, so the runtime
//! moves it with the hand at the headset's own rate, on the back of the hand or above the palm. A
//! channel appears only when the producer has that piece (Dying Light 1 and 2 with the dynamic
//! HUD); until then it is looked for now and then.

use monaka_channel::d3d::{self, create_texture, simple_texture_desc};
use monaka_channel::reader::{self, AnyReader};
use monaka_channel::ChannelName;
use monaka_core::math::{Quat, Vec3, rotate};
use monaka_core::protocol::{HandPose, HeadPose};
use monaka_core::protocol::{RIGHT_HAND, dxgi_format};
use monaka_openxr::{Extent2Df, Layer, Session, Space, Swapchain};
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_SHADER_RESOURCE, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Frames between looks for the channel while it is not there.
const LOOK_EVERY: u64 = 90;
/// The panels' widths on the hand, in metres: the minimap, the weapon widget and health about a
/// hand's, the quests (a wide list of text) beside the minimap, the quick item (one icon) smaller.
pub const HAND_WIDTH: f32 = 0.08;
pub const QUESTS_WIDTH: f32 = 0.20;
pub const ICON_WIDTH: f32 = 0.05;
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

/// Which side of a hand a panel lies on. In a grip space -Z runs along the pointing finger and +Y
/// toward the thumb, and +X leaves the hand through the back of the right hand but the palm of
/// the left (OpenXR's grip pose: both point right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    /// On the back of the hand, a little out from the handle.
    BackOfHand,
    /// Just above the palm (the fingers curl round the handle there).
    Palm,
}

impl Side {
    /// The way the panel faces along the grip's x axis on `hand` (`LEFT_HAND` or `RIGHT_HAND`).
    fn facing(self, hand: usize) -> f32 {
        let back = if hand == RIGHT_HAND { 1.0 } else { -1.0 };
        match self {
            Side::BackOfHand => back,
            Side::Palm => -back,
        }
    }

    /// Where it sits in `hand`'s grip space (metres).
    fn offset(self, hand: usize) -> [f32; 3] {
        let out = self.facing(hand);
        match self {
            Side::BackOfHand => [0.055 * out, 0.0, 0.01],
            Side::Palm => [0.04 * out, 0.0, 0.0],
        }
    }
}

pub struct HandPanel<'s> {
    piece: &'static str,
    hand: usize,
    side: Side,
    width: f32,
    /// A panel left of the hand's main panel on the same side (the quests beside the minimap):
    /// its right edge at that one's left, their tops level.
    beside: bool,
    name: Option<ChannelName>,
    format: u32,
    reader: Option<AnyReader>,
    textures: Option<[ID3D11Texture2D; 2]>,
    swapchain: Option<Swapchain<'s>>,
    size: (u32, u32),
    seen: u64,
    frames: u64,
}

impl<'s> HandPanel<'s> {
    /// The piece `piece` of the game channel `channel` on `side` of `hand`, shown in
    /// swapchains of `format` (the eyes' own).
    pub fn new(channel: &ChannelName, format: u32, piece: &'static str, hand: usize, side: Side, width: f32, beside: bool) -> Self {
        // The channel's texels are RGBA8 and are copied as they are.
        let rgba = matches!(format, dxgi_format::R8G8B8A8_UNORM | dxgi_format::R8G8B8A8_UNORM_SRGB);
        let name = channel.panel(piece).filter(|_| rgba);
        Self { piece, hand, side, width, beside, name, format, reader: None, textures: None, swapchain: None, size: (0, 0), seen: 0, frames: 0 }
    }

    /// Once per frame: opens the channel when it appears and takes a new panel into the swapchain.
    pub fn update(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext) {
        self.frames += 1;
        if self.reader.is_none() {
            if !self.frames.is_multiple_of(LOOK_EVERY) {
                return;
            }
            let Some(name) = &self.name else { return };
            // The producer's transport: keyed (D3D11 games) or fence (D3D12 games).
            let Some(description) = reader::describe_any(name) else { return };
            let Ok(reader) = AnyReader::open(name, device, description.fence) else { return };
            let size = (description.width, description.height);
            // The staging textures in the channel's own format (the reader wants an exact match;
            // the eyes' format, sRGB or not, is of the same family, so the copy into the
            // swapchain is allowed).
            let mut desc = simple_texture_desc(size.0, size.1, DXGI_FORMAT(description.format as i32));
            desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
            let (Ok(a), Ok(b)) = (create_texture(device, &desc), create_texture(device, &desc)) else { return };
            match session.create_swapchain(self.format as i64, size.0, size.1) {
                Ok(swapchain) => {
                    println!("HUD panel {} on the {} hand, {:?} ({}x{})", self.piece, if self.hand == RIGHT_HAND { "right" } else { "left" }, self.side, size.0, size.1);
                    self.reader = Some(reader);
                    self.textures = Some([a, b]);
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
        let (Some(reader), Some(textures), Some(swapchain)) = (&mut self.reader, &self.textures, &self.swapchain) else { return };
        match reader.read(context, [&textures[0], &textures[1]], self.seen) {
            Ok(Some(pair)) => {
                self.seen = pair.sequence;
                let _ = swapchain.fill(|image| {
                    d3d::copy_texture(context, image, &textures[0]);
                    d3d::flush(context);
                });
            }
            Ok(None) => {}
            Err(e) => {
                println!("HUD panel {} lost: {e}", self.piece);
                self.reader = None;
                self.textures = None;
                self.swapchain = None;
                self.seen = 0;
            }
        }
    }

    /// The panel on its hand, once one has arrived; `grips` (by hand) and `head` are the grips'
    /// and the head's poses in `local`, this frame.
    pub fn layer<'a>(&'a self, local: &'a Space<'a>, grips: &[HandPose; 2], head: Option<&HeadPose>, nearest: f32, main_width: f32) -> Option<Layer<'a>> {
        let grip = &grips[self.hand];
        if grip.valid == 0 {
            return None;
        }
        let swapchain = self.swapchain.as_ref().filter(|_| self.seen != 0)?;
        let (width, height) = self.shown_size();
        let head_up = match head {
            Some(head) => rotate(conjugate(grip.orientation), rotate(head.orientation, [0.0, 1.0, 0.0])),
            None => [0.0, 0.0, -1.0],
        };
        let orientation = facing_up(head_up, self.side.facing(self.hand));
        let mut position = self.side.offset(self.hand);
        if self.beside {
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
        Some(Layer::Overlay { space: local, swapchain, pose: crate::pose(orientation, position), size: Extent2Df { width: width * scale, height: height * scale } })
    }
}

impl HandPanel<'_> {
    /// The panel's width and height in metres: its width, widened for a strip so it is at least
    /// [`LEAST_HEIGHT`] tall, [`WIDEST`] at most.
    pub fn shown_size(&self) -> (f32, f32) {
        let aspect = self.size.0 as f32 / self.size.1.max(1) as f32;
        let width = if self.size.0 == 0 { self.width } else { self.width.max(LEAST_HEIGHT * aspect).min(WIDEST) };
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

fn conjugate([x, y, z, w]: Quat) -> Quat {
    [-x, -y, -z, w]
}

/// `a * b`: the rotation applying `b` first, then `a` (Hamilton product).
fn multiply(a: Quat, b: Quat) -> Quat {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The rotation taking x, y and z to the orthonormal axes `x`, `y`, `z` (right-handed).
fn from_axes(x: Vec3, y: Vec3, z: Vec3) -> Quat {
    // Rows of the matrix whose columns are the axes.
    let m = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
    let trace = m[0][0] + m[1][1] + m[2][2];
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, 0.25 * s]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [(m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [(m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s, (m[1][0] - m[0][1]) / s]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_core::math::rotate;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn the_panel_lies_on_the_back_of_the_right_hand_top_toward_the_head_up() {
        // The head's up along the knuckles: the top goes there, the right toward the little finger.
        let q = facing_up([0.3, 0.0, -0.9], 1.0);
        assert!(close(rotate(q, [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]), "faces out of the back of the hand");
        assert!(close(rotate(q, [0.0, 1.0, 0.0]), [0.0, 0.0, -1.0]), "its top toward the knuckles");
        assert!(close(rotate(q, [1.0, 0.0, 0.0]), [0.0, -1.0, 0.0]), "its right toward the little finger");
        // Thumb up (the head's up toward the thumb): the top goes there, the right toward the fingers.
        let q = facing_up([0.0, 2.0, 0.0], 1.0);
        assert!(close(rotate(q, [0.0, 1.0, 0.0]), [0.0, 1.0, 0.0]));
        assert!(close(rotate(q, [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]));
        assert!(close(rotate(q, [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
        // The head's up straight out of the back of the hand: toward the knuckles.
        assert!(close(rotate(facing_up([1.0, 0.05, 0.0], 1.0), [0.0, 1.0, 0.0]), [0.0, 0.0, -1.0]));
    }

    #[test]
    fn the_left_hand_mirrors_its_sides() {
        use monaka_core::protocol::LEFT_HAND;
        assert_eq!(Side::BackOfHand.facing(LEFT_HAND), -1.0, "the left grip's -X leaves the back of the hand");
        assert_eq!(Side::Palm.facing(LEFT_HAND), 1.0);
        assert!(Side::BackOfHand.offset(LEFT_HAND)[0] < 0.0 && Side::Palm.offset(LEFT_HAND)[0] > 0.0);
        // On the back of the left hand, knuckles up: the right side toward the thumb (+Y), unmirrored.
        let q = facing_up([0.0, 0.0, -1.0], Side::BackOfHand.facing(LEFT_HAND));
        assert!(close(rotate(q, [0.0, 0.0, 1.0]), [-1.0, 0.0, 0.0]));
        assert!(close(rotate(q, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn the_palm_panel_faces_out_of_the_palm_unmirrored() {
        // Palm turned to the eyes, fingers up: the top toward the fingers, the right toward the thumb.
        let q = facing_up([0.0, 0.0, -1.0], -1.0);
        assert!(close(rotate(q, [0.0, 0.0, 1.0]), [-1.0, 0.0, 0.0]), "faces out of the palm");
        assert!(close(rotate(q, [0.0, 1.0, 0.0]), [0.0, 0.0, -1.0]), "its top toward the fingers");
        assert!(close(rotate(q, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]), "its right toward the thumb");
    }

    #[test]
    fn axes_round_trip_through_a_quaternion() {
        for (x, y, z) in [
            ([0.0, -1.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
            ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
        ] {
            let q = from_axes(x, y, z);
            assert!(close(rotate(q, [1.0, 0.0, 0.0]), x) && close(rotate(q, [0.0, 1.0, 0.0]), y) && close(rotate(q, [0.0, 0.0, 1.0]), z), "{x:?} {y:?} {z:?}");
        }
    }
}
