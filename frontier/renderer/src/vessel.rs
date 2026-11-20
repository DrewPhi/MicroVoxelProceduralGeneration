//! Continuous held vessel and counted water parcels, sampled onto the world grid.
use crate::avatar::VesselPose;
use frontier_world::{bowl::Bowl, Cell, Material, World, VOXEL_SIZE};
use glam::{DQuat, DVec3};
use std::{collections::BTreeSet, sync::OnceLock};

pub const RIM: f64 = 0.252;
const BASE: f64 = 0.142;
const DEPTH: f64 = 0.140;
const WALL: f64 = VOXEL_SIZE;
// Subcell contact distances for the parcel solver. Each parcel's mass
// and displayed volume still represent a full native water cell.
const PARCEL: f64 = VOXEL_SIZE * 0.46;
const SPACING: f64 = VOXEL_SIZE * 0.97;
const INLET_SPACING: f64 = VOXEL_SIZE * 0.5;
const GRAVITY: DVec3 = DVec3::new(0., -9.81, 0.);

/// Approximate brim volume for the HUD. The opening and parcel solver
/// determine how much water actually fits at any pose.
pub fn brim_units() -> f64 {
    std::f64::consts::PI * DEPTH * (RIM * RIM + RIM * BASE + BASE * BASE)
        / (3. * VOXEL_SIZE.powi(3))
}

pub fn grip(side: f64) -> DVec3 {
    DVec3::new(-side * (RIM + WALL * 0.45), -0.035, -0.065)
}

fn radius(y: f64) -> f64 {
    BASE + (RIM - BASE) * (1. + y / DEPTH).clamp(0., 1.)
}

fn inside(local: DVec3) -> bool {
    local.y < 0. && local.y >= -DEPTH && local.x.hypot(local.z) < radius(local.y)
}

pub fn wood(local: DVec3) -> bool {
    local.y <= 0.
        && local.y >= -DEPTH - WALL
        && local.x.hypot(local.z) <= radius(local.y) + WALL
        && !inside(local)
}

#[derive(Clone, Copy)]
struct Parcel {
    position: DVec3,
    velocity: DVec3,
    escaped: bool,
}

#[derive(Default)]
pub struct Vessel {
    pub pose: Option<VesselPose>,
    velocity: DVec3,
    angular_velocity: DVec3,
    parcels: Vec<Parcel>,
    intake: f64,
    overflow_credit: f64,
}

impl Vessel {
    fn grip_step(&mut self, target: VesselPose, water: u32, dt: f64) -> VesselPose {
        let Some(mut pose) = self.pose else {
            self.pose = Some(target);
            return target;
        };
        if pose.center.distance_squared(target.center) > 4. {
            // A teleport moves the entire vessel, including its held contents.
            for parcel in &mut self.parcels {
                parcel.position = target.point(pose.local(parcel.position));
                parcel.velocity = DVec3::ZERO;
            }
            self.velocity = DVec3::ZERO;
            self.angular_velocity = DVec3::ZERO;
            self.pose = Some(target);
            return target;
        }
        let mass = 0.85 + f64::from(water) * VOXEL_SIZE.powi(3) * 1000.;
        let local_mass_center = -DVec3::Y * (DEPTH * 0.54);
        let center_offset = pose.rotation * local_mass_center;
        let mut mass_center = pose.center + center_offset;
        let grip_stiffness = 700.;
        let damping = 1.65 * (grip_stiffness * 2. * mass).sqrt() * 0.5;
        let mut force = DVec3::ZERO;
        let mut torque = DVec3::ZERO;
        for side in [-1., 1.] {
            let anchor = grip(side);
            let point = pose.point(anchor);
            let lever = point - mass_center;
            let point_velocity = self.velocity + self.angular_velocity.cross(lever);
            let grip_force =
                (target.point(anchor) - point) * grip_stiffness - point_velocity * damping;
            force += grip_force;
            torque += lever.cross(grip_force);
        }
        // The reach limit stiffens the grip spring while still accelerating
        // the bowl continuously. Snapping the pose would launch its contents.
        let separation = target.center - pose.center;
        let stretch = separation.length() - 0.13;
        if stretch > 0. {
            force += separation.normalize() * (480. * stretch);
        }
        self.velocity += force / mass * dt;
        mass_center += self.velocity * dt;

        // Wrist orientation adds resistance around the line between the grips.
        let mut rotation_error = target.rotation * pose.rotation.inverse();
        if rotation_error.w < 0. {
            rotation_error = -rotation_error;
        }
        let inertia = mass * 0.040;
        let wrist_stiffness = 32.;
        let wrist_damping = 1.2 * (wrist_stiffness * inertia).sqrt();
        torque += rotation_error.to_scaled_axis() * wrist_stiffness
            - self.angular_velocity * wrist_damping;
        self.angular_velocity += torque / inertia * dt;
        pose.rotation =
            (DQuat::from_scaled_axis(self.angular_velocity * dt) * pose.rotation).normalize();
        pose.center = mass_center - pose.rotation * local_mass_center;
        self.pose = Some(pose);
        pose
    }

    fn settle_saved(&mut self, pose: VesselPose, units: u32) {
        if self.parcels.len() == units as usize {
            return;
        }
        // Saves retain water mass. Reconstruct a close-packed resting
        // arrangement, using the same chamber walls as the live solver.
        let mut slots = Vec::new();
        let layer_spacing = SPACING * (2_f64 / 3.).sqrt();
        let row_spacing = SPACING * 3_f64.sqrt() * 0.5;
        let mut layer = 0_i32;
        loop {
            let y = -DEPTH + PARCEL + f64::from(layer) * layer_spacing;
            if y >= 0. {
                break;
            }
            for z in -5_i32..=5 {
                for x in -5..=5 {
                    let local = DVec3::new(
                        (f64::from(x) + f64::from((z + layer).rem_euclid(2)) * 0.5) * SPACING,
                        y,
                        (f64::from(z) + f64::from(layer.rem_euclid(2)) / 3.) * row_spacing,
                    );
                    if local.x.hypot(local.z) <= radius(local.y) - PARCEL {
                        slots.push(local);
                    }
                }
            }
            layer += 1;
        }
        slots.sort_by(|a, b| {
            a.y.total_cmp(&b.y)
                .then_with(|| a.length_squared().total_cmp(&b.length_squared()))
        });
        self.parcels = (0..units as usize)
            .map(|i| {
                // Very full saves settle the excess through the opening.
                let escaped = i >= slots.len();
                let point = slots.get(i).copied().unwrap_or_else(|| {
                    let step = i - slots.len();
                    DVec3::new(
                        f64::from((step % 7) as u32) * SPACING - 3. * SPACING,
                        PARCEL + f64::from((step / 49) as u32) * SPACING,
                        f64::from((step / 7 % 7) as u32) * SPACING - 3. * SPACING,
                    )
                });
                Parcel {
                    position: pose.point(point),
                    velocity: DVec3::ZERO,
                    escaped,
                }
            })
            .collect();
        debug_assert_eq!(self.parcels.len(), units as usize);
    }

    pub fn step(
        &mut self,
        world: &mut World,
        bowl: &mut Bowl,
        target: VesselPose,
        accidental: bool,
        dt: f64,
    ) -> u32 {
        let before = self.pose.unwrap_or(target);
        let pose = self.grip_step(target, bowl.units, dt);
        self.settle_saved(pose, bowl.units);
        // The dip input positions the vessel. Actual intake happens whenever
        // the opening intersects the pool, independently of the input state.
        self.intake += dt * 32.;
        let filled = if self.intake >= 1. {
            self.intake -= 1.;
            self.scoop(world, bowl, pose)
        } else {
            0
        };
        let old: Vec<_> = self.parcels.iter().map(|p| p.position).collect();
        for (i, parcel) in self.parcels.iter_mut().enumerate() {
            parcel.velocity += GRAVITY * dt;
            parcel.position += parcel.velocity * dt;
            if !parcel.escaped {
                let start = before.local(old[i]);
                let predicted = pose.local(parcel.position);
                parcel.escaped = exits_opening(start, predicted);
                if !parcel.escaped {
                    parcel.position = pose.point(confine(predicted));
                }
            }
        }
        // Pressure correction shares compression between neighboring parcels.
        // Each parcel carries exactly one native water unit.
        for _ in 0..5 {
            for i in 0..self.parcels.len() {
                if self.parcels[i].escaped {
                    continue;
                }
                for j in i + 1..self.parcels.len() {
                    if self.parcels[j].escaped {
                        continue;
                    }
                    let displacement = self.parcels[j].position - self.parcels[i].position;
                    let distance_squared = displacement.length_squared();
                    if distance_squared < SPACING * SPACING {
                        let distance = distance_squared.sqrt();
                        let direction = if distance > 1e-8 {
                            displacement / distance
                        } else {
                            pose.rotation * DVec3::X
                        };
                        let correction = direction * (SPACING - distance) * 0.5;
                        self.parcels[i].position -= correction;
                        self.parcels[j].position += correction;
                    }
                }
            }
            for (i, parcel) in self.parcels.iter_mut().enumerate() {
                if !parcel.escaped {
                    let p = pose.local(parcel.position);
                    parcel.escaped = exits_opening(before.local(old[i]), p);
                    if !parcel.escaped {
                        parcel.position = pose.point(confine(p));
                    }
                }
            }
        }
        for (i, parcel) in self.parcels.iter_mut().enumerate() {
            let velocity = (parcel.position - old[i]) / dt;
            if parcel.escaped {
                parcel.velocity = velocity.clamp_length_max(7.);
                continue;
            }
            let wall_velocity = self.velocity
                + self
                    .angular_velocity
                    .cross(parcel.position - pose.point(-DVec3::Y * (DEPTH * 0.54)));
            let local = pose.local(parcel.position);
            let mut relative = pose.rotation.inverse() * (velocity - wall_velocity);
            if local.y <= -DEPTH + PARCEL + 1e-5 {
                relative.y = relative.y.max(0.);
            }
            let radial = DVec3::new(local.x, 0., local.z);
            if radial.length() >= radius(local.y) - PARCEL - 1e-5 {
                let outward = radial.normalize_or_zero();
                relative -= outward * relative.dot(outward).max(0.);
            }
            // Viscosity limits the response to pressure correction as well as
            // coupling the water to the moving wall.
            parcel.velocity = wall_velocity + pose.rotation * relative * (-6. * dt).exp();
        }
        let mut index = 0;
        self.parcels.retain_mut(|p| {
            let previous = old[index];
            index += 1;
            if !p.escaped {
                return true;
            }
            let now = Cell::from_meters(p.position.to_array());
            let outside_pool = world.material(now) != Material::Water;
            if bowl.release_at(
                world,
                now,
                p.velocity.to_array(),
                accidental && outside_pool,
            ) {
                return false;
            }
            if world.material(now).is_blocking() {
                let last = Cell::from_meters(previous.to_array());
                if bowl.emit_at(world, last, p.velocity.to_array(), accidental) {
                    return false;
                }
                p.position = previous;
                p.velocity = DVec3::ZERO;
            }
            true
        });
        self.overflow(world, bowl, pose, accidental, dt);
        debug_assert_eq!(self.parcels.len(), bowl.units as usize);
        filled
    }

    /// Pressure-driven discharge through the low part of the rim. The
    /// chamber's volume below the lip sets the resting water level. This
    /// also lets a dense surface drain even if coarse parcels lock together.
    fn overflow(
        &mut self,
        world: &mut World,
        bowl: &mut Bowl,
        pose: VesselPose,
        accidental: bool,
        dt: f64,
    ) {
        let up = pose.rotation.inverse() * DVec3::Y;
        let horizontal = DVec3::new(up.x, 0., up.z);
        let level_bowl = horizontal.length_squared() < 1e-6;
        if self.parcels.is_empty() || (level_bowl && f64::from(bowl.units) < brim_units() + 1.) {
            self.overflow_credit = 0.;
            return;
        }
        let downhill = if level_bowl {
            DVec3::Z
        } else {
            -horizontal.normalize()
        };
        let lip = downhill * RIM;
        let outlet = lip + downhill * PARCEL + DVec3::Y * 0.012;
        let cell = Cell::from_meters(pose.point(outlet).to_array());
        if world.material(cell) == Material::Water {
            self.overflow_credit = 0.;
            return;
        }

        let level = up.dot(lip);
        let step = VOXEL_SIZE / 3.;
        let samples = chamber_samples();
        let sample_units = brim_units() / samples.len() as f64;
        let mut below = 0;
        let mut near_level = 0;
        for &sample in samples {
            let relative = up.dot(sample) - level;
            below += usize::from(relative <= 0.);
            near_level += usize::from(relative.abs() < step * 0.5);
        }
        let held = self.parcels.iter().filter(|p| !p.escaped).count();
        let excess = held as f64 - below as f64 * sample_units;
        if excess < 1. {
            self.overflow_credit = 0.;
            return;
        }
        let head = ((excess * step) / (near_level.max(1) as f64 * sample_units)).min(DEPTH);
        let rim_width = 2. * (2. * RIM * head).sqrt().min(RIM);
        // Broad-crested weir approximation in cubic metres per second.
        let speed = (2. * 9.81 * head).sqrt();
        let discharge = 0.42 * rim_width * head * speed;
        self.overflow_credit = (self.overflow_credit + discharge / VOXEL_SIZE.powi(3) * dt).min(1.);
        if self.overflow_credit < 1. {
            return;
        }
        let Some((i, _)) = self
            .parcels
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.escaped)
            .min_by(|(_, a), (_, b)| {
                a.position
                    .distance_squared(pose.point(lip))
                    .total_cmp(&b.position.distance_squared(pose.point(lip)))
            })
        else {
            return;
        };
        let outward = pose.rotation * downhill;
        let velocity =
            self.velocity + self.angular_velocity.cross(pose.rotation * lip) + outward * speed;
        if bowl.release_at(world, cell, velocity.to_array(), accidental) {
            self.parcels.swap_remove(i);
            self.overflow_credit = 0.;
        }
    }

    fn scoop(&mut self, world: &mut World, bowl: &mut Bowl, pose: VesselPose) -> u32 {
        let mut selected = None;
        let span = ((RIM - PARCEL) / INLET_SPACING).floor() as i32;
        for z in -span..=span {
            for x in -span..=span {
                let aperture = DVec3::new(
                    f64::from(x) * INLET_SPACING,
                    0.,
                    f64::from(z) * INLET_SPACING,
                );
                if aperture.length_squared() > (RIM - PARCEL).powi(2) {
                    continue;
                }
                let contact = Cell::from_meters(pose.point(aperture + DVec3::Y * 0.008).to_array());
                if world.material(contact) != Material::Water {
                    continue;
                }
                let entry = pose.point(confine(aperture - DVec3::Y * PARCEL));
                let clearance = self
                    .parcels
                    .iter()
                    .filter(|p| !p.escaped)
                    .map(|p| entry.distance_squared(p.position))
                    .fold(f64::INFINITY, f64::min);
                if clearance >= (SPACING * 0.94).powi(2)
                    && selected.is_none_or(|(_, _, best)| clearance > best)
                {
                    selected = Some((contact, entry, clearance));
                }
            }
        }
        if let Some((contact, entry, _)) = selected {
            let amount = bowl.scoop_at(world, contact, 1).unwrap_or(0);
            if amount > 0 {
                self.parcels.push(Parcel {
                    position: entry,
                    velocity: self.velocity - pose.rotation * DVec3::Y * 0.22,
                    escaped: false,
                });
                return amount;
            }
        }
        0
    }

    pub fn display_cells(&self, occupied: impl Fn(Cell) -> bool) -> Vec<Cell> {
        let Some(pose) = self.pose else {
            return Vec::new();
        };
        let mut chosen = BTreeSet::new();
        for parcel in &self.parcels {
            let near = Cell::from_meters(parcel.position.to_array());
            let mut best = None;
            for radius in 0_i64..=4 {
                for z in -radius..=radius {
                    for y in -radius..=radius {
                        for x in -radius..=radius {
                            if x.abs().max(y.abs()).max(z.abs()) != radius {
                                continue;
                            }
                            let cell = Cell {
                                x: near.x + x,
                                y: near.y + y,
                                z: near.z + z,
                            };
                            if chosen.contains(&cell) || occupied(cell) {
                                continue;
                            }
                            let center = DVec3::from_array(cell.center_m());
                            if !parcel.escaped && !inside(pose.local(center)) {
                                continue;
                            }
                            let distance = parcel.position.distance_squared(center);
                            if best.is_none_or(|(_, previous)| distance < previous) {
                                best = Some((cell, distance));
                            }
                        }
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            if let Some((cell, _)) = best {
                chosen.insert(cell);
            }
        }
        chosen.into_iter().collect()
    }
}

fn chamber_samples() -> &'static [DVec3] {
    static SAMPLES: OnceLock<Vec<DVec3>> = OnceLock::new();
    SAMPLES.get_or_init(|| {
        let step = VOXEL_SIZE / 3.;
        let span = (RIM / step).ceil() as i32;
        let layers = (DEPTH / step).ceil() as i32;
        let mut samples = Vec::new();
        for y in 0..layers {
            for z in -span..=span {
                for x in -span..=span {
                    let p = DVec3::new(
                        f64::from(x) * step,
                        -(f64::from(y) + 0.5) * step,
                        f64::from(z) * step,
                    );
                    if inside(p) {
                        samples.push(p);
                    }
                }
            }
        }
        samples
    })
}

fn exits_opening(from: DVec3, to: DVec3) -> bool {
    if to.y <= 0. || to.y <= from.y {
        return false;
    }
    let crossing = from.lerp(to, (-from.y / (to.y - from.y)).clamp(0., 1.));
    crossing.x.hypot(crossing.z) < RIM
}

fn confine(mut local: DVec3) -> DVec3 {
    local.y = local.y.clamp(-DEPTH + PARCEL, -0.001);
    let separation = local.x.hypot(local.z);
    let limit = radius(local.y) - PARCEL;
    if separation > limit {
        local.x *= limit / separation;
        local.z *= limit / separation;
    }
    local
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE_UNITS: u32 = 20;

    #[test]
    fn chamber_only_opens_through_lip() {
        assert!(!exits_opening(
            DVec3::new(0., -0.04, 0.),
            DVec3::new(0.34, -0.04, 0.)
        ));
        assert!(exits_opening(
            DVec3::new(0., -0.04, 0.),
            DVec3::new(0.06, 0.04, 0.)
        ));
    }

    fn loaded() -> (World, Bowl) {
        let mut world = World::new(19);
        let pool = 1700;
        world.set_batch(
            (-6..=6).flat_map(|x| (-6..=6).map(move |z| (Cell { x, y: pool, z }, Material::Water))),
        );
        let mut bowl = Bowl::default();
        assert_eq!(
            bowl.scoop_at(
                &mut world,
                Cell {
                    x: 0,
                    y: pool,
                    z: 0
                },
                SAMPLE_UNITS
            ),
            Ok(SAMPLE_UNITS)
        );
        (world, bowl)
    }

    #[test]
    fn orientation_spills_without_a_player_and_conserves_mass() {
        let mut vessel = Vessel::default();
        let (mut world, mut bowl) = loaded();
        let mut target = VesselPose {
            center: DVec3::new(0., 100., 0.),
            rotation: DQuat::IDENTITY,
        };
        for i in 0..600 {
            target.rotation = DQuat::from_rotation_x((i as f64 / 90.).min(1.) * 2.4);
            vessel.step(&mut world, &mut bowl, target, true, 0.01);
            bowl.advance(&mut world, 0.01);
            assert!(bowl.conserved());
        }
        assert_eq!(bowl.units, 0);
        assert_eq!(bowl.spilled, u64::from(SAMPLE_UNITS));
    }

    #[test]
    fn submerged_bowl_fills_to_its_brim_and_survives_a_gentle_lift() {
        let mut world = World::new(19);
        world.set_batch((-12..=12).flat_map(|x| {
            (-12..=12)
                .flat_map(move |z| (1693..1703).map(move |y| (Cell { x, y, z }, Material::Water)))
        }));
        let mut bowl = Bowl::default();
        let mut vessel = Vessel::default();
        let mut target = VesselPose {
            center: DVec3::new(0., 106.3, 0.),
            rotation: DQuat::IDENTITY,
        };
        for _ in 0..1000 {
            vessel.step(&mut world, &mut bowl, target, true, 0.01);
            bowl.advance(&mut world, 0.01);
            assert!(bowl.conserved());
        }
        let full = bowl.units;
        assert!(
            f64::from(full) > brim_units() * 0.90,
            "full bowl held {full} units"
        );
        assert!(f64::from(full) < brim_units() * 1.05);
        for tick in 0..300 {
            target.center.y = 106.3 + 0.74 * (1. - (-0.034 * f64::from(tick)).exp());
            vessel.step(&mut world, &mut bowl, target, true, 0.01);
            bowl.advance(&mut world, 0.01);
            assert!(bowl.conserved());
        }
        assert!(bowl.units >= full - 2);
        let restored = Bowl::restore(&bowl.snapshot(19), 19).unwrap();
        assert_eq!(restored.units, bowl.units);
        assert!(restored.conserved());
    }

    #[test]
    fn a_lower_rim_drains_a_submerged_fill_after_lifting() {
        let mut world = World::new(19);
        world.set_batch((-12..=12).flat_map(|x| {
            (-12..=12)
                .flat_map(move |z| (1693..1703).map(move |y| (Cell { x, y, z }, Material::Water)))
        }));
        let mut bowl = Bowl::default();
        let mut vessel = Vessel::default();
        let mut target = VesselPose {
            center: DVec3::new(0., 106.3, 0.),
            rotation: DQuat::from_rotation_x(0.30),
        };
        for tick in 0..1400 {
            if tick >= 1000 {
                target.center.y = 106.3 + 0.74 * (1. - (-0.02 * f64::from(tick - 1000)).exp());
            }
            vessel.step(&mut world, &mut bowl, target, true, 0.01);
            bowl.advance(&mut world, 0.01);
            assert!(bowl.conserved());
        }
        assert!(bowl.units > 0 && f64::from(bowl.units) < brim_units() * 0.75);
        assert!(bowl.spilled > 0);
    }

    #[test]
    fn sharp_stop_can_spill_a_loaded_bowl() {
        let (mut world, mut bowl) = loaded();
        let source = Cell {
            x: 0,
            y: 1700,
            z: 0,
        };
        bowl.scoop_at(
            &mut world,
            source,
            brim_units().floor() as u32 - SAMPLE_UNITS,
        )
        .unwrap();
        let mut vessel = Vessel::default();
        let mut target = VesselPose {
            center: DVec3::new(0., 100., 0.),
            rotation: DQuat::IDENTITY,
        };
        let mut at_stop = 0;
        for tick in 0..450 {
            if (60..240).contains(&tick) {
                target.center.z += 0.06 * (f64::from(tick - 60) / 50.).min(1.);
            }
            if tick == 240 {
                at_stop = bowl.units;
            }
            vessel.step(&mut world, &mut bowl, target, true, 0.01);
            bowl.advance(&mut world, 0.01);
            assert!(bowl.conserved());
        }
        assert!(bowl.units > 0 && bowl.units < at_stop);
    }

    #[test]
    fn slow_yaw_rest_keeps_native_units_and_grid_cells() {
        let mut vessel = Vessel::default();
        let (mut world, mut bowl) = loaded();
        for tick in 0..1200 {
            let pose = VesselPose {
                center: DVec3::new(0.012, 100., -0.017),
                rotation: DQuat::from_rotation_y(f64::from(tick) * std::f64::consts::TAU / 1200.),
            };
            vessel.step(&mut world, &mut bowl, pose, true, 0.01);
            assert_eq!(bowl.units, SAMPLE_UNITS, "tick {tick}");
            if tick % 10 == 0 {
                let actual = vessel.pose.unwrap();
                assert_eq!(
                    vessel
                        .display_cells(|cell| {
                            wood(actual.local(DVec3::from_array(cell.center_m())))
                        })
                        .len(),
                    SAMPLE_UNITS as usize
                );
            }
        }
    }
}
