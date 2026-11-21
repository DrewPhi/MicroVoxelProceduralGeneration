//! A conservative upper bound on terrain, plants and overrides inside a moving square.
use frontier_world::{
    affine_bounds::{bound_native_height, BoundBudget, ColumnBox},
    voxel_size, FbmTerrain, World,
};
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
            self.0 = Some(SkyClearance::new(world, center, half_width, depth));
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
    fn new(world: &World, center: DVec3, half_width: f64, depth: f64) -> Self {
        use frontier_world::{
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
            let vegetation = if world.vegetation_enabled() {
                MAX_PLANT_HEIGHT_CELLS + 2
            } else {
                1
            };
            let plant_height = vegetation as f64 * voxel_size();
            let target = center.y - depth - plant_height;
            let base = terrain_ceiling(terrain, bounds, target, 2)? + plant_height;
            Some(
                world
                    .override_y_ceiling(lower, upper)
                    .map_or(base, |y| base.max((y as f64 + 1.) * voxel_size())),
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

// On mountain terrain a single height bound can span several ridges.
// Split only boxes whose current certificate intersects the flight envelope.
fn terrain_ceiling(
    terrain: &FbmTerrain,
    bounds: ColumnBox,
    target: f64,
    levels: u8,
) -> Option<f64> {
    let bound = bound_native_height(terrain, bounds, BoundBudget::default())?.height_range[1];
    if bound < target || levels == 0 || bounds.size_cells.contains(&1) {
        return Some(bound);
    }
    let [width, depth] = bounds.size_cells;
    let [hx, hz] = [width / 2, depth / 2];
    let mut ceiling = f64::NEG_INFINITY;
    for (z, sz) in [(0, hz), (hz, depth - hz)] {
        for (x, sx) in [(0, hx), (hx, width - hx)] {
            let child = ColumnBox {
                min: [bounds.min[0] + i64::from(x), bounds.min[1] + i64::from(z)],
                size_cells: [sx, sz],
            };
            let height = terrain_ceiling(terrain, child, target, levels - 1).unwrap_or(bound);
            if height >= target {
                return Some(bound);
            }
            ceiling = ceiling.max(height);
        }
    }
    Some(ceiling.min(bound))
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{Cell, FbmParams, Material};

    #[test]
    fn highlands_flight_proves_clear_air_and_resumes_on_descent() {
        let world = World::fbm(
            673,
            FbmParams {
                recipe: frontier_world::FbmRecipe::Highlands,
                ..FbmParams::default()
            },
        )
        .unwrap()
        .with_vegetation_enabled(true);
        let mut air = Airspace::default();
        let eye = DVec3::new(-512., 2700., -512.);
        assert!(air.clear_above(&world, eye, 144., 144.));
        let ground = world.height_m(eye.x, eye.z);
        assert!(!air.clear_above(&world, DVec3::new(eye.x, ground + 10., eye.z), 144., 144.));
    }

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
