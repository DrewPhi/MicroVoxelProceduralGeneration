//! A coordinated-turn flight controller. All distances and rates use metres,
//! seconds and radians independently of the native voxel size.
use crate::Camera;
use frontier_world::{voxel_size, World};
use glam::{DVec3, Mat3, Vec3};

pub(crate) fn axes(pose: Camera) -> Mat3 {
    let forward = pose.direction().normalize();
    let lateral = pose.view_up().cross(forward).normalize();
    Mat3::from_cols(lateral, forward.cross(lateral), forward)
}

const WHEELS: [Vec3; 2] = [Vec3::new(-0.94, -1.32, 0.34), Vec3::new(0.94, -1.32, 0.34)];
const BODY: [Vec3; 5] = [
    Vec3::ZERO,
    Vec3::new(0., 0., 2.28),
    Vec3::new(0., 0.14, -4.65),
    Vec3::new(-5.55, 1.12, 0.),
    Vec3::new(5.55, 1.12, 0.),
];
const ROTATION_SPEED: f32 = 29.;

struct Contact {
    fraction: f64,
    wheel_on_top: bool,
}
fn swept_contact(world: &World, start: Camera, end: Camera) -> Option<Contact> {
    let before = axes(start);
    let after = axes(end);
    let mut contact: Option<Contact> = None;
    for (wheel, probe) in BODY
        .map(|p| (false, p))
        .into_iter()
        .chain(WHEELS.map(|p| (true, p)))
    {
        let origin = start.position + (before * probe).as_dvec3();
        let travel = end.position + (after * probe).as_dvec3() - origin;
        let length = travel.length();
        if length <= f64::EPSILON {
            continue;
        }
        if let Some(hit) =
            frontier_world::raycast_solid(world, origin.to_array(), travel.to_array(), length)
        {
            let fraction = ((hit.distance - voxel_size().min(0.06)).max(0.) / length).clamp(0., 1.);
            let wheel_on_top = wheel && hit.normal == [0, 1, 0];
            if contact
                .as_ref()
                .is_none_or(|c| fraction < c.fraction || (fraction == c.fraction && !wheel_on_top))
            {
                contact = Some(Contact {
                    fraction,
                    wheel_on_top,
                });
            }
        }
    }
    contact
}

// Probe both tracks in the canonical edited volume. This works on bridges,
// raised runways and floors below an overhang as well as ordinary ground.
fn runway_height(world: &World, mut pose: Camera) -> Option<f64> {
    pose.pitch = 0.;
    pose.roll = 0.;
    let frame = axes(pose);
    let mut levels = [0.; 2];
    for (level, wheel) in levels.iter_mut().zip(WHEELS) {
        let origin = pose.position + (frame * wheel).as_dvec3() + DVec3::Y * 0.48;
        let hit = frontier_world::raycast_solid(world, origin.to_array(), [0., -1., 0.], 1.)?;
        if hit.normal != [0, 1, 0] {
            return None;
        }
        let above = frontier_world::Cell {
            y: hit.cell.y.checked_add(1)?,
            ..hit.cell
        };
        if world.material(above) == frontier_world::Material::Water {
            return None;
        }
        *level = above.y as f64 * voxel_size();
    }
    ((levels[0] - levels[1]).abs() <= 0.22).then(|| levels[0].max(levels[1]) + 1.32 + 0.014)
}

pub(crate) struct Aircraft {
    speed: f32,
    grounded: bool,
    sink: f32,
    // In chase view the viewer camera follows this physical aircraft pose.
    pilot: Option<Camera>,
    clearance: crate::sky_clearance::Airspace,
}

impl Default for Aircraft {
    fn default() -> Self {
        Self {
            speed: 65.,
            grounded: false,
            sink: 0.,
            pilot: None,
            clearance: Default::default(),
        }
    }
}

impl Aircraft {
    pub(crate) fn enter(world: &World, camera: &mut Camera) -> Self {
        let mut plane = Self::default();
        if let Some(y) = runway_height(world, *camera) {
            let level = Camera {
                position: DVec3::new(camera.position.x, y, camera.position.z),
                pitch: 0.,
                roll: 0.,
                ..*camera
            };
            if swept_contact(world, *camera, level).is_none() {
                *camera = level;
                plane.speed = 0.;
                plane.grounded = true;
            }
        }
        plane
    }
    pub(crate) fn on_ground(&self) -> bool {
        self.grounded
    }
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
        reach <= 32. && self.clearance.clear_above(world, position, 32., reach)
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
        let flying = if self.grounded && (self.speed < ROTATION_SPEED || pitch <= 0.) {
            self.taxi(camera, world, bank, throttle, dt);
            true
        } else {
            if self.grounded {
                self.grounded = false;
                camera.pitch = 0.10;
            }
            self.fly(camera, world, bank, pitch, throttle, dt)
        };
        if flying && chase {
            self.follow(camera, world);
        }
        flying
    }

    fn taxi(&mut self, camera: &mut Camera, world: &World, steer: f32, throttle: f32, dt: f32) {
        let start = *camera;
        let acceleration = if throttle >= 0. {
            7.0 * throttle
        } else {
            15.0 * throttle
        };
        self.speed = (self.speed + (acceleration - 0.45) * dt).clamp(0., 48.);
        let steering = (self.speed / 3.).min(1.) / (1. + self.speed / 12.);
        camera.yaw -= steer * steering * dt;
        camera.position += camera.direction().as_dvec3() * (self.speed * dt) as f64;
        let support = runway_height(world, *camera);
        if let Some(y) = support {
            camera.position.y = y;
        }
        if let Some(contact) = swept_contact(world, start, *camera) {
            camera.position = start.position.lerp(camera.position, contact.fraction);
            camera.yaw = start.yaw + (camera.yaw - start.yaw) * contact.fraction as f32;
            self.speed = 0.;
        } else if support.is_none() {
            self.grounded = false;
        }
    }

    fn fly(
        &mut self,
        camera: &mut Camera,
        world: &World,
        bank: f32,
        pitch: f32,
        throttle: f32,
        dt: f32,
    ) -> bool {
        let start = *camera;
        // A plane rolling off unsupported ground retains its actual speed.
        self.speed = (self.speed + throttle * 28. * dt).clamp(self.speed.min(24.), 150.);
        let stall = (1. - self.speed / 24.).clamp(0., 1.);
        self.sink = (self.sink + (9.81 * stall - 5. * (1. - stall)) * dt).clamp(0., 25.);
        camera.roll += (bank * 1.05 - camera.roll) * (1. - (-3. * dt).exp());
        camera.pitch = (camera.pitch + pitch * 0.6 * dt).clamp(-1.3, 1.3);
        camera.yaw -= 9.81 * camera.roll.tan() / self.speed.max(8.) * dt;
        let velocity =
            camera.direction().as_dvec3() * self.speed as f64 - DVec3::Y * self.sink as f64;
        let travel = velocity * dt as f64;
        camera.position += travel;
        if self.clear_air(world, start.position, travel.length() + 6.) {
            return true;
        }
        let Some(contact) = swept_contact(world, start, *camera) else {
            return true;
        };
        camera.position = start.position + travel * contact.fraction;
        if contact.wheel_on_top
            && (-3.5..=0.1).contains(&velocity.y)
            && self.speed <= 48.
            && camera.roll.abs() < 0.16
            && camera.pitch.abs() < 0.22
        {
            if let Some(y) = runway_height(world, *camera) {
                let level = Camera {
                    position: DVec3::new(camera.position.x, y, camera.position.z),
                    pitch: 0.,
                    roll: 0.,
                    ..*camera
                };
                if swept_contact(world, *camera, level).is_none() {
                    *camera = level;
                    self.grounded = true;
                    self.sink = 0.;
                    return true;
                }
            }
        }
        camera.roll = 0.;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{Cell, FbmParams, Material};
    fn runway() -> (World, Camera) {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(0., world.height_m(0., 0.) + 1.65, 0.);
        camera.yaw = 0.;
        camera.pitch = 0.;
        camera.roll = 0.;
        (world, camera)
    }
    #[test]
    fn wheels_land_brake_and_rotate_off_an_edited_runway() {
        let (mut world, mut camera) = runway();
        // Two strips of raised native blocks form an elevated test landing deck.
        let y = 6.;
        let deck = Cell::from_meters([0., y, 0.]);
        let end = Cell::from_meters([0., y, 190.]).z;
        for x in [-0.94, 0.94] {
            let track = Cell::from_meters([x, y, 0.]).x;
            world.set_batch(
                (track - 1..=track + 1).flat_map(|x| {
                    (-20..end).map(move |z| (Cell { x, y: deck.y, z }, Material::Wood))
                }),
            );
        }
        let top = (deck.y + 1) as f64 * voxel_size();
        camera.position.y = top + 1.52;
        camera.pitch = -0.055;
        let mut plane = Aircraft {
            speed: 30.,
            ..Default::default()
        };
        for _ in 0..6 {
            assert!(plane.advance(&mut camera, &world, 0., 0., 0., 0.05));
            if plane.grounded {
                break;
            }
        }
        assert!(plane.grounded);
        assert!((camera.position.y - (top + 1.334)).abs() < 1e-5);
        for _ in 0..48 {
            assert!(plane.advance(&mut camera, &world, 0., 0., -1., 0.05));
        }
        assert_eq!(plane.speed, 0.);
        let rest = camera.position;
        assert!(plane.advance(&mut camera, &world, 1., 0., 0., 0.05));
        assert_eq!(camera.position, rest);
        for _ in 0..96 {
            assert!(plane.advance(&mut camera, &world, 0., 0., 1., 0.05));
        }
        assert!(plane.grounded && plane.speed >= ROTATION_SPEED);
        assert!(plane.advance(&mut camera, &world, 0., 1., 1., 0.1));
        assert!(!plane.grounded && camera.position.y > rest.y && camera.pitch > 0.);
    }
    #[test]
    fn parked_plane_follows_edited_support_and_taxi_stops_at_a_wall() {
        let (mut world, mut camera) = runway();
        let mut plane = Aircraft::enter(&world, &mut camera);
        assert!(plane.grounded && plane.speed == 0.);
        let blocker = Cell::from_meters([0., camera.position.y, 4.]);
        world.set(blocker, Material::Stone);
        for _ in 0..40 {
            assert!(plane.advance(&mut camera, &world, 0., 0., 1., 0.05));
        }
        assert!(plane.grounded && plane.speed < 1. && camera.position.z + 2.28 < 4.);
        let floor = world.column(0, 0).height_cells() - 1;
        let ground_position = camera.position;
        for wheel in WHEELS {
            let c =
                Cell::from_meters((camera.position + (axes(camera) * wheel).as_dvec3()).to_array());
            world.set_batch(
                (floor - 24..=floor).map(|y| (Cell { x: c.x, y, z: c.z }, Material::Air)),
            );
        }
        assert!(plane.advance(&mut camera, &world, 0., 0., 0., 0.05));
        assert!(!plane.grounded);
        assert!(plane.advance(&mut camera, &world, 0., 0., 0., 0.05));
        assert!(camera.position.y < ground_position.y);
    }
    #[test]
    fn steep_impact_does_not_enter_ground_roll() {
        let (world, mut camera) = runway();
        camera.position.y += 1.;
        camera.pitch = -0.45;
        let mut plane = Aircraft::default();
        assert!(!(0..20).all(|_| plane.advance(&mut camera, &world, 0., 0., 0., 0.05)));
        assert!(!plane.grounded);
    }

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
