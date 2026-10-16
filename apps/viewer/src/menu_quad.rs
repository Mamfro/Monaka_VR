//! One panel of a ring in the headset ([`crate::ring`]: Monaka's home, the debug menu): where it
//! floats, its swapchain (made at its first show), its image handed to the headset, and the panel
//! turned off for the run when any of that fails.

use crate::debug_menu::Buzz;
use monaka_channel::d3d;
use monaka_core::math;
use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_openxr::{Posef, Session, Swapchain};
use windows::Win32::Graphics::Direct3D11::{ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Where a panel floats until its ring stands somewhere: this far ahead (metres).
const DISTANCE: f32 = 1.0;

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
