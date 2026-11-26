//! Six-degree-of-freedom dynamics for the Skyhawk. SI units throughout.
//! Local flow uses body +X left, +Y up and +Z forward.
use super::airframe::*;
use glam::{DVec3, Mat3, Quat, Vec3};
use std::sync::OnceLock;

pub(super) const STEP: f32 = 1. / 120.;
pub(super) const GRAVITY: f32 = 9.80665;
pub(super) const EMPTY_MASS: f32 = 754.;
pub(super) const SEA_LEVEL_DENSITY: f32 = 1.225;
const SECTIONS_PER_SIDE: usize = 8;
pub(crate) const WING_SECTIONS: usize = 2 * SECTIONS_PER_SIDE;
const SECTIONS: usize = WING_SECTIONS + 8 + 4;
const POWER: f32 = 134_000.;
const DISK_AREA: f32 = std::f32::consts::PI * 0.965 * 0.965;

#[derive(Clone, Copy)]
pub(super) struct Loading {
    pub mass: f32,
    pub center: Vec3,
    inertia: Mat3,
    inverse_inertia: Mat3,
}

impl Default for Loading {
    fn default() -> Self {
        Self::new(77., 77., 0., 90.)
    }
}

impl Loading {
    pub fn new(pilot: f32, passenger: f32, baggage: f32, fuel: f32) -> Self {
        let empty_center = Vec3::new(0., 0.06, 0.68);
        let loads = [
            (pilot, Vec3::new(0.30, 0.03, -0.20)),
            (passenger, Vec3::new(-0.30, 0.03, -0.20)),
            (baggage, Vec3::new(0., -0.05, -1.95)),
            (fuel * 0.5, Vec3::new(2.15, 0.89, 0.43)),
            (fuel * 0.5, Vec3::new(-2.15, 0.89, 0.43)),
        ];
        let mass = EMPTY_MASS + loads.iter().map(|(m, _)| m).sum::<f32>();
        let center = loads
            .iter()
            .fold(empty_center * EMPTY_MASS, |s, &(m, p)| s + p * m)
            / mass;
        let mut inertia = Mat3::from_diagonal(Vec3::new(1720., 2050., 990.));
        for (mass, point) in std::iter::once((EMPTY_MASS, empty_center)).chain(loads) {
            let r = point - center;
            let outer = Mat3::from_cols(r * r.x, r * r.y, r * r.z);
            inertia += (Mat3::from_diagonal(Vec3::splat(r.length_squared())) - outer) * mass;
        }
        Self {
            mass,
            center,
            inertia,
            inverse_inertia: inertia.inverse(),
        }
    }

    pub fn remove(&mut self, removed: u64) {
        use crate::aircraft_damage::{asset, live, point_inertia};
        let asset = asset();
        let mut moment = self.center * self.mass;
        let mut inertia = self.inertia + point_inertia(self.center) * self.mass;
        for (i, n) in asset
            .structure
            .iter()
            .enumerate()
            .filter(|(i, _)| live(removed, *i))
        {
            self.mass -= n.mass;
            moment -= n.center * n.mass;
            inertia -= asset.inertia[i] + point_inertia(n.center) * n.mass;
        }
        self.center = moment / self.mass;
        inertia -= point_inertia(self.center) * self.mass;
        // Retain a positive tensor if an authored empty-airframe inertia
        // estimate was lighter than the measured detached voxel parts.
        let off_diagonal = Vec3::new(0., inertia.x_axis.y, inertia.x_axis.z).abs()
            + Vec3::new(inertia.y_axis.x, 0., inertia.y_axis.z).abs()
            + Vec3::new(inertia.z_axis.x, inertia.z_axis.y, 0.).abs();
        let diagonal = Vec3::new(inertia.x_axis.x, inertia.y_axis.y, inertia.z_axis.z);
        self.inertia = inertia
            + Mat3::from_diagonal(
                (off_diagonal + Vec3::splat(self.mass * 0.10) - diagonal).max(Vec3::ZERO),
            );
        self.inverse_inertia = self.inertia.inverse();
    }

    pub fn inverse_inertia(&self) -> Mat3 {
        self.inverse_inertia
    }
}

#[derive(Default, Clone, Copy)]
pub(super) struct Deflection {
    pub roll: f32,
    pub pitch: f32,
    pub rudder: f32,
    pub flaps: f32,
    pub throttle: f32,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Forces {
    pub force: Vec3,
    pub moment: Vec3,
}

impl Forces {
    pub fn apply(&mut self, force: Vec3, arm: Vec3) {
        self.force += force;
        self.moment += arm.cross(force);
    }
}

#[derive(Clone, Copy)]
enum Surface {
    Wing,
    Tail,
    Fin,
}

#[derive(Clone, Copy)]
struct Section {
    surface: Surface,
    center: Vec3,
    span: Vec3,
    normal: Vec3,
    chord: f32,
    area: f32,
    incidence: f32,
    control: f32,
    flap: bool,
}

fn sections() -> &'static [Section; SECTIONS] {
    static SECTIONS_CACHE: OnceLock<[Section; SECTIONS]> = OnceLock::new();
    SECTIONS_CACHE.get_or_init(|| {
        std::array::from_fn(|i| {
            if i >= WING_SECTIONS + 8 {
                let u = (i as f32 - (WING_SECTIONS + 8) as f32 + 0.5) / 4.;
                let (front, rear) = fin_section(u);
                let chord = front - rear;
                return Section {
                    surface: Surface::Fin,
                    center: Vec3::new(0., FIN_ROOT + FIN_HEIGHT * u, front - 0.25 * chord),
                    span: Vec3::Y,
                    normal: -Vec3::X,
                    chord,
                    area: FIN_HEIGHT / 4. * chord,
                    incidence: 0.,
                    control: ((RUDDER_HINGE_Z - rear) / chord).clamp(0., 1.).sqrt(),
                    flap: false,
                };
            }
            let wing = i < WING_SECTIONS;
            let (planform, count, index) = if wing {
                (&WING, SECTIONS_PER_SIDE, i)
            } else {
                (&TAIL, 4, i - WING_SECTIONS)
            };
            let side = if index < count { 1. } else { -1. };
            let u = ((index % count) as f32 + 0.5) / count as f32;
            let (chord, center, height) = planform.section(u);
            let rear = center - chord * 0.5;
            let span = Vec3::new(1., side * planform.rise / planform.span, 0.).normalize();
            Section {
                surface: if wing { Surface::Wing } else { Surface::Tail },
                center: Vec3::new(side * u * planform.span, height, center + 0.25 * chord),
                span,
                normal: Vec3::Z.cross(span),
                chord,
                area: chord * planform.span / count as f32,
                incidence: if wing { 0.068 - 0.045 * u * u } else { 0.014 },
                control: if (planform.control_root..CONTROL_TIP).contains(&u) {
                    ((planform.hinge - rear) / chord).clamp(0., 1.).sqrt()
                } else {
                    0.
                },
                flap: wing && u < FLAP_SPLIT,
            }
        })
    })
}

pub(super) fn density(altitude: f64) -> f32 {
    SEA_LEVEL_DENSITY * (1. - 2.25577e-5 * (altitude as f32).clamp(-500., 11_000.)).powf(4.25588)
}

fn wrap(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

fn smooth(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

#[derive(Clone, Copy, Default)]
pub(crate) struct SectionFlow {
    pub origin: Vec3,
    pub lift: Vec3,
    pub drag: Vec3,
    pub separation: f32,
    pub moment: Vec3,
}

pub(super) struct Dynamics {
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
    pub loading: Loading,
    separation: [f32; SECTIONS],
    pub wing_separation: [f32; 2],
    pub stall_warning: bool,
    pub airspeed: f32,
    pub equivalent_airspeed: f32,
    pub alpha: f32,
    pub relative_air: Vec3,
    pub flow_samples: [SectionFlow; SECTIONS],
    pub propwash: f32,
    pub attached: u64,
}

impl Default for Dynamics {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            loading: Loading::default(),
            separation: [0.; SECTIONS],
            wing_separation: [0.; 2],
            stall_warning: false,
            airspeed: 0.,
            equivalent_airspeed: 0.,
            alpha: 0.,
            relative_air: Vec3::ZERO,
            propwash: 0.,
            attached: crate::aircraft_damage::INTACT,
            flow_samples: [SectionFlow::default(); SECTIONS],
        }
    }
}

impl Dynamics {
    #[cfg(test)]
    pub fn aerodynamic_loads(
        &mut self,
        origin: DVec3,
        body: Mat3,
        control: Deflection,
        wind: impl Fn(DVec3) -> Vec3,
        dt: f32,
    ) -> Forces {
        self.loads_in_air(origin, body, control, wind, dt, 1.)
    }

    pub fn loads_in_air(
        &mut self,
        origin: DVec3,
        body: Mat3,
        control: Deflection,
        wind: impl Fn(DVec3) -> Vec3,
        dt: f32,
        density_ratio: f32,
    ) -> Forces {
        let rho = density(origin.y) * density_ratio;
        let body_from_world = body.transpose();
        let v = body_from_world * self.velocity;
        let cg = self.loading.center;
        let omega = self.angular_velocity;
        let air = v - body_from_world * wind(origin + (body * cg).as_dvec3());
        self.relative_air = -air;
        self.airspeed = air.length();
        self.equivalent_airspeed = self.airspeed * (rho / SEA_LEVEL_DENSITY).sqrt();
        self.alpha = (-air.y).atan2(air.z);

        // Momentum theory for a propeller disk. Solve 2*rho*A*vi*(V+vi)^2 = power.
        use crate::aircraft_damage::{live, section_node, GEAR_FIRST, NOSE, TAILCONE};
        let power = POWER
            * control.throttle
            * (rho / SEA_LEVEL_DENSITY).min(1.)
            * f32::from(live(self.attached, NOSE));
        let useful_power = power * 0.73;
        let axial = air.z.max(0.);
        let disk = 2. * rho * DISK_AREA;
        let static_vi = (useful_power / disk).cbrt();
        let mut slip = static_vi * (static_vi / (static_vi + axial).max(1.)).powi(2);
        for _ in 0..4 {
            let flow = axial + slip;
            let derivative = (disk * flow * (axial + 3. * slip)).max(1.);
            slip -= (disk * slip * flow * flow - useful_power) / derivative;
        }
        self.propwash = 1.25 * slip;
        let thrust = (disk * slip * (axial + slip)).max(0.);
        let mut loads = Forces::default();
        loads.apply(Vec3::Z * thrust, Vec3::new(0., -0.06, PROP_Z) - cg);
        // The crankshaft reacts against the airframe as the propeller spins.
        // The vertical tail also sees a lateral component of the propwash.
        loads.moment.z -= power / 250.;

        // Crossflow on the body and parasite drag from the fuselage and gear.
        for (point, area, group) in [
            (
                Vec3::new(0., -0.12, 1.9),
                Vec3::new(1.04, 0.85, 0.036),
                NOSE,
            ),
            (Vec3::new(0., 0.11, -0.60), Vec3::new(2.7, 2.25, 0.062), 0),
            (
                Vec3::new(0., 0.10, -3.1),
                Vec3::new(1.45, 1.2, 0.027),
                TAILCONE,
            ),
            (
                Vec3::new(0., -0.90, 0.55),
                Vec3::new(0.35, 0.32, 0.067),
                GEAR_FIRST + 2,
            ),
        ] {
            if !live(self.attached, group) {
                continue;
            }
            let r = point - cg;
            let flow =
                v + omega.cross(r) - body_from_world * wind(origin + (body * point).as_dvec3());
            loads.apply(-0.5 * rho * area * flow.abs() * flow, r);
        }

        self.wing_separation = [0.; 2];
        self.stall_warning = false;
        let mut downwash = 0.;
        let wake = smooth(0.25, 0.80, self.alpha.abs());
        for (i, section) in sections().iter().enumerate() {
            if !live(self.attached, section_node(i)) {
                self.flow_samples[i] = SectionFlow {
                    origin: section.center,
                    ..Default::default()
                };
                continue;
            }
            let r = section.center - cg;
            let mut flow = v + omega.cross(r)
                - body_from_world * wind(origin + (body * section.center).as_dvec3());
            let (delta, slope, profile_drag, aspect_factor, pressure) = match section.surface {
                Surface::Wing => {
                    let delta = if section.flap {
                        FLAP_TRAVEL * control.flaps
                    } else {
                        AILERON_TRAVEL * control.roll * section.center.x.signum()
                    };
                    (delta, 5.45, 0.013, 0.060, 1.)
                }
                Surface::Tail => {
                    flow.z += 1.25 * slip;
                    flow.y += axial * downwash * (1. - wake);
                    (
                        -elevator_deflection(control.pitch),
                        4.15,
                        0.014,
                        0.11,
                        1. - 0.60 * wake,
                    )
                }
                Surface::Fin => {
                    flow.z += 0.55 * slip;
                    flow.x += 0.075 * slip;
                    (
                        -RUDDER_TRAVEL * control.rudder,
                        3.75,
                        0.015,
                        0.17,
                        1. - 0.48 * wake,
                    )
                }
            };
            let projected = flow - section.span * flow.dot(section.span);
            let speed_sq = projected.length_squared();
            if speed_sq < 0.01 {
                self.flow_samples[i] = SectionFlow {
                    origin: section.center,
                    ..Default::default()
                };
                continue;
            }
            let dir = projected / speed_sq.sqrt();
            let angle = wrap((-flow.dot(section.normal)).atan2(flow.z) + section.incidence);
            let effectiveness = if matches!(section.surface, Surface::Tail) {
                1.13
            } else {
                0.72
            };
            let camber = delta * section.control * effectiveness;
            let effective = wrap(angle + camber);
            let abs = effective.abs();
            let separation_target = smooth(0.225, 0.350, abs);
            let separation = &mut self.separation[i];
            // Relaxation scales with the chord transit time. It gives a
            // continuous stall break and delayed reattachment during recovery.
            let response = dt * speed_sq.sqrt() / (section.chord * 2.5);
            *separation += (separation_target - *separation) * (response / (1. + response));

            let (sin, cos) = effective.sin_cos();
            let attached_lift = slope * effective.clamp(-0.29, 0.29);
            let separated_lift = 2.04 * sin * cos;
            let lift = attached_lift + *separation * (separated_lift - attached_lift);
            let drag = profile_drag
                + aspect_factor * attached_lift * attached_lift * (1. - *separation)
                + *separation * (0.20 + 1.75 * sin * sin)
                + 0.11 * camber.abs();
            let q_area = 0.5 * rho * speed_sq * section.area * pressure;
            // Center of pressure migrates toward mid-chord as the flow separates.
            let arm = r - Vec3::Z * (0.13 * section.chord * *separation);
            let lift_force = dir.cross(section.span) * (lift * q_area);
            let drag_force = -dir * (drag * q_area);
            loads.apply(lift_force + drag_force, arm);
            let moment = if i < WING_SECTIONS {
                section.span * (q_area * section.chord * 0.020 * (1. - *separation))
            } else {
                Vec3::ZERO
            };
            self.flow_samples[i] = SectionFlow {
                origin: arm + cg,
                lift: lift_force,
                drag: drag_force,
                separation: *separation,
                moment,
            };
            if i < WING_SECTIONS {
                downwash += lift * (0.062 / WING_SECTIONS as f32);
                self.wing_separation[i / SECTIONS_PER_SIDE] +=
                    *separation / SECTIONS_PER_SIDE as f32;
                self.stall_warning |= abs > 0.216 && speed_sq > 64.;
                // Small nose-down moment about the aerodynamic center of a
                // cambered wing. It diminishes as the attached flow separates.
                loads.moment += moment;
            }
        }
        loads
    }

    pub fn integrate(&mut self, body: Mat3, loads: Forces, dt: f32) -> Mat3 {
        self.velocity += (body * loads.force / self.loading.mass - Vec3::Y * GRAVITY) * dt;
        let omega = self.angular_velocity;
        let angular_accel = self.loading.inverse_inertia
            * (loads.moment - omega.cross(self.loading.inertia * omega));
        let next = omega + angular_accel * dt;
        self.angular_velocity = next;
        let rotation = Quat::from_scaled_axis((omega + next) * (0.5 * dt));
        Mat3::from_quat((Quat::from_mat3(&body) * rotation).normalize())
    }
}
