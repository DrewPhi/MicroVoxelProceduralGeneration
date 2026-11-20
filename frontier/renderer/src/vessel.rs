//! Continuous held vessel and counted water parcels, sampled onto the world grid.
use crate::avatar::VesselPose;
use frontier_world::{
    bowl::{Bowl, CAPACITY},
    Cell, Material, World, VOXEL_SIZE,
};
use glam::{DQuat, DVec3};
use std::collections::BTreeSet;

pub const RIM: f64 = 0.252;
const BASE: f64 = 0.142;
const DEPTH: f64 = 0.140;
const WALL: f64 = VOXEL_SIZE;
const PARCEL: f64 = VOXEL_SIZE * 0.5;
const SPACING: f64 = VOXEL_SIZE;
const GRAVITY: DVec3 = DVec3::new(0., -9.81, 0.);

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
        let delta = pose.center - target.center;
        if delta.length_squared() > 0.13 * 0.13 {
            pose.center = target.center + delta.clamp_length_max(0.13);
            let away = delta.normalize();
            self.velocity -= away * self.velocity.dot(away).max(0.);
        }
        self.pose = Some(pose);
        pose
    }

    fn settle_saved(&mut self, pose: VesselPose, units: u32) {
        if self.parcels.len() == units as usize {
            return;
        }
        let mut slots = Vec::new();
        for layer in 0..3 {
            for z in -4..=4 {
                for x in -4..=4 {
                    let local = DVec3::new(
                        f64::from(x) * SPACING,
                        -DEPTH + PARCEL + f64::from(layer) * SPACING,
                        f64::from(z) * SPACING,
                    );
                    if local.x.hypot(local.z) <= radius(local.y) - PARCEL {
                        slots.push(local);
                    }
                }
            }
        }
        slots.sort_by(|a, b| {
            a.y.total_cmp(&b.y)
                .then_with(|| a.length_squared().total_cmp(&b.length_squared()))
        });
        self.parcels = slots
            .into_iter()
            .take(units as usize)
            .map(|local| Parcel {
                position: pose.point(local),
                velocity: DVec3::ZERO,
                escaped: false,
            })
            .collect();
        debug_assert_eq!(self.parcels.len(), units as usize);
    }

    pub fn step(
        &mut self,
        world: &mut World,
        bowl: &mut Bowl,
        target: VesselPose,
        scooping: bool,
        accidental: bool,
        dt: f64,
    ) -> u32 {
        let before = self.pose.unwrap_or(target);
        let pose = self.grip_step(target, bowl.units, dt);
        self.settle_saved(pose, bowl.units);
        let mut filled = 0;
        if scooping && bowl.units < CAPACITY {
            self.intake = (self.intake + dt * 32.).min(1.);
            if self.intake >= 1. {
                filled = self.scoop(world, bowl, pose);
                self.intake -= f64::from(filled);
            }
        } else {
            self.intake = 0.;
        }
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
                    let distance = displacement.length();
                    if distance < SPACING {
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
            if bowl.release_at(world, now, p.velocity.to_array(), accidental) {
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
        debug_assert_eq!(self.parcels.len(), bowl.units as usize);
        filled
    }

    fn scoop(&mut self, world: &mut World, bowl: &mut Bowl, pose: VesselPose) -> u32 {
        let candidates = [
            (0., 0.),
            (-0.13, 0.),
            (0.13, 0.),
            (0., -0.13),
            (0., 0.13),
            (-0.13, 0.13),
            (0.13, 0.13),
        ];
        let mut selected = None;
        for (x, z) in candidates {
            let local = DVec3::new(x, 0.008, z);
            let contact = Cell::from_meters(pose.point(local).to_array());
            if world.material(contact) != Material::Water {
                continue;
            }
            let entry = pose.point(DVec3::new(x, -PARCEL, z));
            let clearance = self
                .parcels
                .iter()
                .map(|p| entry.distance_squared(p.position))
                .fold(f64::INFINITY, f64::min);
            if clearance >= (SPACING * 0.88).powi(2)
                && selected.is_none_or(|(_, _, best)| clearance > best)
            {
                selected = Some((contact, entry, clearance));
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
                CAPACITY
            ),
            Ok(CAPACITY)
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
            vessel.step(&mut world, &mut bowl, target, false, true, 0.01);
            bowl.advance(&mut world, 0.01);
            assert!(bowl.conserved());
        }
        assert_eq!(bowl.units, 0);
        assert_eq!(bowl.spilled, u64::from(CAPACITY));
    }

    #[test]
    fn full_bowl_survives_a_gentle_lift_and_sharp_stop_can_spill() {
        let mut world = World::new(19);
        world.set_batch((-10..=10).flat_map(|x| {
            (-10..=10)
                .flat_map(move |z| (1693..1702).map(move |y| (Cell { x, y, z }, Material::Water)))
        }));
        let mut bowl = Bowl::default();
        let mut vessel = Vessel::default();
        let mut target = VesselPose {
            center: DVec3::new(0., 106.3, 0.),
            rotation: DQuat::IDENTITY,
        };
        for tick in 0..480 {
            if tick >= 180 {
                target.center.y = 106.3 + 0.74 * (1. - (-0.034 * f64::from(tick - 180)).exp());
            }
            vessel.step(&mut world, &mut bowl, target, tick < 180, true, 0.01);
            bowl.advance(&mut world, 0.01);
            assert!(bowl.conserved());
        }
        assert_eq!(bowl.units, CAPACITY);
        assert_eq!(bowl.spilled, 0);

        let (mut world, mut bowl) = loaded();
        let mut vessel = Vessel::default();
        target.center = DVec3::new(0., 100., 0.);
        let mut at_stop = 0;
        for tick in 0..450 {
            if (60..240).contains(&tick) {
                target.center.z += 0.06 * (f64::from(tick - 60) / 50.).min(1.);
            }
            if tick == 240 {
                at_stop = bowl.units;
            }
            vessel.step(&mut world, &mut bowl, target, false, true, 0.01);
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
            vessel.step(&mut world, &mut bowl, pose, false, true, 0.01);
            assert_eq!(bowl.units, CAPACITY, "tick {tick}");
            if tick % 10 == 0 {
                let actual = vessel.pose.unwrap();
                assert_eq!(
                    vessel
                        .display_cells(|cell| {
                            wood(actual.local(DVec3::from_array(cell.center_m())))
                        })
                        .len(),
                    CAPACITY as usize
                );
            }
        }
    }
}
