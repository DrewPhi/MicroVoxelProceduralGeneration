//! A shared player pose for first- and third-person walking.
use crate::{walking::EYE_HEIGHT, Camera};
use frontier_world::{Cell, Material, World, VOXEL_SIZE};
use glam::{DQuat, DVec3, Vec3Swizzles};

pub const RIM: f64 = 0.295;
pub const DEPTH: f64 = 0.070;
pub const BASE_RADIUS: f64 = 0.68;

pub struct Hands {
    pub aim: Camera,
    pub crouch: f64,
    pub lean: f64,
    pub flinch: f64,
}

#[derive(Clone, Copy, Default)]
struct Foot {
    position: DVec3,
    from: DVec3,
    target: DVec3,
}

#[derive(Default)]
pub struct Avatar {
    pub third_person: bool,
    feet: [Foot; 2],
    active_foot: usize,
    swing: f64,
    previous_root: Option<DVec3>,
    crouch: f64,
    lean: f64,
    angular_speed: f64,
    cooldown: f64,
    trace_elapsed: f64,
    trace: bool,
}

impl Avatar {
    pub fn new() -> Self {
        Self {
            trace: std::env::var_os("FRONTIER_PLAYER_TRACE").is_some(),
            ..Default::default()
        }
    }

    pub fn reset(&mut self) {
        *self = Self {
            third_person: self.third_person,
            trace: self.trace,
            ..Default::default()
        };
    }

    pub fn hands(&self, aim: Camera) -> Hands {
        Hands {
            aim,
            crouch: self.crouch,
            lean: self.lean,
            flinch: (self.lean * 1.95).clamp(0., 2.72),
        }
    }

    pub fn move_intent(
        &mut self,
        world: &World,
        eye: DVec3,
        mut velocity: DVec3,
        careful: bool,
        grounded: bool,
        dt: f64,
    ) -> DVec3 {
        self.cooldown = (self.cooldown - dt).max(0.);
        if careful {
            velocity *= 0.40;
        }
        let forward = velocity.normalize_or_zero();
        if grounded && self.cooldown == 0. && velocity.length() >= 2.65 {
            let root = eye - DVec3::Y * EYE_HEIGHT;
            let side = forward.cross(DVec3::Y);
            // Check the toe's approach to a low obstacle before stepping the
            // body capsule up. Test both boot tracks, including their edges.
            let reach = 0.29 + velocity.length().min(6.) * dt;
            'contact: for lateral in [-0.20, -0.10, 0.0, 0.10, 0.20] {
                for advance in [0.19, 0.25, reach] {
                    let toe = root + forward * advance + side * lateral;
                    let lower = Cell::from_meters((toe + DVec3::Y * 0.066).to_array());
                    let upper = Cell::from_meters((toe + DVec3::Y * 0.26).to_array());
                    if world.blocking_material(lower) == Material::Stone
                        && world.ground_height_m(toe.x, toe.z) < toe.y + 0.04
                        && !world.blocking_material(upper).is_blocking()
                    {
                        self.angular_speed += 6.7 + velocity.length() * 0.46;
                        self.cooldown = 1.3;
                        if self.trace {
                            eprintln!(
                                "player toe contact at {:?}, speed {:.2}",
                                lower,
                                velocity.length()
                            );
                        }
                        break 'contact;
                    }
                }
            }
        }
        for _ in 0..4 {
            let step = dt * 0.25;
            self.angular_speed += (-10. * self.lean - 3.7 * self.angular_speed) * step;
            self.lean = (self.lean + self.angular_speed * step).clamp(0., 1.51);
        }
        self.crouch += (f64::from(careful) - self.crouch) * (dt * 11.).min(1.);
        let bracing = (self.lean * 0.73 + self.angular_speed.max(0.) * 0.075).clamp(0., 0.89);
        velocity * (1. - bracing)
    }

    pub fn advance_pose(&mut self, world: &World, eye: DVec3, yaw: f32, grounded: bool, dt: f64) {
        let root = eye - DVec3::Y * EYE_HEIGHT;
        let forward = heading(yaw);
        let right = forward.cross(DVec3::Y);
        let old = self.previous_root.replace(root);
        let mut velocity = (root - old.unwrap_or(root)) / dt.max(0.001);
        velocity.y = 0.;
        let speed = velocity.length().min(6.);
        let stance = |i| root + right * if i == 0 { -0.148 } else { 0.148 };
        if old.is_none_or(|p| p.distance_squared(root) > 1.) || !grounded {
            for i in 0..2 {
                self.feet[i].position = if grounded {
                    sole(world, stance(i), root.y)
                } else {
                    stance(i) + DVec3::Y * 0.10
                };
            }
            self.swing = 0.;
            return;
        }
        let i = self.active_foot;
        let anchor = stance(i);
        if self.swing == 0.
            && self.feet[i].position.xz().distance(anchor.xz()) > (0.14 + speed * 0.015)
        {
            let target = anchor + velocity.normalize_or_zero() * (0.12 + speed * 0.055);
            self.feet[i].from = self.feet[i].position;
            self.feet[i].target = sole(world, target, root.y);
            self.swing = 0.001;
        }
        if self.swing > 0. {
            let duration = (0.34 - speed * 0.032 + self.crouch * 0.08).max(0.14);
            self.swing = (self.swing + dt / duration).min(1.);
            let t = self.swing;
            let foot = &mut self.feet[i];
            let mut position = foot.from.lerp(foot.target, t * t * (3. - 2. * t));
            let clearance = 0.075 + self.crouch * 0.11 + self.lean * 0.065;
            let nearest = sole(world, position, root.y);
            position.y = position.y.max(nearest.y) + (std::f64::consts::PI * t).sin() * clearance;
            foot.position = position;
            if t >= 1. {
                self.swing = 0.;
                self.active_foot = 1 - i;
            }
        }
    }

    pub fn boots(&self) -> [DVec3; 2] {
        self.feet.map(|foot| foot.position)
    }

    pub fn view(&self, world: &World, aim: Camera) -> Camera {
        let forward = heading(aim.yaw);
        let right = forward.cross(DVec3::Y);
        let eye = aim.position - DVec3::Y * (self.crouch * 0.46 + self.lean * 0.44);
        let mut camera = aim;
        if self.third_person {
            let pivot = eye + forward * (0.30 * self.lean) - DVec3::Y * 0.38;
            let boom = -forward * 2.9 + right * 2.05 + DVec3::Y * 1.56;
            let length = boom.length();
            let ray = boom / length;
            let clearance = [-0.13, 0., 0.13].into_iter().fold(length, |space, offset| {
                let origin = pivot + right * offset;
                frontier_world::raycast_solid(world, origin.to_array(), ray.to_array(), length)
                    .map_or(space, |hit| {
                        space.min(
                            (DVec3::from_array(hit.cell.center_m()).distance(origin) - 0.22)
                                .max(0.25),
                        )
                    })
            });
            camera.position = pivot + ray * clearance;
            let target = pivot + forward * 0.97 + DVec3::Y * (aim.pitch as f64 * 0.25 - 0.12);
            let dir = (target - camera.position).normalize();
            camera.yaw = dir.x.atan2(dir.z) as f32;
            camera.pitch = dir.y.asin() as f32;
        } else {
            camera.position = eye + forward * (0.075 + self.lean * 0.26);
            camera.pitch = (aim.pitch - (self.lean * 0.29) as f32).clamp(-1.55, 1.55);
        }
        camera
    }

    pub fn trace(&mut self, aim: Camera, water: u32, spilled: u64, dt: f64) {
        if self.trace {
            self.trace_elapsed += dt;
            if self.trace_elapsed >= 0.1 {
                self.trace_elapsed -= 0.1;
                eprintln!(
                    "player [{:.5},{:.5},{:.5},{:.5},{:.5},{:.3},{},{}]",
                    aim.position.x,
                    aim.position.y,
                    aim.position.z,
                    aim.yaw,
                    aim.pitch,
                    self.lean,
                    water,
                    spilled
                );
            }
        }
    }
}

pub fn heading(yaw: f32) -> DVec3 {
    let (sin, cos) = (yaw as f64).sin_cos();
    DVec3::new(sin, 0., cos)
}

fn sole(world: &World, mut point: DVec3, around: f64) -> DVec3 {
    let c = Cell::from_meters([point.x, around + 0.24, point.z]);
    for y in (c.y - 15..=c.y).rev() {
        if world.blocking_material(Cell { y, ..c }).is_blocking() {
            point.y = (y + 1) as f64 * VOXEL_SIZE;
            return point;
        }
    }
    point.y = around;
    point
}

#[derive(Clone, Copy)]
pub struct VesselPose {
    pub center: DVec3,
    pub rotation: DQuat,
}

impl VesselPose {
    pub fn point(self, local: DVec3) -> DVec3 {
        self.center + self.rotation * local
    }
}

impl Hands {
    pub fn vessel(&self, dip: f64, tilt: f64) -> VesselPose {
        let pitch = self.aim.pitch as f64;
        let forward = heading(self.aim.yaw);
        let right = forward.cross(DVec3::Y);
        let reach = 0.78 + dip * 0.11 + self.lean * 0.25;
        let down = 0.51
            + self.crouch * 0.51
            + dip * (0.35 + (-pitch).clamp(0., 1.2) * 0.38)
            + self.lean * 0.21;
        let center =
            self.aim.position + forward * reach + right * (0.12 * (1. - dip)) - DVec3::Y * down;
        VesselPose {
            center,
            rotation: DQuat::from_rotation_y(self.aim.yaw as f64)
                * DQuat::from_rotation_x(tilt + self.flinch + dip * 0.065),
        }
    }
}

/// Place a small ledge of loose rocks just inland of the oasis in the desert game.
pub fn rocks(world: &mut World, start: [f64; 2]) {
    let Some(terrain) = world.fbm_terrain() else {
        return;
    };
    let oasis = frontier_world::desert::oasis(world.seed(), 0, 0);
    let scale = terrain.params().horizontal_scale / 1800.;
    let begin = DVec3::new(start[0], 0., start[1]);
    let end = DVec3::new(oasis.x * scale, 0., oasis.z * scale);
    let course = (end - begin).normalize_or_zero();
    let Some(shore) = (0..=(end.distance(begin) / 0.1) as usize)
        .map(|step| begin + course * (step as f64 * 0.1))
        .find(|p| world.height_m(p.x, p.z) > world.ground_height_m(p.x, p.z) + 0.02)
    else {
        return;
    };
    if (shore - begin).length() < 4.5 {
        return;
    }
    let rock = shore - course * 3.4;
    let axis = course.cross(DVec3::Y);
    let start_cell = Cell::from_meters(rock.to_array());
    let mut edits = Vec::new();
    for z in -9..=9 {
        for x in -9..=9 {
            let cell = Cell {
                x: start_cell.x + x,
                y: 0,
                z: start_cell.z + z,
            };
            let p = DVec3::from_array(cell.center_m());
            let delta = p - rock;
            let r = (delta.dot(course) / 0.22).powi(2) + (delta.dot(axis) / 0.43).powi(2);
            if r >= 1. {
                continue;
            }
            let column = world.column(cell.x, cell.z);
            let surface = (column.ground_height_m() / VOXEL_SIZE).round() as i64;
            let rise = if r < 0.72 { 3 } else { 1 };
            for y in surface..surface + rise {
                edits.push((Cell { y, ..cell }, Material::Stone));
            }
        }
    }
    world.set_batch(edits);
    if std::env::var_os("FRONTIER_PLAYER_TRACE").is_some() {
        eprintln!(
            "player course [{},{},{},{},{},{}]",
            begin.x, begin.z, shore.x, shore.z, rock.x, rock.z
        );
    }
}
