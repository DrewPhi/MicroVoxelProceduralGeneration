//! A conservative upper bound on terrain, plants and overrides inside a moving square.
use frontier_world::{voxel_size, World};
use glam::DVec3;

const REUSE_MARGIN: f64 = 32.;

#[derive(Default)]
pub(crate) struct Airspace(Option<SkyClearance>);
impl Airspace {
    pub(crate) fn clear_above(
        &mut self,
        world: &World,
        center: DVec3,
        half_width: f64,
        depth: f64,
    ) -> bool {
        if self.0.as_ref().is_none_or(|old| {
            old.source.revision() != world.revision()
                || !old.source.shares_source_lineage(world)
                || half_width > old.half_width
                || (center.x - old.center.x).abs() > REUSE_MARGIN
                || (center.z - old.center.z).abs() > REUSE_MARGIN
        }) {
            self.0 = Some(SkyClearance::new(world, center, half_width));
        }
        center.y - depth > self.0.as_ref().unwrap().ceiling
    }
}

struct SkyClearance {
    source: World,
    center: DVec3,
    ceiling: f64,
    half_width: f64,
}
impl SkyClearance {
    fn new(world: &World, center: DVec3, half_width: f64) -> Self {
        use frontier_world::{
            affine_bounds::{bound_native_height, BoundBudget, ColumnBox},
            vegetation::{MAX_PLANT_HEIGHT_CELLS, MAX_PLANT_RADIUS_CELLS},
            voxel_grid,
        };
        let extent = half_width + REUSE_MARGIN;
        let ceiling = (|| {
            let terrain = world.fbm_terrain()?;
            if world.topology_base_cells().is_some() {
                return None;
            }
            let lo = voxel_grid().checked_cell([center.x - extent, 0., center.z - extent])?;
            let hi = voxel_grid().checked_cell([center.x + extent, 0., center.z + extent])?;
            let pad = if world.vegetation_enabled() {
                MAX_PLANT_RADIUS_CELLS + 1
            } else {
                1
            };
            let lower = [lo.x.checked_sub(pad)?, lo.z.checked_sub(pad)?];
            let upper = [hi.x.checked_add(pad + 1)?, hi.z.checked_add(pad + 1)?];
            let bounds = ColumnBox {
                min: lower,
                size_cells: [
                    u32::try_from(upper[0].checked_sub(lower[0])?).ok()?,
                    u32::try_from(upper[1].checked_sub(lower[1])?).ok()?,
                ],
            };
            let height =
                bound_native_height(terrain, bounds, BoundBudget::default())?.height_range[1];
            let vegetation = if world.vegetation_enabled() {
                MAX_PLANT_HEIGHT_CELLS + 2
            } else {
                1
            };
            Some(
                world
                    .edit_cells_in_columns(lower, upper)
                    .into_iter()
                    .fold(height + vegetation as f64 * voxel_size(), |y, c| {
                        y.max((c.y as f64 + 1.) * voxel_size())
                    }),
            )
        })()
        .unwrap_or(f64::INFINITY);
        Self {
            source: world.clone(),
            center,
            ceiling,
            half_width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{Cell, FbmParams, Material};

    #[test]
    fn cached_airspace_handles_descent_expansion_edits_and_source_changes() {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let mut air = Airspace::default();
        let eye = DVec3::new(0., 300., 0.);
        assert!(air.clear_above(&world, eye, 130., 130.));
        assert!(!air.clear_above(&world, DVec3::ZERO, 130., 130.));
        let mut edited = world.clone();
        edited.set(Cell::from_meters([149., 250., 0.]), Material::Stone);
        assert!(!air.clear_above(&edited, eye + DVec3::X * 32., 130., 130.));
        assert!(air.clear_above(&edited, eye - DVec3::X * 100., 20., 100.));
        assert!(!air.clear_above(&edited, eye - DVec3::X * 100., 250., 100.));
        assert!(air.clear_above(&world, eye, 250., 100.));
    }
}
