//! Anything the voxel previews can draw: a density, solid where > 0, with a
//! per-column vertical shell (solid below, air above).

use terrain_core::fit::FittedTerrain;
use terrain_core::{Column, Terrain};

pub trait VoxelSource: Sync {
    /// Per-(x, z) data reused for every voxel in a column.
    type Col: Clone + Default + Send + Sync;
    fn column(&self, x: f64, z: f64) -> Self::Col;
    /// Solid below `.0`, air above `.1`.
    fn shell(&self, col: &Self::Col) -> (f64, f64);
    fn density(&self, col: &Self::Col, y: f64) -> f64;
    fn gradient(&self, col: &Self::Col, y: f64) -> [f64; 3];
}

impl VoxelSource for Terrain {
    type Col = Column;
    fn column(&self, x: f64, z: f64) -> Column {
        Terrain::column(self, x, z)
    }
    fn shell(&self, col: &Column) -> (f64, f64) {
        col.shell()
    }
    fn density(&self, col: &Column, y: f64) -> f64 {
        self.density_in(col, y)
    }
    fn gradient(&self, col: &Column, y: f64) -> [f64; 3] {
        self.sample_in(col, y).gradient
    }
}

/// A column of a fitted terrain: its position and patch index.
#[derive(Clone, Copy, Debug, Default)]
pub struct FitCol {
    x: f64,
    z: f64,
    patch: Option<usize>,
}

impl VoxelSource for FittedTerrain {
    type Col = FitCol;
    fn column(&self, x: f64, z: f64) -> FitCol {
        FitCol {
            x,
            z,
            patch: self.patch_index(x, z),
        }
    }
    fn shell(&self, col: &FitCol) -> (f64, f64) {
        col.patch
            .map_or((0.0, 0.0), |i| (self.patches[i].y_lo, self.patches[i].y_hi))
    }
    fn density(&self, col: &FitCol, y: f64) -> f64 {
        col.patch
            .map_or(-y, |i| self.patches[i].density([col.x, y, col.z]))
    }
    fn gradient(&self, col: &FitCol, y: f64) -> [f64; 3] {
        col.patch.map_or([0.0, -1.0, 0.0], |i| {
            self.patches[i].gradient([col.x, y, col.z])
        })
    }
}
