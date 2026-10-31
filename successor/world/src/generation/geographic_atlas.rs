//! Seeded geographic controls, in meters. Not a moving-plate simulation.
//! Sparse curved orogens carry related forelands, plateaus and uplift histories.
//! Fields do not depend on sample spacing or export bounds; erosion still does.
use super::noise::{mix, noise, smoothstep};

const SECTOR: f64 = 340_000.;
const REACH: f64 = 650_000.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geography {
    pub basement_m: f64,
    pub uplift_m_per_year: f64,
    pub old_fraction: f64,
    pub erodibility: f64,
    pub continentalness: f64,
    pub basin: f64,
}

struct System {
    points: [[f64; 2]; 17],
    width: f64,
    rate: f64,
    old: f64,
    side: f64,
    bounds: [f64; 4],
}

pub struct Atlas {
    seed: u64,
    systems: Vec<System>,
}

fn random(seed: u64, salt: u64) -> f64 {
    (mix(seed ^ salt) >> 11) as f64 / (1u64 << 53) as f64
}

impl Atlas {
    /// Include a halo large enough for every finite-support system affecting bounds.
    pub fn new(seed: u64, bounds: [f64; 4]) -> Self {
        let mut systems = Vec::new();
        for z in ((bounds[1] - REACH) / SECTOR).floor() as i64
            ..=((bounds[3] + REACH) / SECTOR).floor() as i64
        {
            for x in ((bounds[0] - REACH) / SECTOR).floor() as i64
                ..=((bounds[2] + REACH) / SECTOR).floor() as i64
            {
                let key = mix(seed ^ mix(x as u64) ^ mix(z as u64).rotate_left(29));
                // Quiet interiors are intentional; no forced range in each sector.
                if random(key, 1) > 0.64 {
                    continue;
                }
                let center = [
                    (x as f64 + random(key, 2)) * SECTOR,
                    (z as f64 + random(key, 3)) * SECTOR,
                ];
                let angle = random(key, 4) * std::f64::consts::TAU;
                let axis = [angle.cos(), angle.sin()];
                let normal = [-axis[1], axis[0]];
                let length = 170_000. + 350_000. * random(key, 5);
                let bend = (random(key, 6) - 0.5) * 150_000.;
                let phase = random(key, 7) * std::f64::consts::TAU;
                let points = std::array::from_fn(|i| {
                    let t = i as f64 / 16.;
                    let along = (t - 0.5) * length;
                    let cross = bend * (t * std::f64::consts::PI).sin()
                        + 16_000. * (t * std::f64::consts::TAU + phase).sin();
                    [
                        center[0] + axis[0] * along + normal[0] * cross,
                        center[1] + axis[1] * along + normal[1] * cross,
                    ]
                });
                let old = if random(key, 8) < 0.38 { 1. } else { 0. };
                let width = 14_000. + 19_000. * random(key, 9) + old * 13_000.;
                let radius = 4. * width;
                let bounds = [
                    points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min) - radius,
                    points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min) - radius,
                    points
                        .iter()
                        .map(|p| p[0])
                        .fold(f64::NEG_INFINITY, f64::max)
                        + radius,
                    points
                        .iter()
                        .map(|p| p[1])
                        .fold(f64::NEG_INFINITY, f64::max)
                        + radius,
                ];
                systems.push(System {
                    points,
                    width,
                    old,
                    bounds,
                    rate: (0.0009 + 0.0010 * random(key, 10)) * (1. - old * 0.3),
                    side: if random(key, 11) < 0.5 { -1. } else { 1. },
                });
            }
        }
        Self { seed, systems }
    }

    pub fn sample(&self, x: f64, z: f64) -> Geography {
        let n = |w, salt| noise(x + 173_129., z - 318_731., w, self.seed ^ salt);
        // Broad crustal provinces, not final mountain noise. Warped coasts have
        // shelves and deep basins, without a square island falloff at map edges.
        let wx = x + 70_000. * n(430_000., 910);
        let wz = z + 70_000. * n(430_000., 911);
        let continentalness = 0.12
            + noise(wx + 427_111., wz - 831_797., 900_000., self.seed ^ 912)
            + 0.38 * noise(wx - 912_173., wz + 291_731., 330_000., self.seed ^ 913)
            + 0.08 * n(95_000., 914);
        let land = smoothstep(0., 0.14, continentalness);
        let mut uplift: f64 = 0.;
        let mut old_uplift = 0.;
        let mut plateau: f64 = 0.;
        let mut basin: f64 = 0.;
        for s in &self.systems {
            if x < s.bounds[0] || z < s.bounds[1] || x > s.bounds[2] || z > s.bounds[3] {
                continue;
            }
            let mut best = f64::INFINITY;
            let mut signed = 0.;
            let mut along = 0.;
            for (i, pair) in s.points.windows(2).enumerate() {
                let a = pair[0];
                let b = pair[1];
                let dx = b[0] - a[0];
                let dz = b[1] - a[1];
                let t = (((x - a[0]) * dx + (z - a[1]) * dz) / (dx * dx + dz * dz)).clamp(0., 1.);
                let qx = x - a[0] - t * dx;
                let qz = z - a[1] - t * dz;
                let distance = qx * qx + qz * qz;
                if distance < best {
                    best = distance;
                    signed = (dx * qz - dz * qx).signum() * distance.sqrt() * s.side;
                    along = (i as f64 + t) / 16.;
                }
            }
            let d = best.sqrt() / s.width;
            if d >= 4. {
                continue;
            }
            // Compact support avoids dependence on which distant sectors loaded.
            let envelope = smoothstep(0., 0.16, along)
                * smoothstep(0., 0.16, 1. - along)
                * (1. - smoothstep(3., 4., d));
            let belt = (-d * d).exp() * envelope;
            let rate = s.rate * belt * (0.85 + 0.3 * n(65_000., 915));
            uplift += rate;
            old_uplift += rate * s.old;
            // One side's broad backland and opposite foreland share the same arc.
            plateau = plateau.max(envelope * (-((signed / s.width + 1.25) / 1.6).powi(2)).exp());
            basin = basin.max(envelope * (-((signed / s.width - 2.) / 0.9).powi(2)).exp());
        }
        let basement = if continentalness < 0. {
            -160. * smoothstep(0., 0.08, -continentalness)
                - 2600. * smoothstep(0.08, 0.65, -continentalness)
        } else {
            land * (100. + 260. * smoothstep(0., 0.6, continentalness) + 850. * plateau
                - 210. * basin
                + 45. * n(24_000., 916))
        };
        Geography {
            basement_m: basement,
            uplift_m_per_year: land * (uplift + 0.00012 * plateau),
            old_fraction: if uplift > 1e-12 {
                old_uplift / uplift
            } else {
                0.
            },
            erodibility: 4.5e-6 * (1. + 0.55 * n(110_000., 917) + 0.15 * n(27_000., 918)),
            continentalness,
            basin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_are_independent_of_export_bounds_and_order() {
        for seed in [1, 42, 99] {
            let full = Atlas::new(seed, [0., 0., 1e6, 1e6]);
            let crop = Atlas::new(seed, [400_000., 400_000., 500_000., 500_000.]);
            for i in 0..25 {
                let p = [400_000. + i as f64 * 3991., 499_000. - i as f64 * 3113.];
                assert_eq!(full.sample(p[0], p[1]), crop.sample(p[0], p[1]));
            }
        }
    }
    #[test]
    fn seeds_have_coasts_quiet_interiors_and_uplift_without_fixed_locations() {
        let mut signatures = Vec::new();
        for seed in [1, 42, 99, 12345, 7, 19, 2026, 85] {
            let atlas = Atlas::new(seed, [0., 0., 1e6, 1e6]);
            let mut counts = [0usize; 3];
            let mut signature = 0.;
            for z in 0..101 {
                for x in 0..101 {
                    let g = atlas.sample(x as f64 * 10_000., z as f64 * 10_000.);
                    assert!(g.basement_m.is_finite() && g.erodibility > 0.);
                    assert!((0. ..=1.).contains(&g.old_fraction));
                    counts[0] += usize::from(g.basement_m < 0.);
                    counts[1] += usize::from(g.basement_m > 0. && g.uplift_m_per_year < 0.0001);
                    counts[2] += usize::from(g.uplift_m_per_year > 0.0004);
                    signature += g.uplift_m_per_year * x as f64;
                }
            }
            assert!(counts.iter().all(|&n| n > 50), "seed {seed}: {counts:?}");
            signatures.push(signature);
        }
        for pair in signatures.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
    }
}
