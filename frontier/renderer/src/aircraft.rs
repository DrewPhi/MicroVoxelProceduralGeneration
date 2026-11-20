//! A coordinated-turn flight controller. All distances and rates use metres,
//! seconds and radians independently of the native voxel size.
use crate::Camera;
use frontier_world::{voxel_size, World};
use glam::{DVec3, Mat3, Vec3};

struct SkyClearance {
    source: World,
    center: DVec3,
    ceiling: f64,
}
impl SkyClearance {
    fn new(world: &World, center: DVec3) -> Self {
        use frontier_world::{
            affine_bounds::{bound_native_height, BoundBudget, ColumnBox},
            vegetation::{MAX_PLANT_HEIGHT_CELLS, MAX_PLANT_RADIUS_CELLS},
            voxel_grid,
        };
        let ceiling = (|| {
            let terrain = world.fbm_terrain()?;
            if world.topology_base_cells().is_some() {
                return None;
            }
            let lo = voxel_grid().checked_cell([center.x - 64., 0., center.z - 64.])?;
            let hi = voxel_grid().checked_cell([center.x + 64., 0., center.z + 64.])?;
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
        }
    }
}

pub(crate) fn axes(pose: Camera) -> Mat3 {
    let forward = pose.direction().normalize();
    let lateral = pose.view_up().cross(forward).normalize();
    Mat3::from_cols(lateral, forward.cross(lateral), forward)
}

pub(crate) struct Aircraft {
    speed: f32,
    // In chase view the viewer camera follows this physical aircraft pose.
    pilot: Option<Camera>,
    clearance: Option<SkyClearance>,
}

impl Default for Aircraft {
    fn default() -> Self {
        Self {
            speed: 65.,
            pilot: None,
            clearance: None,
        }
    }
}

impl Aircraft {
    pub(crate) fn airspeed_mps(&self) -> f32 {
        self.speed
    }
    pub(crate) fn exterior_pose(&self) -> Option<Camera> {
        self.pilot
    }
    pub(crate) fn exit(self, camera: &mut Camera) {
        if let Some(pilot) = self.pilot {
            camera.position = pilot.position;
            camera.yaw = pilot.yaw;
            camera.pitch = pilot.pitch;
        }
        camera.roll = 0.;
    }
    fn clear_air(&mut self, world: &World, position: DVec3, reach: f64) -> bool {
        if reach > 32. {
            return false;
        }
        if self.clearance.as_ref().is_none_or(|old| {
            old.source.revision() != world.revision()
                || !old.source.shares_source_lineage(world)
                || (position.x - old.center.x).abs() > 32.
                || (position.z - old.center.z).abs() > 32.
        }) {
            self.clearance = Some(SkyClearance::new(world, position));
        }
        position.y - reach > self.clearance.as_ref().unwrap().ceiling
    }
    pub(crate) fn toggle_view(&mut self, camera: &mut Camera, world: &World) {
        if let Some(pilot) = self.pilot.take() {
            *camera = pilot;
        } else {
            self.follow(camera, world);
        }
    }
    fn follow(&mut self, camera: &mut Camera, world: &World) {
        self.pilot = Some(*camera);
        let forward = camera.direction().as_dvec3();
        let offset = -forward * 15. + glam::DVec3::Y * 4.2;
        let direction = offset.normalize();
        let distance = if self.clear_air(world, camera.position, offset.length()) {
            offset.length()
        } else {
            frontier_world::raycast_solid(
                world,
                camera.position.to_array(),
                direction.to_array(),
                offset.length(),
            )
            .map_or(offset.length(), |h| (h.distance - 0.20).max(0.))
        };
        let target = camera.position + forward * 6.;
        camera.position += direction * distance;
        let look = (target - camera.position).normalize();
        camera.yaw = look.x.atan2(look.z) as f32;
        camera.pitch = look.y.asin() as f32;
        camera.roll *= 0.30;
    }

    /// Return false when the swept flight path meets a blocking native cell.
    pub(crate) fn advance(
        &mut self,
        camera: &mut Camera,
        world: &World,
        bank: f32,
        pitch: f32,
        throttle: f32,
        dt: f32,
    ) -> bool {
        let chase = if let Some(pilot) = self.pilot.take() {
            *camera = Camera {
                far: camera.far,
                fov_y: camera.fov_y,
                ..pilot
            };
            true
        } else {
            false
        };
        let start = *camera;
        self.speed = (self.speed + throttle * 28. * dt).clamp(24., 150.);
        camera.roll += (bank * 1.05 - camera.roll) * (1. - (-3. * dt).exp());
        camera.pitch = (camera.pitch + pitch * 0.6 * dt).clamp(-1.3, 1.3);
        camera.yaw -= 9.81 * camera.roll.tan() / self.speed * dt;
        let forward = camera.direction().as_dvec3().normalize();
        let distance = (self.speed * dt) as f64;
        camera.position += forward * distance;
        if !self.clear_air(world, start.position, distance + 6.) {
            let before = axes(start);
            let after = axes(*camera);
            let mut fraction = 1.0_f64;
            for probe in [
                Vec3::ZERO,
                Vec3::new(0., 0., 2.28),
                Vec3::new(0., 0.14, -4.65),
                Vec3::new(-5.55, 1.12, 0.),
                Vec3::new(5.55, 1.12, 0.),
                Vec3::new(-0.94, -1.32, 0.34),
                Vec3::new(0.94, -1.32, 0.34),
            ] {
                let origin = start.position + (before * probe).as_dvec3();
                let sweep = camera.position + (after * probe).as_dvec3() - origin;
                let length = sweep.length();
                if let Some(hit) = frontier_world::raycast_solid(
                    world,
                    origin.to_array(),
                    sweep.to_array(),
                    length,
                ) {
                    fraction = fraction
                        .min(((hit.distance - voxel_size()).max(0.) / length).clamp(0., 1.));
                }
            }
            if fraction < 1. {
                camera.position = start.position + forward * distance * fraction;
                camera.roll = 0.;
                return false;
            }
        }
        if chase {
            self.follow(camera, world);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{Cell, FbmParams, Material};
    #[test]
    fn plane_banks_and_its_sweep_stops_at_an_edited_cell() {
        let mut world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let mut camera = Camera::fixture();
        camera.position = glam::DVec3::new(0.02, 200., 0.);
        camera.pitch = 0.;
        camera.yaw = 0.;
        let mut plane = Aircraft::default();
        world.set(Cell::from_meters([0.02, 200., 2.]), Material::Stone);
        assert!(!plane.advance(&mut camera, &world, 0., 0., 0., 0.1));
        assert!(camera.position.z < 2.);
        camera.position.y += 1.;
        assert!(plane.advance(&mut camera, &world, 1., 0., 0., 0.1));
        assert!(camera.roll > 0. && camera.yaw < 0.);
    }

    #[test]
    fn new_wingtip_obstacle_invalidates_cached_open_sky() {
        let mut world = World::fbm(
            11,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0.02, 200., 0.);
        camera.yaw = 0.;
        camera.pitch = 0.;
        let mut plane = Aircraft::default();
        assert!(plane.advance(&mut camera, &world, 0., 0., 0., 0.02));
        let blocker = Cell::from_meters((camera.position + DVec3::new(5.55, 1.12, 1.)).to_array());
        world.set(blocker, Material::Stone);
        assert!(!plane.advance(&mut camera, &world, 0., 0., 0., 0.05));
    }
}
