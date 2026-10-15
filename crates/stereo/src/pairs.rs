//! Same-frame pairs: two eye images published as one stereo pair over the fence channel, each eye
//! with its own pose. [`d3d11`]: each eye copied out of a D3D11 back buffer at its present.
//! [`d3d12`]: any two D3D12 textures, with each eye's depth when it goes to the headset.

pub mod d3d11;
pub mod d3d12;
