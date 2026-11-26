//! Immutable cut-and-fill grade sampled by canonical native columns.
use crate::voxel_size;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct DirtRunway {
    center: [i64; 2],
    top: i64,
    along_x: bool,
}

impl DirtRunway {
    pub const HALF_LENGTH_M: f64 = 260.;
    pub const HALF_WIDTH_M: f64 = 12.;
    const SHOULDER_M: f64 = 28.;

    pub fn new(center: [i64; 2], top: i64, along_x: bool) -> Self {
        Self {
            center,
            top,
            along_x,
        }
    }

    pub fn center_m(self) -> [f64; 2] {
        self.center.map(|c| c as f64 * voxel_size())
    }

    pub fn along_x(self) -> bool {
        self.along_x
    }

    pub fn top_cells(self) -> i64 {
        self.top
    }

    pub fn height_m(self) -> f64 {
        self.top as f64 * voxel_size()
    }

    pub fn half_extents_m(self) -> [f64; 2] {
        if self.along_x {
            [Self::HALF_LENGTH_M, Self::HALF_WIDTH_M]
        } else {
            [Self::HALF_WIDTH_M, Self::HALF_LENGTH_M]
        }
    }

    pub(super) fn weight(self, point: [f64; 2]) -> f64 {
        let center = self.center_m();
        let extent = self.half_extents_m();
        let across_x = (point[0] - center[0]).abs() - extent[0];
        let across_z = (point[1] - center[1]).abs() - extent[1];
        let t = (across_x.max(across_z) / Self::SHOULDER_M).clamp(0., 1.);
        1. - t * t * (3. - 2. * t)
    }

    pub(crate) fn intersects(self, min: [f64; 2], max: [f64; 2]) -> bool {
        let center = self.center_m();
        let extent = self.half_extents_m();
        (0..2).all(|i| {
            min[i] <= center[i] + extent[i] + Self::SHOULDER_M
                && max[i] >= center[i] - extent[i] - Self::SHOULDER_M
        })
    }

    pub(crate) fn covers(self, min: [f64; 2], max: [f64; 2]) -> bool {
        let center = self.center_m();
        let extent = self.half_extents_m();
        (0..2).all(|i| min[i] >= center[i] - extent[i] && max[i] <= center[i] + extent[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        affine_bounds, raycast_solid, Cell, FbmParams, FbmRecipe, FbmTerrain, Material, World,
    };

    #[test]
    fn dirt_runway_flattens_canonical_columns_and_preserves_the_surroundings() {
        for recipe in [FbmRecipe::SeededWorld, FbmRecipe::Alpine] {
            let base = FbmTerrain::new(
                7,
                FbmParams {
                    recipe,
                    ..Default::default()
                },
            )
            .unwrap();
            let center = Cell::from_meters([-123.25, 0., 881.5]);
            let top = base.column_cells(center.x, center.z).ground_top;
            let runway = DirtRunway::new([center.x, center.z], top, true);
            let world =
                World::from_fbm_terrain(base.clone().with_dirt_runway(runway)).with_vegetation();
            let [x, z] = runway.center_m();
            for along in [-259., -130., 0., 130., 259.] {
                for side in [-11., 0., 11.] {
                    let position = [x + along, runway.height_m() + 4., z + side];
                    let c = Cell::from_meters(position);
                    let column = world.column(c.x, c.z);
                    assert_eq!(column.height_cells(), top);
                    assert_eq!(column.ground_height_cells(), top);
                    let hit = raycast_solid(&world, position, [0., -1., 0.], 5.).unwrap();
                    assert_eq!(hit.material, Material::Soil);
                    assert!((hit.distance - 4.).abs() < 1e-9);
                }
            }
            let outside = Cell::from_meters([x, 0., z + 65.]);
            assert_eq!(
                world.column(outside.x, outside.z).height_m(),
                base.height_m(x, z + 65.)
            );
            let h = affine_bounds::bound_native_height(
                world.fbm_terrain().unwrap(),
                affine_bounds::ColumnBox {
                    min: [center.x - 8, center.z - 8],
                    size_cells: [16, 16],
                },
                affine_bounds::BoundBudget::default(),
            )
            .unwrap();
            assert_eq!(h.center_height_m, runway.height_m());
            assert_eq!(h.remainder_m, 0.);
        }
    }
}
