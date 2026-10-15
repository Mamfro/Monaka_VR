//! A headset menu's quad, the same for the debug menu ([`crate::debug_menu`]) and Monaka's home
//! (each panel of [`crate::home`]): where it floats (placed ahead of the head when it opens), its
//! swapchain (made at its first show), its image handed to the headset, and the menu turned off for
//! the run when any of that fails.

use crate::debug_menu::Buzz;
use monaka_channel::d3d;
use monaka_core::math::{self, Quat, Vec3};
use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_openxr::{Posef, Session, Swapchain};
use windows::Win32::Graphics::Direct3D11::{ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Where a menu floats when it opens: this far ahead of the head and this far below the eyes
/// (metres).
const DISTANCE: f32 = 1.0;
const BELOW_EYES: f32 = 0.1;

/// The buzz on both hands when a menu opens or closes.
pub const TOGGLED: [Buzz; 2] = [(LEFT_HAND, 0.5, 0.08), (RIGHT_HAND, 0.5, 0.08)];

/// The buzz on the right hand for a press in a menu (`switch`: one that changes something).
pub fn pressed(switch: bool) -> Buzz {
    (RIGHT_HAND, if switch { 0.4 } else { 0.15 }, 0.03)
}

pub struct MenuQuad<'s> {
    /// The menu's name, for the line that says it is off.
    name: &'static str,
    format: DXGI_FORMAT,
    pixels: [u32; 2],
    swapchain: Option<Swapchain<'s>>,
    failed: bool,
    /// Where it floats (tracking space).
    pub anchor: Posef,
}

impl<'s> MenuQuad<'s> {
    /// The quad of menu `name`, `pixels` in `format`, a metre straight ahead until it opens.
    pub fn new(name: &'static str, format: DXGI_FORMAT, pixels: [u32; 2]) -> Self {
        Self { name, format, pixels, swapchain: None, failed: false, anchor: Posef::new(math::IDENTITY, [0.0, 0.0, -DISTANCE]) }
    }

    /// The menu opened with the head at `head` (none: unknown, it stays where it was).
    pub fn open_at(&mut self, head: Option<(Quat, Vec3)>) {
        if let Some((orientation, position)) = head {
            let (facing, ahead) = math::ahead_of(orientation, position, DISTANCE);
            self.anchor = Posef::new(facing, [ahead[0], ahead[1] - BELOW_EYES, ahead[2]]);
        }
    }

    /// Hands the menu's image to the headset, `draw`n when it shows something new (its argument:
    /// the swapchain is new, so the image must be drawn whole again). Nothing once failed.
    pub fn show(&mut self, session: &'s Session<'_>, context: &ID3D11DeviceContext, draw: impl FnOnce(bool) -> Result<Option<ID3D11Texture2D>, String>) {
        if self.failed {
            return;
        }
        let fresh = self.swapchain.is_none();
        if fresh {
            match session.create_swapchain(self.format.0 as i64, self.pixels[0], self.pixels[1]) {
                Ok(swapchain) => self.swapchain = Some(swapchain),
                Err(e) => return self.fail(format!("no swapchain for it ({e})")),
            }
        }
        let image = match draw(fresh) {
            Ok(Some(image)) => image,
            Ok(None) => return,
            Err(e) => return self.fail(e),
        };
        if let Some(swapchain) = &self.swapchain
            && let Err(e) = swapchain.fill(|target| {
                d3d::copy_texture(context, target, &image);
            })
        {
            self.fail(format!("its image could not be shown ({e})"));
        }
    }

    /// Its swapchain, once made and while nothing failed.
    pub fn swapchain(&self) -> Option<&Swapchain<'s>> {
        self.swapchain.as_ref().filter(|_| !self.failed)
    }

    pub fn fail(&mut self, why: String) {
        println!("{}: {why}; the menu is off this run", self.name);
        self.failed = true;
    }
}
