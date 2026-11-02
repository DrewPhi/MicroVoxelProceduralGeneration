//! Analytic micro-voxel terrain core.
//!
//! The *source* terrain is an implicit 3D density ([`terrain::Terrain`]): a 2D
//! FBM heightfield plus a bounded 3D FBM term that carves overhangs, with exact
//! analytic gradients throughout. Later stages fit cheap closed-form pieces to
//! this source; those pieces, not the noise, are what the renderer intersects.
//!
//! Integer hashing is 32-bit and every operation has a direct WGSL equivalent,
//! so the same functions can later run in a compute shader.

pub mod fbm;
pub mod fit;
pub mod noise;
pub mod noise3;
pub mod terrain;

pub use fbm::{Fbm, FbmParams, Sample};
pub use terrain::{Column, OverhangParams, Sample3, SurfaceScan, Terrain, TerrainParams};
