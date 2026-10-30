//! Shared river geometry and bounded footprint queries, in meters.
#[derive(Clone)]
pub(super) struct Channel {
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub level: [f64; 2],
    pub half_width: f64,
    pub depth: f64,
    pub bend: f64,
    pub id: u32,
    pub points: [[f64; 2]; 5],
}
/// Bounded static drainage correction report, not a water-flow simulation.
#[derive(Clone, Copy, Debug, Default)]
pub struct RiverDiagnostics {
    pub segments: usize,
    pub reaches: usize,
    pub blocked_spill_links: usize,
    pub straightened_segments: usize,
    pub unresolved_segments: usize,
}
/// Render metadata, never authoritative fractional occupancy.
#[derive(Clone, Copy, Debug)]
pub struct WaterFootprint {
    pub id: u32,
    pub point: [f64; 2],
    pub width_m: f64,
    pub coverage: f64,
}
impl Channel {
    pub fn sample(&self, x: f64, z: f64) -> Option<(f64, f64, f64)> {
        let mut best = (f64::INFINITY, 0.);
        for i in 0..4 {
            let a = self.points[i];
            let b = self.points[i + 1];
            let v = [b[0] - a[0], b[1] - a[1]];
            let d = v[0] * v[0] + v[1] * v[1];
            if d < 1e-12 {
                continue;
            }
            let t = (((x - a[0]) * v[0] + (z - a[1]) * v[1]) / d).clamp(0., 1.);
            let ds = (x - a[0] - t * v[0]).powi(2) + (z - a[1] - t * v[1]).powi(2);
            if ds < best.0 {
                best = (ds, (i as f64 + t) * 0.25);
            }
        }
        let distance = best.0.sqrt();
        (distance <= self.half_width + 16.).then(|| {
            (
                distance,
                self.level[0] * (1. - best.1) + self.level[1] * best.1,
                self.half_width,
            )
        })
    }
    pub fn footprint(&self, origin: [f64; 2], edge: f64) -> Option<WaterFootprint> {
        let mut length = 0.;
        let mut point = self.start;
        let mut hit = false;
        // Clip centerline pieces against an expanded cell. Only local indexed
        // channels are visited; no fine voxel/heightmap reduction is necessary.
        for i in 0..4 {
            let a = self.points[i];
            let b = self.points[i + 1];
            let v = [b[0] - a[0], b[1] - a[1]];
            let (mut lo, mut hi) = (0f64, 1f64);
            for axis in 0..2 {
                let low = origin[axis] - self.half_width;
                let high = origin[axis] + edge + self.half_width;
                if v[axis].abs() < 1e-12 {
                    if a[axis] < low || a[axis] > high {
                        hi = -1.;
                    }
                } else {
                    let t0 = (low - a[axis]) / v[axis];
                    let t1 = (high - a[axis]) / v[axis];
                    lo = lo.max(t0.min(t1));
                    hi = hi.min(t0.max(t1));
                }
            }
            if hi >= lo {
                hit = true;
                length += (hi - lo) * v[0].hypot(v[1]);
                let t = (lo + hi) * 0.5;
                point = [a[0] + t * v[0], a[1] + t * v[1]];
            }
        }
        hit.then(|| WaterFootprint {
            id: self.id,
            point,
            width_m: self.half_width * 2.,
            coverage: (length * self.half_width * 2. / (edge * edge)).clamp(0., 1.),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn narrow_diagonal_footprints_survive_missed_quadrature_points() {
        let c = Channel {
            start: [0., 4.3],
            end: [16., 11.3],
            points: std::array::from_fn(|i| [i as f64 * 4., 4.3 + i as f64 * 1.75]),
            level: [20., 19.],
            half_width: 0.1,
            depth: 0.2,
            bend: 0.,
            id: 7,
        };
        for [x, z] in [[4., 4.], [12., 4.], [4., 12.], [12., 12.]] {
            assert!(c.sample(x, z).unwrap().0 > c.half_width);
        }
        let hit = c.footprint([0., 0.], 16.).unwrap();
        assert_eq!(hit.id, 7);
        assert!(hit.coverage > 0. && hit.coverage < 0.05);
        assert_eq!(hit.width_m, 0.2);
        assert!(c.footprint([100., 100.], 16.).is_none());
        assert!((c.sample(0., 4.3).unwrap().1 - 20.).abs() < 1e-9);
        assert!((c.sample(16., 11.3).unwrap().1 - 19.).abs() < 1e-9);
    }
}
