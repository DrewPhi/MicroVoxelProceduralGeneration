use frontier_world::FbmTerrain;
use glam::{DVec2, DVec3};

pub struct Surface {
    pub origin: DVec2,
    pub n: usize,
    pub pitch: f64,
    pub bilinear: bool,
    pub points: Vec<f32>,
}

impl Surface {
    pub fn build(
        terrain: &FbmTerrain,
        origin: DVec2,
        n: usize,
        pitch: f64,
        bilinear: bool,
    ) -> Self {
        let side = n + usize::from(bilinear);
        let mut points = vec![0.; side * side];
        let workers = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(8);
        let rows = side.div_ceil(workers);
        std::thread::scope(|scope| {
            for (part, plane) in points.chunks_mut(rows * side).enumerate() {
                scope.spawn(move || {
                    for (i, height) in plane.iter_mut().enumerate() {
                        let shift = if bilinear { 0. } else { 0.5 };
                        let x = origin.x + (i.rem_euclid(side) as f64 + shift) * pitch;
                        let z = origin.y + ((part * rows + i / side) as f64 + shift) * pitch;
                        *height = if bilinear {
                            terrain.continuous_height_m(x, z)
                        } else {
                            terrain.height_m(x, z)
                        } as f32;
                    }
                });
            }
        });
        Self {
            origin,
            n,
            pitch,
            bilinear,
            points,
        }
    }

    fn corners(&self, x: usize, z: usize) -> [f32; 4] {
        if self.bilinear {
            let a = z * (self.n + 1) + x;
            [
                self.points[a],
                self.points[a + 1],
                self.points[a + self.n + 1],
                self.points[a + self.n + 2],
            ]
        } else {
            [self.points[z * self.n + x]; 4]
        }
    }

    pub fn hierarchy(&self, minmax: bool) -> Vec<Vec<f32>> {
        let channels = if minmax { 2 } else { 1 };
        let mut leaf = Vec::with_capacity(self.n * self.n * channels);
        for z in 0..self.n {
            for x in 0..self.n {
                let h = self.corners(x, z);
                leaf.push(h.into_iter().fold(f32::NEG_INFINITY, f32::max));
                if minmax {
                    leaf.push(h.into_iter().fold(f32::INFINITY, f32::min));
                }
            }
        }
        let mut levels = vec![leaf];
        let mut side = self.n;
        while side > 1 {
            let child = levels.last().unwrap();
            let half = side / 2;
            let mut level = Vec::with_capacity(half * half * channels);
            for y in 0..half {
                for x in 0..half {
                    let a = 2 * (x + y * side) * channels;
                    let neighbors = [
                        a,
                        a + channels,
                        a + side * channels,
                        a + (side + 1) * channels,
                    ];
                    level.push(
                        neighbors
                            .into_iter()
                            .map(|i| child[i])
                            .fold(f32::NEG_INFINITY, f32::max),
                    );
                    if minmax {
                        level.push(
                            neighbors
                                .into_iter()
                                .map(|i| child[i + 1])
                                .fold(f32::INFINITY, f32::min),
                        );
                    }
                }
            }
            levels.push(level);
            side = half;
        }
        levels
    }

    pub fn blend_parent(&mut self, parent: &Self, eye: DVec2) {
        let side = self.n + 1;
        let radius = (self.n as f64 * 0.5 - 66.) * self.pitch;
        for i in 0..self.points.len() {
            let world = self.origin + DVec2::new((i % side) as f64, (i / side) as f64) * self.pitch;
            let distance = (world - eye).abs().max_element();
            let u = ((distance - 0.55 * radius) / (0.45 * radius)).clamp(0., 1.);
            if u == 0. {
                continue;
            }
            let g = (world - parent.origin) / parent.pitch;
            let x = (g.x.floor() as usize).min(parent.n - 1);
            let z = (g.y.floor() as usize).min(parent.n - 1);
            let f = (g - DVec2::new(x as f64, z as f64)).clamp(DVec2::ZERO, DVec2::ONE);
            let [a, b, c, d] = parent.corners(x, z).map(f64::from);
            let h = (a * (1. - f.x) + b * f.x) * (1. - f.y) + (c * (1. - f.x) + d * f.x) * f.y;
            self.points[i] = (f64::from(self.points[i])
                + (h - f64::from(self.points[i])) * u * u * (3. - 2. * u))
                as f32;
        }
    }

    pub fn clip(
        &self,
        eye: DVec3,
        direction: DVec3,
        mut lo: f64,
        mut hi: f64,
    ) -> Option<(f64, f64)> {
        let p = (DVec2::new(eye.x, eye.z) - self.origin) / self.pitch;
        let d = DVec2::new(direction.x, direction.z) / self.pitch;
        for axis in 0..2 {
            if d[axis] == 0. {
                if !(0. ..self.n as f64).contains(&p[axis]) {
                    return None;
                }
            } else {
                let a = -p[axis] / d[axis];
                let b = (self.n as f64 - p[axis]) / d[axis];
                lo = lo.max(a.min(b));
                hi = hi.min(a.max(b));
            }
        }
        (lo < hi).then_some((lo, hi))
    }

    /// Double-precision reference uses a flat DDA with independently computed
    /// roots. It consumes the same finite samples, never the GPU hierarchy.
    pub fn reference(&self, eye: DVec3, direction: DVec3, near: f64, far: f64) -> Option<f64> {
        let p = (DVec2::new(eye.x, eye.z) - self.origin) / self.pitch;
        let d = DVec2::new(direction.x, direction.z) / self.pitch;
        let (mut lo, hi) = self.clip(eye, direction, near, far)?;
        let q = p + d * lo;
        let mut cell = [q.x.floor() as i32, q.y.floor() as i32];
        for i in 0..2 {
            if q[i] == f64::from(cell[i]) && d[i] < 0. {
                cell[i] -= 1;
            }
            cell[i] = cell[i].clamp(0, self.n as i32 - 1);
        }
        loop {
            let exits = [0, 1].map(|i| {
                if d[i] == 0. {
                    f64::INFINITY
                } else {
                    (f64::from(cell[i] + i32::from(d[i] > 0.)) - p[i]) / d[i]
                }
            });
            let end = hi.min(exits[0].min(exits[1])).max(lo);
            let g = self
                .corners(cell[0] as usize, cell[1] as usize)
                .map(f64::from);
            if self.bilinear {
                let gap = |t: f64| {
                    let uv = (p + d * t - DVec2::from_array(cell.map(f64::from)))
                        .clamp(DVec2::ZERO, DVec2::ONE);
                    let height = g[0] * (1. - uv.x) * (1. - uv.y)
                        + g[1] * uv.x * (1. - uv.y)
                        + g[2] * (1. - uv.x) * uv.y
                        + g[3] * uv.x * uv.y;
                    eye.y + direction.y * t - height
                };
                let c = gap(lo);
                if c <= 0. {
                    return Some(lo);
                }
                let e = gap(end);
                let m = gap((lo + end) * 0.5);
                let a = 2. * (e + c - 2. * m);
                let b = e - c - a;
                // Split at the stationary point rather than sharing the
                // shader's quadratic formula. Bisect only a monotone bracket.
                let x = -b / (2. * a);
                let mid = if a > 0. && x > 0. && x < 1. {
                    lo + (end - lo) * x
                } else {
                    end
                };
                if gap(mid) <= 0. {
                    let (mut l, mut r) = (lo, mid);
                    for _ in 0..36 {
                        let t = (l + r) * 0.5;
                        if gap(t) > 0. {
                            l = t;
                        } else {
                            r = t;
                        }
                    }
                    return Some(r);
                }
            } else {
                if eye.y + direction.y * lo <= g[0] {
                    return Some(lo);
                }
                if direction.y < 0. && eye.y + direction.y * end <= g[0] {
                    return Some(((g[0] - eye.y) / direction.y).clamp(lo, end));
                }
            }
            if end >= hi {
                return None;
            }
            for i in 0..2 {
                if exits[i] <= end {
                    cell[i] += if d[i] > 0. { 1 } else { -1 };
                }
            }
            if cell.iter().any(|&v| v < 0 || v >= self.n as i32) {
                return None;
            }
            lo = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_saddle_finds_first_crossing_between_positive_endpoints() {
        let surface = Surface {
            origin: DVec2::ZERO,
            n: 1,
            pitch: 1.,
            bilinear: true,
            points: vec![0., 2., 2., 0.],
        };
        let depth = surface
            .reference(DVec3::new(0., 0.75, 0.), DVec3::new(1., 0., 1.), 0.025, 1.)
            .unwrap();
        assert!((depth - 0.25).abs() < 1e-9);
    }
}
