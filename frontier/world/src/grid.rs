//! The native lattice and the conversion between cell coordinates and meters.
use crate::Cell;
use std::{fmt, sync::OnceLock};

/// The native edge length used by existing worlds and fixtures, in meters.
pub const VOXEL_SIZE: f64 = 0.0625;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelGrid(f64);

#[derive(Clone, Copy, Debug)]
pub struct InvalidVoxelSize;

impl fmt::Display for InvalidVoxelSize {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("voxel size must be positive and finite, with a finite GPU conversion in both directions")
    }
}
impl std::error::Error for InvalidVoxelSize {}

impl VoxelGrid {
    pub const DEFAULT: Self = Self(VOXEL_SIZE);

    pub fn new(meters: f64) -> Result<Self, InvalidVoxelSize> {
        let gpu_meters = meters as f32;
        if !meters.is_finite()
            || gpu_meters <= 0.
            || !gpu_meters.is_finite()
            || !gpu_meters.recip().is_finite()
        {
            return Err(InvalidVoxelSize);
        }
        Ok(Self(meters))
    }

    pub fn size_m(self) -> f64 {
        self.0
    }

    pub fn checked_cell(self, position: [f64; 3]) -> Option<Cell> {
        let coordinate = |meters: f64| {
            let cells = meters / self.0;
            let nearest = cells.round();
            // A face produced by the same lattice may divide back to a value
            // just below its integer coordinate when the size is a decimal.
            let index = if nearest * self.0 == meters {
                nearest
            } else {
                cells.floor()
            };
            (index.is_finite() && index >= i64::MIN as f64 && index < -(i64::MIN as f64))
                .then_some(index as i64)
        };
        Some(Cell {
            x: coordinate(position[0])?,
            y: coordinate(position[1])?,
            z: coordinate(position[2])?,
        })
    }
}

// Frontier currently runs one world per process. Install the lattice before
// constructing sources, loading saves, or starting renderer/worker threads.
static ACTIVE_GRID: OnceLock<VoxelGrid> = OnceLock::new();

pub fn configure_voxel_grid(grid: VoxelGrid) -> Result<(), &'static str> {
    if *ACTIVE_GRID.get_or_init(|| grid) == grid {
        Ok(())
    } else {
        Err("the voxel grid is already configured for this process")
    }
}

#[inline]
pub fn voxel_size() -> f64 {
    voxel_grid().size_m()
}

#[inline]
pub fn voxel_grid() -> VoxelGrid {
    *ACTIVE_GRID.get_or_init(|| VoxelGrid::DEFAULT)
}

/// Extract the common CLI setting before parsing options that query terrain.
pub fn voxel_grid_args(
    mut args: impl Iterator<Item = String>,
) -> Result<std::vec::IntoIter<String>, String> {
    let mut rest = Vec::new();
    let mut grid = None;
    while let Some(arg) = args.next() {
        let value = if arg == "--voxel-size" {
            args.next()
                .ok_or("--voxel-size requires an edge length in meters")?
        } else if let Some(value) = arg.strip_prefix("--voxel-size=") {
            value.to_owned()
        } else {
            rest.push(arg);
            continue;
        };
        if grid.is_some() {
            return Err("--voxel-size may only be specified once".into());
        }
        let meters = value
            .parse()
            .map_err(|_| format!("invalid --voxel-size {value:?} in meters"))?;
        grid = Some(VoxelGrid::new(meters).map_err(|error| error.to_string())?);
    }
    configure_voxel_grid(grid.unwrap_or(VoxelGrid::DEFAULT))?;
    Ok(rest.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_sizes_keep_half_open_cells_on_both_sides_of_the_origin() {
        for size in [0.03, VOXEL_SIZE, 0.15, 0.5, 1.25] {
            let grid = VoxelGrid::new(size).unwrap();
            for cell in -129..=129 {
                let face = cell as f64 * size;
                assert_eq!(grid.checked_cell([face; 3]).unwrap().x, cell);
                assert_eq!(
                    grid.checked_cell([face - size / 16.; 3]).unwrap().x,
                    cell - 1
                );
                assert_eq!(grid.checked_cell([face + size / 2.; 3]).unwrap().x, cell);
            }
        }
    }

    #[test]
    fn invalid_sizes_are_rejected_before_gpu_setup() {
        for size in [0., -0.1, f64::NAN, f64::INFINITY, 1e-45, 1e50] {
            assert!(VoxelGrid::new(size).is_err());
        }
    }
}
