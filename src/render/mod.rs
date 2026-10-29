//! Renderer-independent surface compression and a balanced voxel LOD cover.
//! These are disposable display data; canonical world queries never use them.
pub mod lod;
pub mod surface;
pub mod terrain;
pub mod triangulation;

pub mod ray;

pub mod octree;
