//! Rigid bowl with a shallow free surface and conserved native-cell overflow.
use crate::avatar::{VesselPose, BASE_RADIUS, DEPTH, RIM};
use frontier_world::{
    bowl::{Bowl, CAPACITY},
    Cell, Material, World,
};
use glam::{DVec2, DVec3};

// Usable capacity leaves space above the water in a filled bowl.
const FILL_FRACTION: f64 = 0.48;

#[derive(Default)]
pub struct Vessel {
    pub pose: Option<VesselPose>,
    pub slope: DVec2,
    pub level: f64,
    previous: Option<DVec3>,
    velocity: DVec3,
    swirl: DVec2,
    swirl_speed: DVec2,
    overflow: f64,
    lip: DVec3,
}

impl Vessel {
    pub fn update(&mut self, pose: VesselPose, dt: f64) {
        let target_velocity = self
            .previous
            .replace(pose.center)
            .map_or(DVec3::ZERO, |last| {
                ((pose.center - last) / dt).clamp_length_max(8.)
            });
        let old_velocity = self.velocity;
        self.velocity = old_velocity.lerp(target_velocity, (dt * 16.).min(1.));
        let local_acceleration = pose.rotation.inverse() * (self.velocity - old_velocity) / dt;
        let target = -DVec2::new(local_acceleration.x, local_acceleration.z).clamp_length_max(14.)
            * (0.15 / 9.81);
        for _ in 0..4 {
            let step = dt / 4.;
            self.swirl_speed += ((target - self.swirl) * 65. - self.swirl_speed * 9.) * step;
            self.swirl += self.swirl_speed * step;
        }
        let up = pose.rotation.inverse() * DVec3::Y;
        self.slope = (-DVec2::new(up.x, up.z) / up.y.max(0.20) + self.swirl).clamp_length_max(4.5);
        let downhill = self.slope.normalize_or_zero();
        self.lip = pose.point(DVec3::new(downhill.x * RIM, 0., downhill.y * RIM));
        self.pose = Some(pose);
    }

    pub fn step(
        &mut self,
        world: &mut World,
        bowl: &mut Bowl,
        scoop: bool,
        accidental: bool,
        dt: f64,
    ) -> u32 {
        let Some(pose) = self.pose else { return 0 };
        let contact = if scoop {
            water_at_rim(world, pose)
        } else {
            None
        };
        let retained = if bowl.units == 0 || (pose.rotation * DVec3::Y).y < 0. {
            0
        } else {
            (fill_at(self.slope, -RIM * self.slope.length()) * CAPACITY as f64 / FILL_FRACTION
                + 0.35)
                .floor() as u32
        };

        let mut filled = 0;
        if let Some(cell) = contact {
            filled = bowl.scoop_at(world, cell, 4).unwrap_or(0);
            self.overflow = 0.;
        } else if bowl.units > retained {
            let excess = bowl.units - retained;
            self.overflow += dt * (11. + 2.9 * f64::from(excess)).min(66.);
            while self.overflow >= 1. && bowl.units > retained {
                self.overflow -= 1.;
                let outward =
                    DVec3::new(self.lip.x - pose.center.x, 0., self.lip.z - pose.center.z)
                        .normalize_or_zero();
                let across = DVec3::Y.cross(outward);
                let sign = if bowl.emitted.is_multiple_of(2) {
                    1.
                } else {
                    -1.
                };
                let velocity =
                    self.velocity.clamp_length_max(2.1) + outward * 1.55 - DVec3::Y * 0.7;
                for (side, lift) in [
                    (0., 0.),
                    (0.065, 0.),
                    (-0.065, 0.),
                    (0., 0.072),
                    (0.12, 0.072),
                    (-0.12, 0.072),
                ] {
                    let point = self.lip + outward * 0.035 + across * sign * side + DVec3::Y * lift;
                    if bowl.emit_at(
                        world,
                        Cell::from_meters(point.to_array()),
                        velocity.to_array(),
                        accidental,
                    ) {
                        break;
                    }
                }
            }
        } else {
            self.overflow = 0.;
        }
        if bowl.units == 0 {
            return filled;
        }
        let fraction = f64::from(bowl.units) * FILL_FRACTION / f64::from(CAPACITY);
        let mut low = -DEPTH - RIM * self.slope.length();
        let mut high = RIM * self.slope.length();
        for _ in 0..11 {
            let mid = (low + high) * 0.5;
            if fill_at(self.slope, mid) < fraction {
                low = mid;
            } else {
                high = mid;
            }
        }
        self.level = (low + high) * 0.5;
        filled
    }
}

fn water_at_rim(world: &World, pose: VesselPose) -> Option<Cell> {
    for i in 0..16 {
        let angle = i as f64 * std::f64::consts::TAU / 16.;
        let (z, x) = angle.sin_cos();
        let point = pose.point(DVec3::new(x * RIM, 0., z * RIM));
        let cell = Cell::from_meters(point.to_array());
        if world.material(cell) == Material::Water {
            return Some(cell);
        }
    }
    None
}

fn interior_bottom(radius: f64) -> f64 {
    -DEPTH * (1. - (radius / RIM - BASE_RADIUS).max(0.) / (1. - BASE_RADIUS))
}

fn fill_at(slope: DVec2, level: f64) -> f64 {
    let mut capacity = 0.;
    let mut filled = 0.;
    for z in -4..=4 {
        for x in -4..=4 {
            let point = DVec2::new(x as f64, z as f64) * (RIM / 4.5);
            let bottom = interior_bottom(point.length());
            if bottom < 0. {
                capacity -= bottom;
                filled += ((level + slope.dot(point)).min(0.) - bottom).max(0.);
            }
        }
    }
    filled / capacity
}

/// Surface perimeter clipped to the tapered interior. A fan from any point
/// inside the clipped polygon lies on the same plane as the visible waves.
pub fn surface(slope: DVec2, level: f64, ring: &[DVec2]) -> Vec<DVec3> {
    let ring = ring.iter().map(|p| *p * (RIM - 0.011)).collect::<Vec<_>>();
    let mut polygon = ring
        .iter()
        .map(|p| DVec3::new(p.x, level + slope.dot(*p), p.y))
        .collect::<Vec<_>>();
    let mut clip = |normal: DVec3, offset: f64| {
        let input = std::mem::take(&mut polygon);
        let Some(&last) = input.last() else { return };
        let mut previous = last;
        let mut a = previous.dot(normal) + offset;
        for point in input {
            let b = point.dot(normal) + offset;
            if (a >= 0.) != (b >= 0.) {
                polygon.push(previous.lerp(point, a / (a - b)));
            }
            if b >= 0. {
                polygon.push(point);
            }
            previous = point;
            a = b;
        }
    };
    clip(-DVec3::Y, -0.004);
    clip(DVec3::Y, DEPTH);
    let taper = (RIM - 0.011) * (1. - BASE_RADIUS) / DEPTH;
    for i in 0..ring.len() {
        let side = (ring[i] + ring[(i + 1) % ring.len()]).normalize();
        clip(DVec3::new(-side.x, taper, -side.y), side.dot(ring[i]));
    }
    polygon
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilt_reduces_retained_water() {
        assert_eq!(fill_at(DVec2::ZERO, 0.), 1.);
        let slight = DVec2::new(0.1, 0.);
        let steep = DVec2::new(0.8, 0.);
        let retained = |slope: DVec2| fill_at(slope, -RIM * slope.length());
        assert!(retained(slight) > 0.35 && retained(slight) < 0.8);
        assert!(retained(steep) < 0.15);
    }
}
