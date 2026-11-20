//! A coordinated-turn flight controller. All distances and rates use metres,
//! seconds and radians independently of the native voxel size.
use crate::Camera;
use frontier_world::World;

pub(crate) struct Aircraft {
    speed: f32,
}

impl Default for Aircraft {
    fn default() -> Self {
        Self { speed: 65. }
    }
}

impl Aircraft {
    pub(crate) fn airspeed_mps(&self) -> f32 {
        self.speed
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
        self.speed = (self.speed + throttle * 28. * dt).clamp(24., 150.);
        camera.roll += (bank * 1.05 - camera.roll) * (1. - (-3. * dt).exp());
        camera.pitch = (camera.pitch + pitch * 0.6 * dt).clamp(-1.3, 1.3);
        camera.yaw -= 9.81 * camera.roll.tan() / self.speed * dt;
        let forward = camera.direction().as_dvec3().normalize();
        let distance = (self.speed * dt) as f64;
        if let Some(hit) = frontier_world::raycast_solid(
            world,
            camera.position.to_array(),
            forward.to_array(),
            distance,
        ) {
            camera.position += forward * (hit.distance - frontier_world::voxel_size()).max(0.);
            camera.roll = 0.;
            return false;
        }
        camera.position += forward * distance;
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
}
