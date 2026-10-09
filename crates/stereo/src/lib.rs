//! Producer-side stereo that does not depend on the game. A game crate supplies the part only it
//! can do (making the next frame render a given eye from a given head pose); this crate turns the
//! frames into published stereo pairs.
//!
//! - [`driver`]: the alternate-eye present loop around DXGI's Present (the game's swapchain,
//!   presents counted, the next eye and pose chosen, pairs published), for any game drawing one
//!   eye per frame.
//! - [`alternate`]: alternate-eye rendering, each presented frame one eye (D3D11 swapchains).
//! - [`alternate12`]: the same over a D3D12 swapchain and the fence channel.
//! - [`pairs11`]: same-frame pairs over a D3D11 swapchain, each eye copied at its present.
//! - [`pairs12`]: any two D3D12 textures published as one pair, each eye with its own pose.
//! - [`hybrid`]: alternate-eye rendering with depth: a pair every frame, the other eye reprojected
//!   from the real one and its gaps filled from that eye's own last real frame.
//! - [`hud11`]: a D3D11 game's HUD placed in front of each eye (viewport of the draws after the scene copy).
//! - [`present`]: DXGI's Present hooked for a game whose swapchain is out of reach.
//! - [`queue12`]: the game's D3D12 direct queue, caught at `ExecuteCommandLists`, for a game whose
//!   engine keeps it out of reach.
//!
//! Depth-based stereo from a single centre render is `monaka_warp`, with its math in
//! `monaka_core::depth`.

pub mod alternate;
pub mod alternate12;
pub mod driver;
pub mod hud11;
pub mod hybrid;
pub mod pairs11;
pub mod pairs12;
pub mod present;
pub mod queue12;
