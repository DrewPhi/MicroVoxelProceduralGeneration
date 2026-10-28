//! Authoritative discrete world data. Renderers and simulations are consumers.
//!
//! Spatial state uses integer 1/16-meter cells. Continuous terrain mathematics
//! are private generation details; a cell's value never depends on camera LOD.
//! No physical/biological material properties or gameplay systems are populated.

pub mod generation;
pub mod grid;
pub mod materials;
pub mod meshing;
pub mod painting;
pub mod persistence;
pub mod queries;
pub mod render;
pub mod simulation;
pub mod storage;
pub mod world;

pub use grid::{Bounds, CellPos, ChunkPos, VOXEL_SIZE};
pub use materials::{MaterialId, Voxel};
pub use world::{Edit, Lookup, World};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
