//! Alternate-eye rendering: every presented frame is one eye, and each left/right pair goes to the
//! viewer with the head poses its two frames were rendered from (see [`monaka_core::alternate`] for
//! the timing). One publisher per backend: [`d3d11`] (copies on the swapchain's device) and
//! [`d3d12`] (the left eye held on the game's queue), both over the fence channel.

pub mod d3d11;
pub mod d3d12;

/// What a present did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing shown yet that belongs to a pair.
    Waiting,
    /// A left eye was copied.
    Left,
    /// A pair was published.
    Published,
    /// A right eye arrived without a left one (after a resize or a failure).
    Unpaired,
    Failed,
}
