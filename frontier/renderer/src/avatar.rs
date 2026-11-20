//! A shared player pose for first- and third-person walking.
use crate::{walking::EYE_HEIGHT, Camera};
use frontier_world::{Cell, Material, World, VOXEL_SIZE};
use glam::{DQuat, DVec3, Vec3Swizzles};

pub const TICK: f64 = 1. / 120.;
pub const BOOT_HALF: DVec3 = DVec3::new(0.079, 0.063, 0.14);
pub const BOOT_FORWARD: f64 = 0.044;

pub struct Hands {
    pub aim: Camera,
    pub crouch: f64,
    pub lean: f64,
}

#[derive(Clone, Copy, Default)]
struct Foot {
    position: DVec3,
    from: DVec3,
    target: DVec3,
    elevation: f64,
    blocked_y: Option<f64>,
}

#[derive(Default)]
pub struct Avatar {
    pub third_person: bool,
    feet: [Foot; 2],
    active_foot: usize,
    swing: f64,
    previous_root: Option<DVec3>,
    crouch: f64,
    crouch_speed: f64,
    lean: f64,
    angular_speed: f64,
    momentum: DVec3,
    body_height: Option<f64>,
    body_vertical_speed: f64,
    contact: Option<Cell>,
    support_failed: bool,
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
        }
    }

    pub fn walk_tick(
        &mut self,
        world: &World,
        walking: &mut crate::walking::WalkingCamera,
        mut aim: Camera,
        intent: DVec3,
        careful: bool,
        jump: bool,
        smooth: bool,
    ) -> DVec3 {
        let presented = aim.position;
        aim.position = walking.physical_eye().map_or(presented, DVec3::from_array);
        let (motion, step_cells) = self.gait(world, aim, intent, careful, walking.grounded());
        let next = walking.advance_avatar(
            world,
            presented.to_array(),
            [motion.x, motion.z],
            jump,
            TICK,
            smooth,
            step_cells,
        );
        let actual = walking.physical_eye().map_or(presented, DVec3::from_array);
        let velocity = (actual - aim.position) / TICK;
        self.momentum = DVec3::new(velocity.x, 0., velocity.z);
        let current_height = self
            .body_height
            .filter(|previous| (actual.y - previous).abs() < 1.5);
        if let Some(height) = current_height {
            // The capsule can select a discrete support step. The moving
            // pelvis carries its previous velocity into the next foothold.
            self.body_vertical_speed +=
                ((actual.y - height) * 112. - self.body_vertical_speed * 20.) * TICK;
            self.body_height = Some(height + self.body_vertical_speed * TICK);
        } else {
            self.body_vertical_speed = 0.;
            self.body_height = Some(actual.y);
        }
        DVec3::from_array(next)
    }

    fn gait(
        &mut self,
        world: &World,
        aim: Camera,
        mut intent: DVec3,
        careful: bool,
        grounded: bool,
    ) -> (DVec3, u32) {
        self.crouch_speed +=
            ((f64::from(careful) - self.crouch) * 24. - self.crouch_speed * 10.) * TICK;
        self.crouch = (self.crouch + self.crouch_speed * TICK).clamp(0., 1.);
        intent *= 1. - self.crouch * 0.60;
        let root = aim.position - DVec3::Y * EYE_HEIGHT;
        let forward = heading(aim.yaw);
        let right = forward.cross(DVec3::Y);
        let old = self.previous_root.replace(root);
        let stance = |i| root + right * if i == 0 { -0.145 } else { 0.145 };
        if old.is_none_or(|p| p.distance_squared(root) > 1.) || !grounded {
            self.swing = 0.;
            self.contact = None;
            self.support_failed = false;
            for i in 0..2 {
                self.feet[i].position = sole(world, stance(i), root.y, 0.09)
                    .unwrap_or_else(|| stance(i) + DVec3::Y * 0.10);
            }
        }

        let speed = intent.length();
        let direction = intent.normalize_or_zero();
        let i = self.active_foot;
        let other = 1 - i;
        let capture = root + self.momentum / (9.81_f64 / 0.88).sqrt();
        let anchor = stance(i);
        if grounded
            && self.swing == 0.
            && (self.feet[i].position.xz().distance(anchor.xz()) > 0.13
                || (speed > 0.12 && self.momentum.length() < 0.09))
        {
            let reach = (0.20 + speed * 0.058).min(0.48);
            let target = anchor + direction * reach;
            let foot = &mut self.feet[i];
            foot.from = foot.position;
            foot.target = boot_support(world, target, forward, root.y, 0.19).unwrap_or(target);
            let obstacle = foot.blocked_y.take().filter(|top| *top <= root.y + 0.22);
            foot.elevation = foot.position.y.max(foot.target.y) + 0.064 + self.crouch * 0.105;
            if let Some(top) = obstacle {
                foot.elevation = foot.elevation.max(top + 0.065);
            }
            self.swing = TICK;
        }

        let planted = self.feet[other].position;
        let mut higher_support = planted.y;
        if grounded && self.swing > 0. {
            let foot = &mut self.feet[i];
            let duration = (0.30 - speed * 0.035 + self.crouch * 0.11).max(0.14);
            self.swing = (self.swing + TICK / duration).min(1.);
            let t = self.swing;
            let slide = smoothstep((t - 0.19) / 0.67);
            let mut desired = foot.from.lerp(foot.target, slide);
            desired.y = if t < 0.36 {
                foot.from.y + (foot.elevation - foot.from.y) * smoothstep(t / 0.36)
            } else {
                foot.elevation + (foot.target.y - foot.elevation) * smoothstep((t - 0.65) / 0.35)
            };
            let delta = desired - foot.position;
            let hit = sweep_boot(foot.position, delta, forward, |cell| {
                world.blocking_material(cell)
            });
            if let Some(hit) = hit {
                foot.position += delta * (hit.fraction - 1e-5).max(0.);
                if hit.normal.y.abs() < 0.5 {
                    let top = (hit.cell.y + 1) as f64 * VOXEL_SIZE;
                    foot.blocked_y = Some(foot.blocked_y.map_or(top, |last| last.max(top)));
                }
                let blocked = (-delta.dot(hit.normal) / TICK).max(0.);
                let incoming = (-self.momentum.dot(hit.normal)).max(0.);
                if self.contact != Some(hit.cell) && hit.normal.y.abs() < 0.5 && blocked > 0.2 {
                    // A canceled step removes support ahead of the moving
                    // pelvis. Use the contact direction and loss of velocity
                    // to drive its angular response about the planted foot.
                    let recoverable = planted.xz().distance(capture.xz()) < 0.31;
                    let impulse = blocked.min(incoming) * (1. - hit.normal.y.abs());
                    self.angular_speed += impulse * (-hit.normal.dot(forward)).max(0.) / 0.88
                        * if recoverable { 0.58 } else { 1.72 };
                    self.support_failed = !recoverable && impulse > 0.0;
                    if self.trace {
                        eprintln!(
                            "player boot contact {:?} normal {:?} blocked {:.2} recovery {} material {:?}",
                            hit.cell, hit.normal, blocked, recoverable, world.blocking_material(hit.cell)
                        );
                    }
                }
                self.contact = Some(hit.cell);
            } else {
                foot.position = desired;
                self.contact = None;
                // A clearing swing can accept an available foothold before the
                // capsule reaches the same ledge.
                if t >= 0.53 && foot.position.y >= foot.target.y - 0.005 {
                    higher_support = higher_support.max(foot.target.y);
                }
            }
            if t >= 1. {
                let planted = sole(world, foot.position, foot.position.y, 0.025)
                    .is_some_and(|p| (p.y - foot.position.y).abs() < VOXEL_SIZE * 0.55);
                if planted {
                    higher_support = higher_support.max(foot.position.y);
                    self.support_failed = false;
                }
                self.active_foot = other;
                self.swing = 0.;
            }
        }
        if self.support_failed && self.lean > 0.0 {
            self.angular_speed += (9.81 / 0.88 * self.lean.sin()) * TICK;
        }
        let recovering = !self.support_failed || self.lean > 0.96 || speed < 0.1;
        let restoring = if recovering { 20. } else { 10. };
        self.angular_speed += (-restoring * self.lean - 5.4 * self.angular_speed) * TICK;
        self.lean = (self.lean + self.angular_speed * TICK).clamp(0., 1.25);
        if self.lean == 0. && self.angular_speed < 0. {
            self.angular_speed = 0.;
            self.support_failed = false;
        }
        let movement = self
            .momentum
            .lerp(intent, if self.support_failed { 0.055 } else { 0.21 });
        let step_cells = ((higher_support - root.y + 0.002) / VOXEL_SIZE)
            .floor()
            .clamp(0., 3.) as u32;
        (movement, step_cells)
    }

    pub fn body_aim(&self, mut aim: Camera) -> Camera {
        if let Some(height) = self.body_height {
            aim.position.y = height;
        }
        aim
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

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn boot_support(
    world: &World,
    point: DVec3,
    forward: DVec3,
    around: f64,
    rise: f64,
) -> Option<DVec3> {
    let right = forward.cross(DVec3::Y);
    let mut highest = None;
    for distance in [-0.075, 0.04, 0.15] {
        for lateral in [-0.059, 0., 0.059] {
            let sample = point + forward * distance + right * lateral;
            if let Some(support) = sole(world, sample, around, rise) {
                if highest.is_none_or(|y| support.y > y) {
                    highest = Some(support.y);
                }
            }
        }
    }
    highest.map(|y| DVec3::new(point.x, y, point.z))
}

fn sole(world: &World, mut point: DVec3, around: f64, max_rise: f64) -> Option<DVec3> {
    let c = Cell::from_meters([point.x, around + max_rise - 0.0001, point.z]);
    for y in (c.y - 10..=c.y).rev() {
        if world.blocking_material(Cell { y, ..c }).is_blocking() {
            point.y = (y + 1) as f64 * VOXEL_SIZE;
            return Some(point);
        }
    }
    None
}

struct BootHit {
    cell: Cell,
    normal: DVec3,
    fraction: f64,
}

/// Swept oriented boot against exact voxel boxes. With yaw-only rotation,
/// the separating axes are the vertical axis and both horizontal edge pairs.
fn sweep_boot(
    sole: DVec3,
    delta: DVec3,
    forward: DVec3,
    mut material: impl FnMut(Cell) -> Material,
) -> Option<BootHit> {
    let right = forward.cross(DVec3::Y);
    let center = sole + DVec3::Y * (BOOT_HALF.y + 0.00001) + forward * BOOT_FORWARD;
    let extent = right.abs() * BOOT_HALF.x + DVec3::Y * BOOT_HALF.y + forward.abs() * BOOT_HALF.z;
    let end = center + delta;
    let min = Cell::from_meters((center.min(end) - extent + DVec3::splat(1e-7)).to_array());
    let max = Cell::from_meters((center.max(end) + extent - DVec3::splat(1e-7)).to_array());
    let axes = [DVec3::X, DVec3::Y, DVec3::Z, forward, right];
    let mut best: Option<BootHit> = None;
    for z in min.z..=max.z {
        for y in min.y..=max.y {
            for x in min.x..=max.x {
                let cell = Cell { x, y, z };
                if !material(cell).is_blocking() {
                    continue;
                }
                let relative = center - DVec3::from_array(cell.center_m());
                let mut enter: f64 = 0.;
                let mut exit: f64 = 1.;
                let mut normal = DVec3::ZERO;
                for axis in axes {
                    let span = VOXEL_SIZE * 0.5 * axis.abs().element_sum()
                        + BOOT_HALF.x * axis.dot(right).abs()
                        + BOOT_HALF.y * axis.y.abs()
                        + BOOT_HALF.z * axis.dot(forward).abs();
                    let along = relative.dot(axis);
                    let speed = delta.dot(axis);
                    if speed.abs() < 1e-9 {
                        if along.abs() >= span {
                            exit = -1.;
                            break;
                        }
                        continue;
                    }
                    let a = (-span - along) / speed;
                    let b = (span - along) / speed;
                    let near = a.min(b);
                    if near >= enter {
                        enter = near;
                        normal = -axis * speed.signum();
                    }
                    exit = exit.min(a.max(b));
                }
                if enter <= exit
                    && normal != DVec3::ZERO
                    && best.as_ref().is_none_or(|hit| enter < hit.fraction)
                {
                    best = Some(BootHit {
                        cell,
                        fraction: enter,
                        normal,
                    });
                }
            }
        }
    }
    best
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

    pub fn local(self, point: DVec3) -> DVec3 {
        self.rotation.inverse() * (point - self.center)
    }
}

impl Hands {
    pub fn torso(&self) -> DQuat {
        DQuat::from_rotation_y(self.aim.yaw as f64) * DQuat::from_rotation_x(self.lean * 0.78)
    }

    pub fn chest(&self) -> DVec3 {
        self.aim.position - DVec3::Y * (0.43 + self.lean * 0.31 + self.crouch * 0.51)
            + heading(self.aim.yaw) * (self.lean * 0.36)
    }

    pub fn vessel(&self, dip: f64, tilt: f64) -> VesselPose {
        let pitch = self.aim.pitch as f64;
        let torso = self.torso();
        let reach = 0.78 + dip * 0.11;
        let down = 0.08 + dip * (0.35 + (-pitch).clamp(0., 1.2) * 0.38);
        VesselPose {
            center: self.chest() + torso * DVec3::new(-0.12 * (1. - dip), -down, reach),
            rotation: torso * DQuat::from_rotation_x(tilt + dip * 0.065),
        }
    }
}

/// Place a small ledge of loose rocks just inland of the oasis in the desert game.
pub fn pond_fixture(world: &mut World, start: [f64; 2]) {
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
    // A level patch of shoreline isolates the obstacle from the approach's
    // unrelated steep stairs. It is ordinary editable terrain in this fixture.
    let bank = shore - course * 0.62;
    let floor = (world.ground_height_m(bank.x, bank.z) / VOXEL_SIZE).round() as i64;
    let shoreline_cell = Cell::from_meters(shore.to_array());
    let mut edits = Vec::new();
    for z in -130..=130 {
        for x in -130..=130 {
            let cell = Cell {
                x: shoreline_cell.x + x,
                y: 0,
                z: shoreline_cell.z + z,
            };
            let point = DVec3::from_array(cell.center_m());
            let from_shore = point - shore;
            let inland = -from_shore.dot(course);
            let side = from_shore.dot(axis).abs();
            if !(0.23..7.).contains(&inland) || side > 1.55 {
                continue;
            }
            let column = world.column(cell.x, cell.z);
            let ground = (column.ground_height_m() / VOXEL_SIZE).round() as i64;
            let strength =
                (1. - smoothstep((inland - 3.95) / 3.05)) * (1. - smoothstep((side - 0.85) / 0.70));
            let trail_top = (ground as f64 + strength * (floor - ground) as f64).round() as i64;
            for y in trail_top.min(ground)..trail_top.max(ground) {
                let material = if y < trail_top {
                    Material::Sand
                } else {
                    Material::Air
                };
                edits.push((Cell { y, ..cell }, material));
            }
            let from_rock = point - rock;
            let footprint =
                (from_rock.dot(course) / 0.085).powi(2) + (from_rock.dot(axis) / 0.44).powi(2);
            if footprint < 1. {
                let height = if footprint < 0.86 { 2 } else { 1 };
                for y in trail_top..trail_top + height {
                    edits.push((Cell { y, ..cell }, Material::Stone));
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_sweep_catches_voxel_edges_and_clears_with_height() {
        let ledge = Cell { x: 0, y: 0, z: 4 };
        for barrier in [Material::Stone, Material::Wood, Material::Sand] {
            let sample = |cell| {
                if cell == ledge {
                    barrier
                } else {
                    Material::Air
                }
            };
            let start = DVec3::new(VOXEL_SIZE * 0.5, 0., 0.);
            let travel = DVec3::Z * 0.24;
            let hit = sweep_boot(start, travel, DVec3::Z, sample).unwrap();
            assert_eq!(hit.cell, ledge);
            assert_eq!(hit.normal, -DVec3::Z);
            assert!(hit.fraction > 0. && hit.fraction < 1.);
            assert!(sweep_boot(start + DVec3::Y * VOXEL_SIZE, travel, DVec3::Z, sample).is_none());
        }
    }

    #[test]
    fn fixed_input_ticks_are_independent_of_the_camera_mode() {
        let mut world = World::new(19);
        let ground = 1600;
        world.set_batch((-8..=8).flat_map(|x| {
            (-5..=45).map(move |z| {
                (
                    Cell {
                        x,
                        y: ground - 1,
                        z,
                    },
                    Material::Sand,
                )
            })
        }));
        world.set_batch((-6..=6).flat_map(|x| {
            (10..=10).flat_map(move |z| {
                (0..2).map(move |y| {
                    (
                        Cell {
                            x,
                            y: ground + y,
                            z,
                        },
                        Material::Wood,
                    )
                })
            })
        }));
        let mut first = Avatar::default();
        let mut third = Avatar {
            third_person: true,
            ..Default::default()
        };
        let mut first_camera = Camera::fixture();
        first_camera.position = DVec3::new(0., ground as f64 * VOXEL_SIZE + EYE_HEIGHT, 0.);
        first_camera.yaw = 0.;
        let mut third_camera = first_camera;
        let mut first_walker = crate::walking::WalkingCamera::default();
        let mut third_walker = crate::walking::WalkingCamera::default();
        let mut peak_lean = 0.0_f64;
        for _ in 0..190 {
            first_camera.position = first.walk_tick(
                &world,
                &mut first_walker,
                first_camera,
                DVec3::Z * 3.,
                false,
                false,
                true,
            );
            third_camera.position = third.walk_tick(
                &world,
                &mut third_walker,
                third_camera,
                DVec3::Z * 3.,
                false,
                false,
                true,
            );
            let _ = first.view(&world, first_camera);
            let _ = third.view(&world, third_camera);
            assert_eq!(first_camera.position, third_camera.position);
            assert_eq!(first.boots(), third.boots());
            assert_eq!(first.lean, third.lean);
            peak_lean = peak_lean.max(first.lean);
        }
        assert!(
            peak_lean > 0.04,
            "blocked foot should disturb balance, observed {peak_lean}"
        );
    }
}
