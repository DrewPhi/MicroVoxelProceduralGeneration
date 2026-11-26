//! Aircraft controls, terrain contacts and camera presentation. All distances and rates use metres,
//! seconds and radians independently of the native voxel size.
use crate::flight_feedback::Feedback;
use crate::Camera;
use crate::{aircraft_damage, destruction};
use frontier_world::{voxel_size, World};
use glam::{DVec3, Mat3, Quat, Vec3};

#[path = "airframe.rs"]
pub(super) mod airframe;
#[path = "flight_dynamics.rs"]
mod dynamics;
use dynamics::{Deflection, Dynamics, Forces, STEP};
pub(crate) use dynamics::{SectionFlow, WING_SECTIONS};

pub(crate) fn axes(pose: Camera) -> Mat3 {
    let forward = pose.direction().normalize();
    let lateral = pose.view_up().cross(forward).normalize();
    Mat3::from_cols(lateral, forward.cross(lateral), forward)
}

fn apply_frame(pose: &mut Camera, frame: Mat3) {
    let forward = frame.z_axis;
    let horizontal = forward.x.hypot(forward.z);
    pose.yaw = if horizontal > 1e-6 {
        forward.x.atan2(forward.z)
    } else {
        // At either pole, choose the heading from body-left. This preserves
        // a finite view basis without projecting world-up onto a vertical ray.
        -frame.x_axis.z.atan2(frame.x_axis.x)
    };
    pose.pitch = forward.y.atan2(horizontal);
    let left = Vec3::new(pose.yaw.cos(), 0., -pose.yaw.sin());
    let up = forward.cross(left).normalize();
    pose.roll = forward
        .dot(up.cross(frame.y_axis))
        .atan2(up.dot(frame.y_axis));
}

/// Local collision volumes for an on-foot player around the stationary aircraft.
pub(crate) fn parked_solid(p: Vec3) -> bool {
    let x = p.x.abs();
    let y = p.y;
    let z = p.z;
    if (-2.1..1.1).contains(&z) && x < 0.66 && (-0.55..0.84).contains(&y) {
        return true;
    }
    if (1.1..2.62).contains(&z) && x < 0.60 && (-0.48..0.17).contains(&y) {
        return true;
    }
    if (-5.62..-2.1).contains(&z) {
        let radius = 0.09 + (z + 5.62) * 0.17;
        if x < radius && (y - 0.06).abs() < radius {
            return true;
        }
    }
    let wing_height = 0.85 + x * 0.044;
    if x < 5.55 && (z - 0.04).abs() < 0.66 && (y - wing_height).abs() < 0.11 {
        return true;
    }
    (x < 1.82 && (z + 4.54).abs() < 0.43 && (y - 0.20).abs() < 0.10)
        || (x < 0.09 && (-5.58..-4.48).contains(&z) && (0.20..1.52).contains(&y))
        || ((x - 0.94).abs() < 0.12 && (y + 1.07).hypot(z + 0.38) < 0.25)
        || (x < 0.10 && (y + 1.14).hypot(z - 1.63) < 0.18)
}

const WHEELS: [Vec3; 2] = [
    Vec3::new(-0.94, -1.32, -0.38),
    Vec3::new(0.94, -1.32, -0.38),
];
const NOSE_WHEEL: Vec3 = Vec3::new(0., -1.32, 1.63);
const BODY: [Vec3; 7] = [
    Vec3::ZERO,
    Vec3::new(0., 0., 2.61),
    Vec3::new(0., 0.06, 2.59),
    Vec3::new(0., -0.10, 2.53),
    Vec3::new(0., 0.14, -5.61),
    Vec3::new(-5.55, 1.12, 0.),
    Vec3::new(5.55, 1.12, 0.),
];
const CRUISE_SPEED: f32 = 56.;
const CRUISE_POWER: f32 = 0.56;
const STRUT_TRAVEL: f32 = 0.24;
const PILOT_EYE: Vec3 = Vec3::new(0.28, 0.41, -0.48);
const PILOT_VIEW_DOWN: f32 = 0.105;
#[derive(Clone, Copy, Debug)]
pub(crate) struct Instruments {
    pub speed_mps: f32,
    pub altitude_m: f64,
    pub climb_mps: f32,
    pub bank_degrees: f32,
    pub pitch_degrees: f32,
    pub heading_degrees: f32,
    pub stall_warning: bool,
    pub flaps_degrees: f32,
    pub pitch_trim: f32,
    pub angle_of_attack: f32,
    pub wind: Vec3,
    pub show_airflow: bool,
    pub grounded: bool,
    pub throttle: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct Presentation {
    pub pose: Camera,
    pub instruments: Instruments,
    pub controls: Vec3,
    pub door: f32,
    pub rudder: f32,
    pub turn_rate: f32,
    pub prop_angle: f32,
    pub engine_running: bool,
    pub wheel_turn: [f32; 3],
    pub gear_compression: [f32; 3],
    pub radio_khz: u32,
    pub home_relative_radians: f32,
    pub home_distance_km: f32,
    pub hold: bool,
    pub wing_flex: [f32; 2],
}

#[derive(Clone, Copy)]
struct FlightHold {
    yaw: f32,
    altitude: f64,
}

struct Contact {
    fraction: f64,
    wheel_on_top: bool,
}
fn swept_contact(
    world: &World,
    start: Camera,
    end: Camera,
    wheel_clearance: f32,
) -> Option<Contact> {
    let before = axes(start);
    let after = axes(end);
    let mut contact: Option<Contact> = None;
    for (wheel, probe) in BODY.map(|p| (false, p)).into_iter().chain(
        WHEELS
            .into_iter()
            .chain([NOSE_WHEEL])
            .map(|p| (true, p + Vec3::Y * wheel_clearance)),
    ) {
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
            let tread_y = origin.y + travel.y * (hit.distance / length);
            let step = (hit.cell.y + 1) as f64 * voxel_size() - tread_y;
            let wheel_on_top = wheel
                && (hit.normal == [0, 1, 0]
                    || (hit.normal[1] == 0 && (0.0..=0.14).contains(&step)));
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
    let ground_at = |wheel: Vec3| {
        let origin = pose.position + (frame * wheel).as_dvec3() + DVec3::Y * 0.48;
        let hit = frontier_world::raycast_solid(world, origin.to_array(), [0., -1., 0.], 1.)?;
        if hit.normal != [0, 1, 0] {
            return None;
        }
        let above = frontier_world::Cell {
            y: hit.cell.y.checked_add(1)?,
            ..hit.cell
        };
        (world.material(above) != frontier_world::Material::Water)
            .then_some(above.y as f64 * voxel_size())
    };
    let left = ground_at(WHEELS[0])?;
    let right = ground_at(WHEELS[1])?;
    if (left - right).abs() > 0.22 {
        return None;
    }
    let mains = left.max(right);
    let highest = ground_at(NOSE_WHEEL).unwrap_or(mains).max(mains);
    (highest - mains <= 0.27).then_some(highest + 1.28)
}

#[derive(Default, Clone, Copy)]
pub(crate) struct FlightInput {
    pub roll: f32,
    pub pitch: f32,
    pub rudder: f32,
    pub throttle_rate: f32,
    pub brake: bool,
    /// Digital keys request travel toward a stop at a limited rate.
    pub keyboard: bool,
}

fn slew_control(current: f32, target: f32, travel_seconds: f32, return_seconds: f32) -> f32 {
    // Reversing direction first brings the surface back through neutral.
    let target = if current * target < 0. { 0. } else { target };
    let seconds = if target == 0. {
        return_seconds
    } else {
        travel_seconds
    };
    let max_change = STEP / seconds;
    current + (target - current).clamp(-max_change, max_change)
}

pub(crate) struct Airflow<'a> {
    pub relative_air: Vec3,
    pub angular_velocity: Vec3,
    pub center_of_gravity: Vec3,
    pub propwash: f32,
    pub sections: &'a [SectionFlow],
}

pub(crate) fn engine_speed_rad_s(throttle: f32) -> f32 {
    72. + 210. * throttle
}

pub(crate) struct Aircraft {
    dynamics: Dynamics,
    time_pending: f32,
    throttle: f32,
    rudder: f32,
    brake_pressure: f32,
    prop_angle: f32,
    engine_running: bool,
    wheel_turn: [f32; 3],
    gear_compression: [f32; 3],
    turn_rate: f32,
    grounded: bool,
    flap_detent: u8,
    flaps: f32,
    pitch_trim: f32,
    wind: Vec3,
    sampled_wind: Vec3,
    pub(super) feedback: Feedback,
    pub(super) damage: Option<aircraft_damage::Damage>,
    pub(crate) radar: frontier_runtime::weather::radar::Controls,
    show_airflow: bool,
    // In chase view the viewer camera follows this physical aircraft pose.
    pilot: Option<Camera>,
    controls: Vec3,
    door: f32,
    door_open: bool,
    look: [f32; 2],
    looking: bool,
    home: DVec3,
    radio_khz: u32,
    hold: Option<FlightHold>,
}

impl Default for Aircraft {
    fn default() -> Self {
        Self {
            dynamics: Dynamics::default(),
            time_pending: 0.,
            throttle: CRUISE_POWER,
            rudder: 0.,
            brake_pressure: 0.,
            prop_angle: 0.,
            engine_running: true,
            wheel_turn: [0.; 3],
            gear_compression: [0.; 3],
            turn_rate: 0.,
            grounded: false,
            flap_detent: 0,
            flaps: 0.,
            pitch_trim: 0.,
            wind: Vec3::ZERO,
            sampled_wind: Vec3::ZERO,
            feedback: Feedback::default(),
            damage: None,
            radar: Default::default(),
            show_airflow: false,
            pilot: None,
            controls: Vec3::ZERO,
            door: 0.,
            door_open: false,
            look: [0.; 2],
            looking: false,
            home: DVec3::ZERO,
            radio_khz: 118_000,
            hold: None,
        }
    }
}

#[derive(Default)]
struct GearContact {
    supported: bool,
    compression: [f32; 3],
    wheel_rates: [f32; 3],
    forces: [Vec3; 3],
}

/// Grade a nearby dry corridor. Relief is scored over the entire strip and
/// river bounds screen out narrow water features between terrain samples.
pub(crate) fn find_dirt_runway(
    terrain: &frontier_world::FbmTerrain,
    near: [f64; 2],
) -> Option<frontier_world::DirtRunway> {
    use frontier_world::{voxel_size, Cell, DirtRunway};
    let sample = |center: [i64; 2], along_x: bool| {
        let footprint = DirtRunway::new(center, 0, along_x);
        let position = footprint.center_m();
        let extents = footprint.half_extents_m();
        let min = std::array::from_fn(|i| position[i] - extents[i]);
        let max = std::array::from_fn(|i| position[i] + extents[i]);
        if terrain.seeded_water_feature_spacing(min, max).is_some() {
            return None;
        }
        let mut heights = Vec::with_capacity(81);
        for station in -13..=13 {
            for offset in [-14., 0., 14.] {
                let along = f64::from(station) * 20.;
                let [dx, dz] = if along_x {
                    [along, offset]
                } else {
                    [offset, along]
                };
                let cell = Cell::from_meters([position[0] + dx, 0., position[1] + dz]);
                let column = terrain.column_cells(cell.x, cell.z);
                if column.visible_top != column.ground_top {
                    return None;
                }
                heights.push(column.ground_top);
            }
        }
        heights.sort_unstable();
        let top = heights[heights.len() / 2];
        let spread = (*heights.last().unwrap() - heights[0]) as f64 * voxel_size();
        let distance = (position[0] - near[0]).hypot(position[1] - near[1]);
        Some((
            spread + distance * 0.009,
            DirtRunway::new(center, top, along_x),
        ))
    };

    let mut best: Option<(f64, DirtRunway)> = None;
    for radius in [0., 240., 480., 960., 1920., 3840., 7680., 15360.] {
        if best.is_some_and(|(cost, _)| radius * 0.009 > cost + 1.) {
            break;
        }
        let bearings = if radius == 0. { 1 } else { 12 };
        for bearing in 0..bearings {
            let angle = f64::from(bearing) * std::f64::consts::TAU / f64::from(bearings);
            let cell = Cell::from_meters([
                near[0] + radius * angle.cos(),
                0.,
                near[1] + radius * angle.sin(),
            ]);
            for along_x in [false, true] {
                if let Some(candidate) = sample([cell.x, cell.z], along_x) {
                    if best.is_none_or(|(cost, _)| candidate.0 < cost) {
                        best = Some(candidate);
                    }
                }
            }
        }
    }
    best.map(|(_, runway)| runway)
}

impl Aircraft {
    pub(super) fn enable_damage(&mut self) {
        let _ = aircraft_damage::asset();
        self.damage.get_or_insert_with(Default::default);
    }

    pub(super) fn attached(&self) -> u64 {
        self.dynamics.attached
    }

    pub(super) fn wreck(&mut self, camera: Camera) {
        let pose = self.presentation(camera);
        if let Some(damage) = &mut self.damage {
            aircraft_damage::separate(
                damage.active,
                damage,
                pose,
                self.dynamics.loading.center,
                self.dynamics.loading.mass,
                self.dynamics.velocity,
                self.dynamics.angular_velocity,
            );
        }
    }

    fn solve_structure(
        &mut self,
        camera: Camera,
        angular_accel: Vec3,
        load: Vec3,
        gear: &GearContact,
        impact: Option<destruction::Impact>,
    ) {
        use aircraft_damage::{asset, live, section_node, GEAR_FIRST};
        use frontier_runtime::structure::Load;
        let pose = self.presentation(camera);
        let Some(damage) = &mut self.damage else {
            return;
        };
        let asset = asset();
        let axes = axes(camera);
        let cg = self.dynamics.loading.center;
        let omega = self.dynamics.angular_velocity;
        let mass = self.dynamics.loading.mass;
        let mut loads = [Load::default(); aircraft_damage::COUNT];
        for (i, node) in asset
            .structure
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(i, _)| live(damage.active, *i))
        {
            let arm = node.center - cg;
            let inertia = asset.inertia[i];
            loads[i].force =
                -(load + angular_accel.cross(arm) + omega.cross(omega.cross(arm))) * node.mass;
            loads[i].moment = -(inertia * angular_accel + omega.cross(inertia * omega));
        }
        for (i, sample) in self.dynamics.flow_samples.iter().enumerate() {
            let group = section_node(i);
            loads[group].apply(
                sample.lift + sample.drag,
                sample.origin - asset.structure[group].center,
            );
            loads[group].moment += sample.moment;
        }
        for (i, p) in WHEELS.into_iter().chain([NOSE_WHEEL]).enumerate() {
            let group = GEAR_FIRST + i;
            loads[group].apply(gear.forces[i], p - asset.structure[group].center);
        }
        if let Some(hit) = impact {
            let group = hit.contact.node;
            let world_force = hit.impulse / STEP;
            let local = axes.transpose() * (hit.contact.point - camera.position).as_vec3();
            loads[group].apply(
                axes.transpose() * world_force,
                local - asset.structure[group].center,
            );
            if hit.energy > 20. {
                damage.impacts.push(hit);
            }
        }
        let mut components = Vec::new();
        let changed = damage.structure.solve(
            &asset.structure,
            damage.active,
            &loads,
            STEP,
            &mut components,
        );
        let target = aircraft_damage::wing_flex(&damage.structure, damage.active);
        damage.flex = std::array::from_fn(|i| damage.flex[i] + (target[i] - damage.flex[i]) * 0.18);
        damage.peak_seen = damage.peak_seen.max(damage.structure.peak_utilization);
        if !changed {
            return;
        }
        let pose = Presentation {
            wing_flex: damage.flex,
            ..pose
        };
        for &mask in &components[1..] {
            aircraft_damage::separate(mask, damage, pose, cg, mass, self.dynamics.velocity, omega);
        }
        let removed = damage.active & !components[0];
        damage.active = components[0];
        self.dynamics.attached = damage.active;
        self.dynamics.loading.remove(removed);
        self.dynamics.velocity += axes * omega.cross(self.dynamics.loading.center - cg);
        self.hold = None;
    }

    fn collision(
        &mut self,
        world: &World,
        from: Camera,
        to: Camera,
    ) -> Option<destruction::Contact> {
        use aircraft_damage::{asset, live, GEAR_FIRST};
        let pose = |p: Camera| destruction::Pose {
            center: p.position,
            orientation: Quat::from_mat3(&axes(p)),
        };
        if self
            .damage
            .as_mut()
            .unwrap()
            .clearance
            .sky(world, from.position, to.position, 6.3)
        {
            return None;
        }
        let shape = asset();
        let flex = self.damage.as_ref().unwrap().flex;
        let hull = shape
            .probes
            .iter()
            .enumerate()
            .filter(|(i, _)| live(self.attached(), *i) && !(GEAR_FIRST..GEAR_FIRST + 3).contains(i))
            .flat_map(|(i, points)| {
                points.iter().map(move |p| {
                    let local = if (1..17).contains(&i) {
                        aircraft_damage::flex_point(*p, p.x, flex)
                    } else {
                        *p
                    };
                    (i, local)
                })
            });
        let wheels = WHEELS
            .into_iter()
            .chain([NOSE_WHEEL])
            .enumerate()
            .filter(|(i, _)| live(self.attached(), GEAR_FIRST + i))
            .map(|(i, point)| (GEAR_FIRST + i, point + Vec3::Y * STRUT_TRAVEL));
        destruction::sweep(world, pose(from), pose(to), hull.chain(wheels))
    }

    pub(crate) fn enter(world: &World, camera: &mut Camera) -> Self {
        let mut plane = Self::default();
        if let Some(y) = runway_height(world, *camera) {
            let level = Camera {
                position: DVec3::new(camera.position.x, y, camera.position.z),
                pitch: 0.,
                roll: 0.,
                ..*camera
            };
            if swept_contact(world, *camera, level, STRUT_TRAVEL).is_none() {
                *camera = level;
                plane.throttle = 0.;
                plane.grounded = true;
            }
        }
        if !plane.grounded {
            // Begin a cruise spawn with a small positive incidence on the wing
            // and a velocity along the requested view direction.
            plane.dynamics.velocity = camera.direction() * CRUISE_SPEED;
            plane.dynamics.airspeed = CRUISE_SPEED;
            plane.dynamics.equivalent_airspeed = CRUISE_SPEED
                * (dynamics::density(camera.position.y) / dynamics::SEA_LEVEL_DENSITY).sqrt();
            camera.pitch = (camera.pitch + 0.025).clamp(-1.57, 1.57);
        }
        plane.home = camera.position;
        plane
    }
    pub(crate) fn park_on_runway(
        world: &World,
        camera: &mut Camera,
        runway: frontier_world::DirtRunway,
    ) -> Option<Self> {
        use frontier_world::DirtRunway;
        let [x, z] = runway.center_m();
        let center = DVec3::new(x, runway.height_m(), z);
        let axis = if runway.along_x() { DVec3::X } else { DVec3::Z };
        let departure = |sign: f64| {
            [315., 380., 480., 620.]
                .into_iter()
                .map(|distance| {
                    let p = center + axis * (distance * sign);
                    (world.ground_height_m(p.x, p.z) - runway.height_m()).max(0.)
                })
                .sum::<f64>()
        };
        let forward = axis
            * if departure(1.) <= departure(-1.) {
                1.
            } else {
                -1.
            };
        camera.position = center - forward * (DirtRunway::HALF_LENGTH_M - 22.) + DVec3::Y * 1.65;
        camera.yaw = forward.x.atan2(forward.z) as f32;
        camera.pitch = 0.;
        camera.roll = 0.;
        let plane = Self::enter(world, camera);
        plane.on_ground().then_some(plane)
    }

    pub(crate) fn park(world: &World, camera: &mut Camera) -> Option<Self> {
        let origin = *camera;
        // Select a dry, shallow departure corridor in the actual world. A dry
        // parking cell by itself can point straight into a river or a hillside.
        for (stations, search_radius) in [(180, 640.), (144, 640.), (144, 20480.)] {
            for radius in [
                0., 24., 64., 144., 320., 640., 1280., 2560., 5120., 10240., 20480.,
            ] {
                if radius > search_radius {
                    break;
                }
                let bearings = if radius == 0. {
                    1
                } else if radius <= 640. {
                    8
                } else {
                    32
                };
                for angle in 0..bearings {
                    let a = angle as f64 * std::f64::consts::TAU / f64::from(bearings);
                    let mut trial = origin;
                    trial.position.x += radius * a.cos();
                    trial.position.z += radius * a.sin();
                    trial.position.y = world.height_m(trial.position.x, trial.position.z) + 1.65;
                    trial.pitch = 0.;
                    trial.roll = 0.;
                    for heading in 0..12 {
                        trial.yaw = origin.yaw + (heading as f32) * std::f32::consts::TAU / 12.;
                        let frame = axes(trial);
                        let mut previous = trial.position.y - 1.65;
                        let usable = (0..=stations).all(|i| {
                            let center =
                                trial.position + frame.z_axis.as_dvec3() * f64::from(i * 2);
                            let mut highest = f64::NEG_INFINITY;
                            let mut lowest = f64::INFINITY;
                            for offset in [-1., 0., 1.] {
                                let p = center + frame.x_axis.as_dvec3() * offset;
                                let c = frontier_world::Cell::from_meters(p.to_array());
                                let column = world.column(c.x, c.z);
                                if column.height_cells() != column.ground_height_cells() {
                                    return false;
                                }
                                highest = highest.max(column.height_m());
                                lowest = lowest.min(column.height_m());
                            }
                            let suitable =
                                highest - lowest <= 0.14 && (highest - previous).abs() <= 0.20;
                            previous = highest;
                            suitable
                        });
                        if !usable {
                            continue;
                        }
                        let mut pose = trial;
                        let plane = Self::enter(world, &mut pose);
                        if plane.on_ground()
                            && BODY.iter().all(|p| {
                                let point = pose.position + (frame * *p).as_dvec3();
                                frontier_world::voxel_grid()
                                    .checked_cell(point.to_array())
                                    .is_some_and(|c| !world.material(c).is_blocking())
                            })
                        {
                            *camera = pose;
                            return Some(plane);
                        }
                    }
                }
            }
        }
        None
    }
    pub(crate) fn on_ground(&self) -> bool {
        self.grounded
    }
    pub(crate) fn airspeed_mps(&self) -> f32 {
        self.dynamics.airspeed
    }
    pub(crate) fn exterior_pose(&self) -> Option<Camera> {
        self.pilot
    }
    pub(crate) fn toggle_doors(&mut self) {
        if self.grounded && self.dynamics.velocity.length() < 2. {
            self.door_open = !self.door_open;
        }
    }
    pub(crate) fn tune_radio(&mut self, direction: i32) {
        let channel = (self.radio_khz as i32 - 118_000) / 25;
        self.radio_khz = (118_000 + 25 * (channel + direction).rem_euclid(760)) as u32;
    }
    pub(crate) fn toggle_hold(&mut self, camera: Camera) {
        self.hold = if self.hold.is_none() && !self.grounded && self.airspeed_mps() > 34. {
            let pose = self.pilot.unwrap_or(camera);
            Some(FlightHold {
                yaw: pose.yaw,
                altitude: pose.position.y,
            })
        } else {
            None
        };
    }
    pub(crate) fn set_looking(&mut self, looking: bool) {
        self.looking = looking;
        if !self.looking {
            self.look = [0.; 2];
        }
    }
    pub(crate) fn look(&mut self, delta: (f64, f64)) {
        if self.looking {
            self.look[0] = (self.look[0] - delta.0 as f32 * 0.003).clamp(-2.45, 2.45);
            self.look[1] = (self.look[1] + delta.1 as f32 * 0.003).clamp(-0.75, 0.95);
        }
    }
    pub(crate) fn display_camera(&self, mut pose: Camera) -> Camera {
        if self.pilot.is_some() {
            return pose;
        }
        let body = axes(pose);
        pose.position += (body * (PILOT_EYE + self.feedback.eye())).as_dvec3();
        let head = self.feedback.frame(body)
            * Mat3::from_rotation_y(self.look[0])
            * Mat3::from_rotation_x(PILOT_VIEW_DOWN + self.look[1]);
        apply_frame(&mut pose, head);
        pose
    }
    pub(crate) fn presentation(&self, camera: Camera) -> Presentation {
        let pose = self.pilot.unwrap_or(camera);
        let offset = self.home - pose.position;
        Presentation {
            pose,
            instruments: self.instruments(pose),
            controls: self.controls,
            door: self.door,
            rudder: self.rudder,
            turn_rate: self.turn_rate,
            prop_angle: self.prop_angle,
            engine_running: self.engine_running,
            wheel_turn: self.wheel_turn,
            gear_compression: self.gear_compression,
            radio_khz: self.radio_khz,
            home_relative_radians: -(offset.x.atan2(offset.z) as f32 - pose.yaw),
            home_distance_km: offset.x.hypot(offset.z) as f32 / 1_000.,
            hold: self.hold.is_some(),
            wing_flex: self.damage.as_ref().map_or([0.; 2], |damage| damage.flex),
        }
    }
    pub(crate) fn instruments(&self, pose: Camera) -> Instruments {
        Instruments {
            speed_mps: self.dynamics.equivalent_airspeed,
            altitude_m: pose.position.y,
            climb_mps: self.dynamics.velocity.y,
            bank_degrees: pose.roll.to_degrees(),
            pitch_degrees: pose.pitch.to_degrees(),
            heading_degrees: (180. - pose.yaw.to_degrees()).rem_euclid(360.),
            stall_warning: !self.grounded && self.dynamics.stall_warning,
            flaps_degrees: self.flaps * airframe::FLAP_TRAVEL.to_degrees(),
            pitch_trim: self.pitch_trim,
            angle_of_attack: self.dynamics.alpha.to_degrees(),
            wind: self.sampled_wind,
            show_airflow: self.show_airflow,
            grounded: self.grounded,
            throttle: self.throttle,
        }
    }
    /// Find a standing position clear of canonical voxels beside either cabin door.
    pub(crate) fn step_out(&mut self, world: &World, camera: Camera) -> Option<(Camera, Camera)> {
        if !self.grounded || self.dynamics.velocity.length() > 0.35 {
            return None;
        }
        let pose = self.pilot.unwrap_or(camera);
        let basis = axes(pose);
        for side in [1., -1.] {
            let doorstep = pose.position + (basis * Vec3::new(2.05 * side, 0., -0.38)).as_dvec3();
            let hit = frontier_world::raycast_solid(
                world,
                (doorstep + DVec3::Y * 0.65).to_array(),
                [0., -1., 0.],
                3.2,
            );
            let Some(hit) = hit.filter(|h| h.normal == [0, 1, 0]) else {
                continue;
            };
            let floor = (hit.cell.y + 1) as f64 * voxel_size();
            let eye = DVec3::new(
                doorstep.x,
                floor + crate::walking::EYE_HEIGHT + 0.001,
                doorstep.z,
            );
            if crate::walking::aabb_blocked(eye.to_array(), |cell| world.blocking_material(cell))
                || !Self::can_board(world, pose, eye)
            {
                continue;
            }
            let toward = pose.position - eye;
            self.pilot = None;
            self.set_looking(false);
            self.dynamics.velocity = Vec3::ZERO;
            self.dynamics.angular_velocity = Vec3::ZERO;
            self.throttle = 0.;
            self.controls = Vec3::ZERO;
            self.rudder = 0.;
            self.brake_pressure = 0.;
            self.engine_running = false;
            self.gear_compression = [0.; 3];
            self.door_open = true;
            self.door = 1.;
            return Some((
                pose,
                Camera {
                    position: eye,
                    yaw: toward.x.atan2(toward.z) as f32,
                    pitch: -0.10,
                    roll: 0.,
                    ..pose
                },
            ));
        }
        None
    }

    pub(crate) fn can_board(world: &World, pose: Camera, eye: DVec3) -> bool {
        [-1., 1.].into_iter().any(|side| {
            let door =
                pose.position + (axes(pose) * Vec3::new(side * 0.71, 0.34, -0.35)).as_dvec3();
            let path = door - eye;
            path.length() <= 2.25
                && (path.length() < 0.01
                    || frontier_world::raycast_solid(
                        world,
                        eye.to_array(),
                        path.to_array(),
                        path.length(),
                    )
                    .is_none())
        })
    }
    pub(crate) fn toggle_view(&mut self, camera: &mut Camera, world: &World) {
        self.set_looking(false);
        if let Some(pilot) = self.pilot.take() {
            *camera = Camera {
                far: camera.far,
                fov_y: camera.fov_y,
                ..pilot
            };
        } else {
            self.follow(camera, world);
        }
    }
    pub(super) fn toggle_camera_motion(&mut self, camera: &mut Camera, world: &World) {
        self.feedback.camera_motion = !self.feedback.camera_motion;
        if let Some(pose) = self.pilot {
            *camera = Camera {
                far: camera.far,
                fov_y: camera.fov_y,
                ..pose
            };
            self.follow(camera, world);
        }
    }
    pub(super) fn follow(&mut self, camera: &mut Camera, world: &World) {
        self.pilot = Some(*camera);
        let forward = camera.direction().as_dvec3();
        let view = self.feedback.frame(axes(*camera));
        let (offset, anchor) = if self.looking {
            let bearing = camera.yaw + self.look[0];
            let back = DVec3::new(f64::from(bearing.sin()), 0., f64::from(bearing.cos()));
            (
                -back * 9.5 + DVec3::Y * f64::from((2.6 + 5. * self.look[1]).clamp(0.7, 7.)),
                0.55,
            )
        } else {
            ((-view.z_axis * 15. + view.y_axis * 4.2).as_dvec3(), 6.)
        };
        let direction = offset.normalize();
        let distance = frontier_world::raycast_solid(
            world,
            camera.position.to_array(),
            direction.to_array(),
            offset.length(),
        )
        .map_or(offset.length(), |h| (h.distance - 0.20).max(0.));
        let target = camera.position + forward * anchor;
        camera.position += direction * distance;
        let look = (target - camera.position).normalize();
        if self.looking {
            camera.yaw = look.x.atan2(look.z) as f32;
            camera.pitch = look.y.asin() as f32;
            camera.roll = 0.;
        } else {
            let forward = look.as_vec3().normalize();
            let left = view.y_axis.cross(forward).normalize();
            apply_frame(camera, Mat3::from_cols(left, forward.cross(left), forward));
        }
    }

    /// Return false when the swept flight path meets a blocking native cell.
    pub(crate) fn adjust_pitch_trim(&mut self, direction: f32) {
        self.pitch_trim = (self.pitch_trim + 0.025 * direction).clamp(-0.25, 0.25);
    }

    pub(crate) fn aerodynamic_state(&self) -> ([f32; 2], [f32; 3]) {
        (
            self.dynamics.wing_separation,
            self.dynamics.angular_velocity.to_array(),
        )
    }

    pub(crate) fn airflow(&self) -> Option<Airflow<'_>> {
        self.show_airflow.then_some(Airflow {
            relative_air: self.dynamics.relative_air,
            angular_velocity: self.dynamics.angular_velocity,
            center_of_gravity: self.dynamics.loading.center,
            propwash: self.dynamics.propwash,
            sections: &self.dynamics.flow_samples,
        })
    }

    pub(crate) fn toggle_airflow(&mut self) {
        self.show_airflow = !self.show_airflow;
    }

    pub(crate) fn ground_speed_mps(&self) -> f32 {
        self.dynamics.velocity.x.hypot(self.dynamics.velocity.z)
    }

    pub(super) fn brake_pressure(&self) -> f32 {
        self.brake_pressure
    }

    pub(crate) fn set_wind(&mut self, wind: Vec3) {
        self.wind = wind;
        self.sampled_wind = wind;
    }

    pub(crate) fn select_flap(&mut self, direction: i8) {
        self.flap_detent = (self.flap_detent as i8 + direction).clamp(0, 3) as u8;
    }

    pub(crate) fn advance(
        &mut self,
        camera: &mut Camera,
        world: &World,
        input: FlightInput,
        dt: f32,
    ) -> bool {
        let wind = self.wind;
        self.advance_in_weather(camera, world, input, dt, |_| wind, 1.)
    }

    pub(crate) fn advance_in_weather(
        &mut self,
        camera: &mut Camera,
        world: &World,
        input: FlightInput,
        dt: f32,
        wind: impl Fn(DVec3) -> Vec3,
        density_ratio: f32,
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
        self.engine_running = aircraft_damage::live(self.attached(), aircraft_damage::NOSE);
        self.time_pending += dt;
        let start = *camera;
        let mut elapsed = 0.;
        self.sampled_wind = wind(camera.position);
        while self.time_pending >= STEP {
            self.time_pending -= STEP;
            elapsed += STEP;
            if !self.step_in_air(camera, world, input, &wind, density_ratio) {
                return false;
            }
        }
        if elapsed > 0. {
            let turn = start.yaw - camera.yaw;
            self.turn_rate = turn.sin().atan2(turn.cos()) / elapsed;
        }
        if chase {
            self.follow(camera, world);
        }
        true
    }

    #[cfg(test)]
    fn step(
        &mut self,
        camera: &mut Camera,
        world: &World,
        input: FlightInput,
        wind: impl Fn(DVec3) -> Vec3,
    ) -> bool {
        self.step_in_air(camera, world, input, wind, 1.)
    }

    fn step_in_air(
        &mut self,
        camera: &mut Camera,
        world: &World,
        input: FlightInput,
        wind: impl Fn(DVec3) -> Vec3,
        density_ratio: f32,
    ) -> bool {
        let bank = input.roll;
        let pitch = input.pitch;
        self.throttle = (self.throttle + input.throttle_rate * STEP * 0.55).clamp(0., 1.);
        let flap_change = self.flap_detent as f32 / 3. - self.flaps;
        self.flaps += flap_change.clamp(-0.31 * STEP, 0.31 * STEP);
        let brake = f32::from(input.brake);
        if input.keyboard {
            self.rudder = slew_control(self.rudder, input.rudder, 0.9, 0.25);
            self.brake_pressure = slew_control(self.brake_pressure, brake, 0.4, 0.16);
        } else {
            self.rudder += (input.rudder - self.rudder) * (1. - (-10. * STEP).exp());
            self.brake_pressure = brake;
        }
        self.prop_angle = (self.prop_angle + STEP * engine_speed_rad_s(self.throttle))
            .rem_euclid(std::f32::consts::TAU);

        if self.grounded
            || self.airspeed_mps() < 32.
            || bank.abs().max(pitch.abs()).max(input.rudder.abs()) > 0.1
        {
            self.hold = None;
        }
        let (bank, pitch) = if let Some(hold) = self.hold {
            let heading = hold.yaw - camera.yaw;
            let target_bank = (-1.1 * heading.sin().atan2(heading.cos())).clamp(-0.40, 0.40);
            let omega = self.dynamics.angular_velocity;
            let bank = (2.2 * (target_bank - camera.roll) - 0.7 * omega.z).clamp(-1., 1.);
            let altitude_error = (hold.altitude - camera.position.y) as f32;
            let target_climb = (0.26 * altitude_error).clamp(-5., 5.);
            let pitch = (0.08 + 0.10 * (target_climb - self.dynamics.velocity.y) + 0.7 * omega.x)
                .clamp(-0.7, 0.7);
            (bank, pitch)
        } else {
            (bank, pitch)
        };
        let follow = 1. - (-12. * STEP).exp();
        if input.keyboard && self.hold.is_none() {
            self.controls.x = slew_control(self.controls.x, bank, 0.82, 0.22);
            self.controls.y = slew_control(self.controls.y, pitch, 1.0, 0.25);
        } else {
            self.controls.x += (bank - self.controls.x) * follow;
            self.controls.y += (pitch - self.controls.y) * follow;
        }
        self.controls.z += (self.throttle - self.controls.z) * follow;
        if !self.grounded || self.dynamics.velocity.length() >= 2. {
            self.door_open = false;
        }
        let target = if self.door_open { 1. } else { 0. };
        self.door += (target - self.door).clamp(-1.8 * STEP, 1.8 * STEP);

        let start = *camera;
        let body = axes(start);
        let deflection = Deflection {
            roll: self.controls.x,
            pitch: (self.controls.y + self.pitch_trim).clamp(-1., 1.),
            rudder: self.rudder,
            flaps: self.flaps,
            throttle: self.throttle,
        };
        let mut loads =
            self.dynamics
                .loads_in_air(start.position, body, deflection, wind, STEP, density_ratio);
        let gear = self.gear_loads(world, start, body, &mut loads);
        self.grounded = gear.supported;
        self.gear_compression = gear.compression;
        let old_velocity = self.dynamics.velocity;
        let old_omega = self.dynamics.angular_velocity;
        let frame = self.dynamics.integrate(body, loads, STEP);
        apply_frame(camera, frame);
        let center = self.dynamics.loading.center;
        let travel = ((old_velocity + self.dynamics.velocity) * (0.5 * STEP)
            + (body - frame) * center)
            .as_dvec3();
        camera.position += travel;
        let mut impact = None;
        let mut crash = false;
        if self.damage.is_some() {
            if let Some(hit) = self.collision(world, start, *camera) {
                let q = Quat::from_mat3(&body).slerp(Quat::from_mat3(&frame), hit.fraction as f32);
                let at_hit = Mat3::from_quat(q);
                camera.position =
                    start.position + travel * hit.fraction + hit.normal.as_dvec3() * 0.004;
                apply_frame(camera, at_hit);
                let mut angular = at_hit * self.dynamics.angular_velocity;
                let mass = self.dynamics.loading.mass;
                let impulse = destruction::collide(
                    mass,
                    self.dynamics.loading.inverse_inertia(),
                    q,
                    camera.position + (at_hit * center).as_dvec3(),
                    &mut self.dynamics.velocity,
                    &mut angular,
                    hit,
                );
                self.dynamics.angular_velocity = at_hit.transpose() * angular;
                let cabin = hit.node == 0 || hit.node == aircraft_damage::NOSE;
                crash = cabin && impulse.energy > mass * 10.;
                impact = Some(impulse);
            }
            let specific_acceleration =
                (self.dynamics.velocity - old_velocity) / STEP + Vec3::Y * dynamics::GRAVITY;
            self.solve_structure(
                *camera,
                (self.dynamics.angular_velocity - old_omega) / STEP,
                axes(*camera).transpose() * specific_acceleration,
                &gear,
                impact,
            );
            if crash {
                return false;
            }
        } else if let Some(contact) = swept_contact(world, start, *camera, STRUT_TRAVEL) {
            *camera = start;
            camera.position += travel * contact.fraction;
            if self.grounded && !contact.wheel_on_top && self.dynamics.velocity.length() < 13. {
                self.dynamics.velocity = Vec3::ZERO;
                self.dynamics.angular_velocity = Vec3::ZERO;
            } else {
                self.grounded = false;
                return false;
            }
        }
        for (phase, spin) in self.wheel_turn.iter_mut().zip(gear.wheel_rates) {
            *phase = (*phase + spin * STEP).rem_euclid(std::f32::consts::TAU);
        }
        let omega = self.dynamics.angular_velocity;
        let seat_arm = PILOT_EYE - center;
        let seat_acceleration = loads.force / self.dynamics.loading.mass
            + ((omega - old_omega) / STEP).cross(seat_arm)
            + omega.cross(omega.cross(seat_arm));
        self.feedback.advance(
            axes(*camera),
            seat_acceleration / dynamics::GRAVITY,
            omega,
            !self.grounded,
            STEP,
        );
        true
    }

    fn gear_loads(
        &self,
        world: &World,
        pose: Camera,
        body: Mat3,
        loads: &mut Forces,
    ) -> GearContact {
        let to_body = body.transpose();
        let mut contact = GearContact::default();
        for (i, wheel) in WHEELS.into_iter().chain([NOSE_WHEEL]).enumerate() {
            if !aircraft_damage::live(self.attached(), aircraft_damage::GEAR_FIRST + i) {
                continue;
            }
            let nose = i == 2;
            let radius: f32 = if nose { 0.18 } else { 0.25 };
            let steer = if nose { -self.rudder * 0.52 } else { 0. };
            let (sin, cos) = steer.sin_cos();
            let track = body * Vec3::new(sin, 0., cos);
            let forward = Vec3::new(track.x, 0., track.z).normalize_or_zero();
            let tire = pose.position + (body * wheel).as_dvec3();
            let spring_axis = body.y_axis;
            if spring_axis.y < 0.35 {
                continue;
            }
            let surface_at = |tread: DVec3| {
                let lift = f64::from(STRUT_TRAVEL) + 0.15;
                let above = tread + DVec3::Y * lift;
                let hit = frontier_world::raycast_solid(
                    world,
                    above.to_array(),
                    [0., -1., 0.],
                    lift + 0.045,
                )?;
                (hit.normal == [0, 1, 0]).then_some((lift - hit.distance) as f32)
            };
            let center = surface_at(tire);
            if center.is_none() && !self.grounded {
                continue;
            }
            // Sample the rounded tread ahead and behind the axle. A native
            // voxel lip begins loading a rolling tire before it reaches the axle.
            let mut depth = center.unwrap_or(f32::NEG_INFINITY);
            for offset in [-0.72_f32, 0.72] {
                let rise = radius * (1. - (1. - offset * offset).sqrt());
                let tread = tire + (forward * (radius * offset) + Vec3::Y * rise).as_dvec3();
                if let Some(d) = surface_at(tread) {
                    depth = depth.max(d);
                }
            }
            if depth <= 0. {
                continue;
            }
            let compression = depth / spring_axis.y;
            let r = wheel - self.dynamics.loading.center + Vec3::Y * compression;
            let velocity = self.dynamics.velocity + body * self.dynamics.angular_velocity.cross(r);
            let (spring, damper) = if nose {
                (51_000., 5_200.)
            } else {
                (85_000., 7_000.)
            };
            let normal = (spring * compression - damper * velocity.y).max(0.);
            if normal == 0. {
                continue;
            }
            let sideways = Vec3::Y.cross(forward);
            let rolling_speed = velocity.dot(forward);
            let side_speed = velocity.dot(sideways);
            let lateral = -sideways * (3_800. * side_speed);
            let rolling = -0.027 * normal * (rolling_speed / 0.45).clamp(-1., 1.);
            let free = (forward * rolling + lateral).clamp_length_max(normal * 0.58);
            let friction = if self.brake_pressure > 0. {
                let braking = if nose {
                    rolling
                } else {
                    -6_000. * rolling_speed
                };
                let braked = (forward * braking + lateral).clamp_length_max(normal * 0.67);
                free.lerp(braked, self.brake_pressure)
            } else {
                free
            };
            let force = to_body * (Vec3::Y * normal + friction);
            loads.apply(force, r);
            contact.forces[i] = force;
            contact.supported = true;
            // The airframe pose already follows the springs. Store only the
            // visual strut travel here, for the animation of the wheels.
            contact.compression[i] = compression.min(STRUT_TRAVEL);
            contact.wheel_rates[i] = rolling_speed / radius;
        }
        contact
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{Cell, FbmParams, Material};
    fn input(roll: f32, pitch: f32, power_change: f32) -> FlightInput {
        FlightInput {
            roll,
            pitch,
            throttle_rate: power_change,
            brake: power_change < 0.,
            ..Default::default()
        }
    }
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
    fn structural_load_detaches_voxel_component_which_lands_offscreen() {
        let (mut world, mut camera) = runway();
        camera.position.y += 30.;
        let mut plane = Aircraft::enter(&world, &mut camera);
        plane.enable_damage();
        let original_mass = plane.dynamics.loading.mass;
        for (i, section) in plane.dynamics.flow_samples.iter_mut().take(8).enumerate() {
            section.origin = aircraft_damage::asset().structure[i + 1].center;
            section.lift = Vec3::Y * 650.;
        }
        for _ in 0..120 {
            plane.solve_structure(
                camera,
                Vec3::ZERO,
                Vec3::Y * 9.81,
                &GearContact::default(),
                None,
            );
        }
        assert_eq!(plane.attached(), aircraft_damage::INTACT);
        for section in plane.dynamics.flow_samples.iter_mut().take(8) {
            section.lift *= 11.;
        }
        for _ in 0..45 {
            plane.solve_structure(
                camera,
                Vec3::ZERO,
                Vec3::Y * 38.,
                &GearContact::default(),
                None,
            );
            if plane.attached() != aircraft_damage::INTACT {
                break;
            }
        }
        assert_ne!(plane.attached(), aircraft_damage::INTACT);
        assert!(
            (9..17).all(|i| aircraft_damage::live(plane.attached(), i)),
            "the opposite wing stayed attached"
        );
        let damage = plane.damage.as_mut().unwrap();
        assert!(!damage.fragments.is_empty());
        let removed: f32 = damage.fragments.iter().map(|b| b.shape.mass).sum();
        assert!((plane.dynamics.loading.mass + removed - original_mass).abs() < 0.005);
        let mut scene = destruction::Scene::default();
        scene.receive(&mut world, damage);
        for _ in 0..1200 {
            scene.advance(&mut world, DVec3::new(5500., 600., 0.), 1. / 60., |_| {
                Vec3::ZERO
            });
        }
        assert!(
            scene
                .bodies
                .iter()
                .all(|b| b.pose.center.y < 4. && b.pose.center.y > -1.),
            "{}",
            scene.report()
        );
        assert!(
            scene.bodies.iter().all(|b| b.sleep >= 1.2),
            "{}",
            scene.report()
        );
        assert!(scene.removed_cells > 0, "{}", scene.report());
    }

    #[test]
    fn an_overloaded_wing_loses_lift_through_the_flight_entry_point() {
        let (world, mut camera) = runway();
        camera.position.y += 1200.;
        let mut plane = Aircraft::enter(&world, &mut camera);
        plane.enable_damage();
        plane.dynamics.velocity = camera.direction() * 100.;
        let wind = |point: DVec3| {
            // The gust crosses only one wing, so the same rigid flight state
            // drives different pressure loads on either side.
            Vec3::Y * (point.x as f32 / 0.8).clamp(0., 1.) * 35.
        };
        for _ in 0..120 {
            assert!(plane.advance_in_weather(
                &mut camera,
                &world,
                FlightInput::default(),
                STEP,
                wind,
                1.
            ));
            if plane.attached() != aircraft_damage::INTACT {
                break;
            }
        }
        assert_ne!(
            plane.attached(),
            aircraft_damage::INTACT,
            "peak {:.2}",
            plane.damage.as_ref().unwrap().peak_seen
        );
        assert!(!plane.damage.as_ref().unwrap().fragments.is_empty());
        assert!(plane.advance_in_weather(
            &mut camera,
            &world,
            FlightInput::default(),
            STEP,
            wind,
            1.
        ));
        let missing: Vec<_> = (0..8)
            .filter(|&i| !aircraft_damage::live(plane.attached(), 1 + i))
            .collect();
        assert!(!missing.is_empty());
        for i in missing {
            assert_eq!(plane.dynamics.flow_samples[i].lift, Vec3::ZERO);
        }
    }

    #[test]
    fn damaged_flight_crash_keeps_wreckage_and_edits_the_world() {
        let (mut world, mut camera) = runway();
        camera.position.y += 21.;
        camera.pitch = -0.74;
        let mut plane = Aircraft::enter(&world, &mut camera);
        plane.enable_damage();
        plane.dynamics.velocity = camera.direction() * 70.;
        let mut survived = true;
        for _ in 0..360 {
            if !plane.advance(&mut camera, &world, FlightInput::default(), STEP) {
                survived = false;
                break;
            }
        }
        assert!(!survived, "flight ended at {:?}", camera.position);
        plane.wreck(camera);
        let mut scene = destruction::Scene::default();
        scene.receive(&mut world, plane.damage.as_mut().unwrap());
        assert!(!scene.bodies.is_empty());
        assert!(scene.removed_cells > 0);
        assert!(world.revision() > 0);
    }

    #[test]
    fn keyboard_taps_hold_and_release_move_the_flight_controls_proportionally() {
        let (world, mut camera) = runway();
        camera.position.y += 1000.;
        let mut plane = Aircraft::enter(&world, &mut camera);
        let pressed = FlightInput {
            roll: -1.,
            pitch: 1.,
            rudder: -1.,
            brake: true,
            keyboard: true,
            ..Default::default()
        };
        for _ in 0..6 {
            assert!(plane.advance(&mut camera, &world, pressed, STEP));
        }
        let pose = plane.presentation(camera);
        assert!((0.045..0.055).contains(&pose.controls.y));
        assert!((-0.07..-0.05).contains(&pose.controls.x));
        assert!((-0.07..-0.05).contains(&pose.rudder));
        assert!((0.12..0.13).contains(&plane.brake_pressure()));

        for _ in 0..120 {
            assert!(plane.advance(&mut camera, &world, pressed, STEP));
        }
        let pose = plane.presentation(camera);
        assert_eq!(pose.controls.truncate().to_array(), [-1., 1.]);
        assert_eq!(pose.rudder, -1.);
        assert_eq!(plane.brake_pressure(), 1.);

        let released = FlightInput {
            keyboard: true,
            ..Default::default()
        };
        assert!(plane.advance(&mut camera, &world, released, STEP));
        assert!(plane.presentation(camera).controls.y > 0.9);
        for _ in 0..30 {
            assert!(plane.advance(&mut camera, &world, released, STEP));
        }
        let pose = plane.presentation(camera);
        assert_eq!(pose.controls.truncate().to_array(), [0., 0.]);
        assert_eq!(pose.rudder, 0.);
        assert_eq!(plane.brake_pressure(), 0.);
    }
    #[test]
    fn walkup_seed_boards_and_takes_off_from_natural_terrain() {
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(-128., 0., -128.);
        let world = World::fbm(
            1878846491374251281,
            FbmParams {
                recipe: frontier_world::FbmRecipe::SeededWorld,
                ..Default::default()
            },
        )
        .unwrap()
        .with_vegetation_enabled(false);
        camera.yaw =
            crate::terrain::seeded_world_heading(world.fbm_terrain().unwrap(), -128., -128.).0;
        let mut plane = Aircraft::park(&world, &mut camera).unwrap();
        let (pose, foot) = plane.step_out(&world, camera).unwrap();
        assert!(Aircraft::can_board(&world, pose, foot.position));
        assert!(!Aircraft::can_board(
            &world,
            pose,
            foot.position + DVec3::X * 8.
        ));
        camera = pose;
        let dt = 1. / 60.;
        let heading = camera.yaw;
        plane.select_flap(1);
        for _ in 0..1800 {
            let cross_track =
                (camera.position - pose.position).dot(axes(pose).x_axis.as_dvec3()) as f32;
            let turn = camera.yaw - heading;
            let heading_error = turn.sin().atan2(turn.cos()) + cross_track * 0.027;
            let omega = plane.dynamics.angular_velocity;
            let rudder = (heading_error * 2.4 + omega.y * 0.75).clamp(-0.55, 0.55);
            let pitch = if plane.airspeed_mps() >= 23. {
                (0.43 + (0.19 - camera.pitch) * 2. + omega.x * 0.5).clamp(-0.2, 0.55)
            } else {
                0.
            };
            let roll = if plane.grounded {
                0.
            } else {
                (-1.3 * camera.roll - omega.z).clamp(-0.4, 0.4)
            };
            let controls = FlightInput {
                rudder,
                ..input(roll, pitch, 1.)
            };
            assert!(
                plane.advance(&mut camera, &world, controls, dt),
                "collision at {:?}, speed {}, pitch {}",
                camera.position,
                plane.airspeed_mps(),
                camera.pitch
            );
            let agl = camera.position.y - world.height_m(camera.position.x, camera.position.z);
            if !plane.grounded && agl > 6. && plane.dynamics.velocity.y > 1. {
                assert!(plane.step_out(&world, camera).is_none());
                assert!(camera.position.distance(pose.position) > 60.);
                return;
            }
        }
        panic!(
            "no sustained climb at {:?}, v {:?}, attitude {:?}, supported {}",
            camera.position,
            plane.dynamics.velocity,
            axes(camera).z_axis,
            plane.grounded
        );
    }

    #[test]
    fn rudder_steers_independently_of_bank_and_boarding_checks_walls() {
        let (mut world, camera) = runway();
        let mut pose = Camera {
            position: camera.position + DVec3::Y * 120.,
            ..camera
        };
        let mut plane = Aircraft::enter(&world, &mut pose);
        let rudder = FlightInput {
            rudder: 1.,
            ..Default::default()
        };
        for _ in 0..30 {
            assert!(plane.advance(&mut pose, &world, rudder, 1. / 60.));
        }
        assert!(pose.yaw < -0.018);
        let mut pose = camera;
        let mut plane = Aircraft::enter(&world, &mut pose);
        let (_, walk) = plane.step_out(&world, pose).unwrap();
        let wall = Cell::from_meters((pose.position + DVec3::new(1.35, 0.35, -0.365)).to_array());
        world.set_batch((-3..=3).flat_map(|dy| {
            (-4..=4).map(move |dz| {
                (
                    Cell {
                        y: wall.y + dy,
                        z: wall.z + dz,
                        ..wall
                    },
                    Material::Stone,
                )
            })
        }));
        assert!(!Aircraft::can_board(&world, pose, walk.position));
    }

    #[test]
    fn doors_open_while_parked_and_close_at_taxi_speed() {
        let (world, mut camera) = runway();
        let mut plane = Aircraft::enter(&world, &mut camera);
        assert!(plane.on_ground());
        plane.toggle_doors();
        for _ in 0..45 {
            assert!(plane.advance(&mut camera, &world, input(0., 0., 0.), 0.02));
        }
        assert_eq!(plane.presentation(camera).door, 1.);
        for _ in 0..120 {
            assert!(plane.advance(&mut camera, &world, input(0., 0., 1.), 0.02));
        }
        assert_eq!(plane.presentation(camera).door, 0.);
        plane.tune_radio(1);
        assert_eq!(plane.presentation(camera).radio_khz, 118_025);
    }

    #[test]
    fn altitude_hold_trims_climb_and_yields_to_the_yoke() {
        let (world, mut camera) = runway();
        camera.position.y += 100.;
        camera.pitch = 0.09;
        let start_y = camera.position.y;
        let mut plane = Aircraft::enter(&world, &mut camera);
        plane.toggle_hold(camera);
        for _ in 0..500 {
            assert!(plane.advance(&mut camera, &world, input(0., 0., 0.), 0.02));
        }
        assert!(plane.presentation(camera).hold);
        assert!((camera.position.y - start_y).abs() < 8.);
        assert!(plane.advance(&mut camera, &world, input(1., 0., 0.), 0.02));
        assert!(!plane.presentation(camera).hold);
        assert!(plane.presentation(camera).controls.x > 0.);
    }
    #[test]
    fn parked_plane_follows_edited_support_and_taxi_stops_at_a_wall() {
        let (mut world, mut camera) = runway();
        let mut plane = Aircraft::enter(&world, &mut camera);
        assert!(plane.grounded && plane.airspeed_mps() == 0.);
        let blocker = Cell::from_meters([0., camera.position.y, 4.]);
        world.set(blocker, Material::Stone);
        for _ in 0..40 {
            assert!(plane.advance(&mut camera, &world, input(0., 0., 1.), 0.05));
        }
        assert!(
            plane.grounded && plane.airspeed_mps() < 1. && camera.position.z + 2.61 < 4.07,
            "at {:?} actual speed {} grounded {}",
            camera.position,
            plane.dynamics.velocity.length(),
            plane.grounded
        );
        // Pull power and brake before digging underneath the stationary tyres.
        for _ in 0..48 {
            assert!(plane.advance(&mut camera, &world, input(0., 0., -1.), 0.05));
        }
        world.set(blocker, Material::Air);
        let floor = world.column(0, 0).height_cells() - 1;
        let ground_position = camera.position;
        for wheel in WHEELS.into_iter().chain([NOSE_WHEEL]) {
            let c =
                Cell::from_meters((camera.position + (axes(camera) * wheel).as_dvec3()).to_array());
            world.set_batch((-4..=4).flat_map(|x| {
                (-4..=4).flat_map(move |z| {
                    (floor - 24..=floor).map(move |y| {
                        (
                            Cell {
                                x: c.x + x,
                                y,
                                z: c.z + z,
                            },
                            Material::Air,
                        )
                    })
                })
            }));
        }
        assert!(plane.advance(&mut camera, &world, input(0., 0., 0.), 0.05));
        assert!(!plane.grounded);
        assert!(plane.advance(&mut camera, &world, input(0., 0., 0.), 0.05));
        assert!(camera.position.y < ground_position.y);
    }
    #[test]
    fn steep_impact_does_not_enter_ground_roll() {
        let (world, mut camera) = runway();
        camera.position.y += 4.;
        camera.pitch = -0.45;
        let mut plane = Aircraft::enter(&world, &mut camera);
        assert!(!(0..20).all(|_| plane.advance(&mut camera, &world, input(0., 0., 0.), 0.05)));
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
        let mut plane = Aircraft::enter(&world, &mut camera);
        world.set(Cell::from_meters([0.02, 200., 2.]), Material::Stone);
        assert!(!plane.advance(&mut camera, &world, input(0., 0., 0.), 0.1));
        assert!(camera.position.z < 2.);
        camera.position.y += 1.;
        assert!(plane.advance(&mut camera, &world, input(1., 0., 0.), 0.1));
        assert!(camera.roll > 0.);
    }

    #[test]
    fn stalled_turn_autorotates_with_rudder_neutral_then_recovers() {
        let (world, start) = runway();
        for direction in [-1., 1.] {
            let mut camera = Camera {
                position: DVec3::Y * 1600.,
                ..start
            };
            let mut plane = Aircraft::enter(&world, &mut camera);
            let mut rotations = 0.;
            let mut asymmetric_ticks = 0;
            for tick in 0..(120 * 22) {
                let t = tick as f32 * STEP;
                let pitch = if t < 3. {
                    0.15
                } else if t < 7. {
                    0.38
                } else {
                    1.
                };
                let rudder = if (9.1..14.).contains(&t) {
                    direction
                } else {
                    0.
                };
                let controls = FlightInput {
                    pitch,
                    rudder,
                    throttle_rate: -1.,
                    ..Default::default()
                };
                assert!(plane.advance(&mut camera, &world, controls, STEP));
                if t >= 15. {
                    let omega = axes(camera) * plane.dynamics.angular_velocity;
                    rotations -= direction * omega.y * STEP / std::f32::consts::TAU;
                    let [left, right] = plane.dynamics.wing_separation;
                    if (right - left) * direction > 0.48 && plane.dynamics.velocity.y < -20. {
                        asymmetric_ticks += 1;
                    }
                }
            }
            eprintln!("spin {direction:+}: {rotations:.2} turns in 7 s at neutral rudder, {asymmetric_ticks} asymmetric descending steps");
            assert!(rotations > 1.2 && asymmetric_ticks > 600);
            for tick in 0..(120 * 7) {
                let omega = plane.dynamics.angular_velocity;
                let recovering = tick < 95;
                let controls = FlightInput {
                    rudder: if recovering { -direction } else { 0. },
                    roll: if recovering {
                        0.
                    } else {
                        (-2. * camera.roll - omega.z).clamp(-1., 1.)
                    },
                    pitch: if recovering {
                        -0.45
                    } else {
                        (1.9 * (-0.04 - camera.pitch) + omega.x).clamp(-0.6, 0.7)
                    },
                    ..Default::default()
                };
                assert!(plane.advance(&mut camera, &world, controls, STEP));
            }
            assert!(!plane.instruments(camera).stall_warning);
            assert!(
                plane.dynamics.angular_velocity.length() < 0.45 && camera.view_up().y > 0.85,
                "recovery {:?} up {}",
                plane.dynamics.angular_velocity,
                camera.view_up().y
            );
        }
    }

    #[test]
    fn wing_local_gust_and_loading_change_the_forces() {
        use dynamics::Loading;
        let mut calm = Dynamics::default();
        calm.velocity = Vec3::Z * 37.;
        let mut gust = Dynamics::default();
        gust.velocity = calm.velocity;
        let control = Deflection::default();
        let still =
            calm.aerodynamic_loads(DVec3::ZERO, Mat3::IDENTITY, control, |_| Vec3::ZERO, STEP);
        let lift = gust.aerodynamic_loads(
            DVec3::ZERO,
            Mat3::IDENTITY,
            control,
            |p| Vec3::Y * (0.8 * p.x as f32).clamp(0., 4.),
            STEP,
        );
        assert!(still.moment.z.abs() < 0.01);
        assert!(lift.force.y > still.force.y + 900. && lift.moment.z > 2_000.);
        let mut loaded = Dynamics::default();
        loaded.velocity = calm.velocity;
        loaded.loading = Loading::new(77., 77., 50., 90.);
        let heavy =
            loaded.aerodynamic_loads(DVec3::ZERO, Mat3::IDENTITY, control, |_| Vec3::ZERO, STEP);
        assert!(
            loaded.loading.mass > calm.loading.mass
                && loaded.loading.center.z < calm.loading.center.z
        );
        assert!(heavy.moment.x < still.moment.x);
    }

    #[test]
    fn lands_in_crosswind_brakes_and_reboards_on_edited_deck() {
        let (mut world, mut camera) = runway();
        let deck = Cell::from_meters([0., 2., 0.]);
        // Native blocks form a runway above the procedural ground, wide
        // enough to walk beside either cabin door after stopping.
        let width = (2.7 / voxel_size()).ceil() as i64;
        let length = (170. / voxel_size()).ceil() as i64;
        world.set_batch(
            (-width..=width).flat_map(|x| {
                (-50..length).map(move |z| (Cell { x, y: deck.y, z }, Material::Wood))
            }),
        );
        let top = (deck.y + 1) as f64 * voxel_size();
        camera.position.y = top + 1.65;
        let mut plane = Aircraft::enter(&world, &mut camera);
        camera.position.y = top + 1.57;
        camera.pitch = 0.04;
        camera.roll = -0.04;
        plane.grounded = false;
        plane.dynamics.velocity = Vec3::new(0., -1.65, 29.);
        plane.throttle = 0.;
        plane.set_wind(Vec3::X * 3.5);
        let lateral_controls = |plane: &Aircraft, camera: Camera| {
            let omega = plane.dynamics.angular_velocity;
            let heading_error = camera.yaw + 0.12 * camera.position.x as f32;
            let rudder = (2.3 * heading_error + omega.y).clamp(-0.7, 0.7);
            let roll = (2.4 * (-0.055 - camera.roll) - omega.z).clamp(-0.6, 0.6);
            (roll, rudder)
        };
        for _ in 0..120 {
            let (roll, rudder) = lateral_controls(&plane, camera);
            let pitch = (0.09 - camera.pitch - 0.20 * plane.dynamics.velocity.y
                + 0.3 * plane.dynamics.angular_velocity.x)
                .clamp(-0.6, 0.4);
            let controls = FlightInput {
                rudder,
                ..input(roll, pitch, 0.)
            };
            assert!(plane.advance(&mut camera, &world, controls, STEP));
            if plane.grounded {
                break;
            }
        }
        assert!(plane.grounded && plane.gear_compression.iter().any(|&c| c > 0.));
        for _ in 0..(120 * 11) {
            let (roll, rudder) = lateral_controls(&plane, camera);
            let controls = FlightInput {
                brake: true,
                roll,
                rudder,
                ..Default::default()
            };
            assert!(
                plane.advance(&mut camera, &world, controls, STEP),
                "deck impact during rollout at {:?}",
                camera.position
            );
        }
        assert!(plane.on_ground() && plane.dynamics.velocity.length() < 0.1);
        assert!((plane.airspeed_mps() - 3.5).abs() < 0.15);
        assert!((camera.position.y - top - 1.27).abs() < 0.1);
        let (pose, eye) = plane.step_out(&world, camera).unwrap();
        assert!(Aircraft::can_board(&world, pose, eye.position));
    }

    #[test]
    fn clean_power_off_peak_lift_is_near_skyhawk_stall_speed() {
        let mut peak: f32 = 0.;
        let mut peak_a = 0.;
        for deg in 0..30 {
            let mut model = Dynamics::default();
            model.velocity = Vec3::Z * 27.;
            let body = Mat3::from_rotation_x(-(deg as f32).to_radians());
            let mut lift = 0.;
            for _ in 0..180 {
                let load = model.aerodynamic_loads(
                    DVec3::ZERO,
                    body,
                    Deflection::default(),
                    |_| Vec3::ZERO,
                    STEP,
                );
                lift = (body * load.force).y;
            }
            if lift > peak {
                peak = lift;
                peak_a = deg as f32;
            }
        }
        // Scale the measured steady lift to the 2,550 lb POH weight at sea level.
        let stall_knots = 27. * 1.94384 * (1156.66 * dynamics::GRAVITY / peak).sqrt();
        eprintln!("steady lift peak at {peak_a:.0} degrees, equivalent stall {stall_knots:.1} kt");
        assert!((49.0..56.0).contains(&stall_knots) && (8.0..17.0).contains(&peak_a));
    }

    #[test]
    fn sea_level_level_flight_power_and_drag_balance() {
        let trim = |speed, throttle| {
            let mut model = Dynamics::default();
            model.velocity = Vec3::Z * speed;
            let weight = model.loading.mass * dynamics::GRAVITY;
            let mut eval = |pitch: f32, elevator: f32| {
                let body = Mat3::from_rotation_x(-pitch);
                let controls = Deflection {
                    throttle,
                    pitch: elevator,
                    ..Default::default()
                };
                let loads =
                    model.aerodynamic_loads(DVec3::ZERO, body, controls, |_| Vec3::ZERO, STEP);
                let force = body * loads.force;
                Vec3::new(force.y - weight, loads.moment.x, force.z)
            };
            let (mut pitch, mut elevator) = (0.025_f32, 0.0_f32);
            for _ in 0..10 {
                let balance = eval(pitch, elevator);
                let d_pitch = (eval(pitch + 0.001, elevator) - balance) / 0.001;
                let d_elevator = (eval(pitch, elevator + 0.001) - balance) / 0.001;
                let determinant = d_pitch.x * d_elevator.y - d_elevator.x * d_pitch.y;
                pitch -= (d_elevator.y * balance.x - d_elevator.x * balance.y) / determinant;
                elevator -= (d_pitch.x * balance.y - d_pitch.y * balance.x) / determinant;
            }
            let balance = eval(pitch, elevator);
            assert!(balance.x.abs() < 12. && balance.y.abs() < 12.);
            balance.z
        };
        assert!(trim(55., CRUISE_POWER).abs() < 240.);
        let (mut low, mut high) = (58., 71.);
        assert!(trim(low, 1.) > 0. && trim(high, 1.) < 0.);
        for _ in 0..16 {
            let mid = (low + high) * 0.5;
            if trim(mid, 1.) > 0. {
                low = mid;
            } else {
                high = mid;
            }
        }
        let knots = (low + high) * 0.5 * 1.94384;
        eprintln!("longitudinal full-power sea-level trim: {knots:.1} kt");
        assert!((117.0..132.0).contains(&knots));
    }

    #[test]
    #[ignore = "release timing measurement"]
    fn flight_tick_budget() {
        use std::time::Instant;
        let world = World::fbm(
            1878846491374251281,
            FbmParams {
                recipe: frontier_world::FbmRecipe::SeededWorld,
                ..Default::default()
            },
        )
        .unwrap();
        for (label, ground) in [("seeded-world air", false), ("seeded-world parked", true)] {
            let mut samples = Vec::new();
            for _ in 0..8 {
                let mut start = Camera::fixture();
                start.position = DVec3::new(-128., 0., -128.);
                start.position.y = world.height_m(start.position.x, start.position.z);
                start.yaw = 0.;
                start.roll = 0.;
                start.pitch = 0.;
                let mut plane = if ground {
                    Aircraft::park(&world, &mut start).unwrap()
                } else {
                    start.position.y += 1700.;
                    Aircraft::enter(&world, &mut start)
                };
                for i in 0..750 {
                    let controls = if ground {
                        FlightInput {
                            brake: true,
                            ..Default::default()
                        }
                    } else {
                        FlightInput::default()
                    };
                    let begin = Instant::now();
                    assert!(plane.advance(&mut start, &world, controls, 1. / 60.));
                    if i >= 50 {
                        samples.push(begin.elapsed().as_secs_f64() * 1_000.);
                    }
                }
            }
            samples.sort_unstable_by(f64::total_cmp);
            let at = |f: f64| samples[(samples.len() as f64 * f) as usize];
            eprintln!(
                "{label}, {} samples @ 60 Hz (ms): p50={:.4} p95={:.4} p99={:.4}",
                samples.len(),
                at(0.5),
                at(0.95),
                at(0.99)
            );
        }
    }

    #[test]
    fn new_wingtip_obstacle_blocks_flight() {
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
        let mut plane = Aircraft::enter(&world, &mut camera);
        assert!(plane.advance(&mut camera, &world, input(0., 0., 0.), 0.02));
        let blocker = Cell::from_meters((camera.position + DVec3::new(5.55, 1.12, 1.)).to_array());
        world.set(blocker, Material::Stone);
        assert!(!plane.advance(&mut camera, &world, input(0., 0., 0.), 0.05));
    }
}
