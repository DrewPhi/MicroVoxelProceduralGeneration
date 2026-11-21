//! A vertical world plane through the eye projects to a straight line even
//! when the camera pitches and banks. Measure displacement along that line.
use frontier_reference::{cross, dot, normalized};

type V3 = [f64; 3];
pub struct View {
    pub eye: V3,
    forward: V3,
    right: V3,
    up: V3,
    focal: f64,
    width: usize,
    height: usize,
    far: f64,
}

impl View {
    pub fn new(
        eye: V3,
        target: V3,
        roll: f64,
        fov: f64,
        width: usize,
        height: usize,
        far: f64,
    ) -> Self {
        let forward = normalized(std::array::from_fn(|i| target[i] - eye[i]));
        let right = normalized(cross(forward, [0., 1., 0.]));
        let up = cross(right, forward);
        let (sin, cos) = roll.to_radians().sin_cos();
        let banked =
            |a: V3, b: V3, sign: f64| std::array::from_fn(|i| a[i] * cos + sign * b[i] * sin);
        Self {
            eye,
            forward,
            right: banked(right, up, -1.),
            up: banked(up, right, 1.),
            focal: 0.5 * height as f64 / (0.5 * fov.to_radians()).tan(),
            width,
            height,
            far,
        }
    }

    fn ray(&self, x: f64, y: f64) -> V3 {
        let u = (x - self.width as f64 * 0.5) / self.focal;
        let v = (self.height as f64 * 0.5 - y) / self.focal;
        std::array::from_fn(|i| self.forward[i] + self.right[i] * u + self.up[i] * v)
    }

    /// The meridian through a pixel on the middle row. Near a vertical view,
    /// the projected zenith can lie inside the image. Retain the half-plane
    /// on the same side of the zenith as the sampled pixel.
    pub fn meridian(&self, x: usize) -> Option<Meridian<'_>> {
        let center = [x as f64 + 0.5, self.height as f64 * 0.5];
        let ray = self.ray(center[0], center[1]);
        let horizontal = ray[0].hypot(ray[2]);
        if horizontal < 1e-9 {
            return None;
        }
        let heading = [ray[0] / horizontal, 0., ray[2] / horizontal];
        let forward = dot(self.forward, heading);
        let right = dot(self.right, heading);
        let up = dot(self.up, heading);
        // Minus the derivative of projected screen position with respect to
        // elevation slope. Its direction is constant on a perspective line.
        let mut down = [
            right * self.forward[1] - self.right[1] * forward,
            self.up[1] * forward - up * self.forward[1],
        ];
        let length = down[0].hypot(down[1]);
        if length < 1e-12 {
            return None;
        }
        down = down.map(|v| v / length);
        let mut entry = f64::NEG_INFINITY;
        let mut exit = f64::INFINITY;
        for (i, size) in [self.width, self.height].into_iter().enumerate() {
            if down[i].abs() > 1e-12 {
                let a = -center[i] / down[i];
                let b = (size as f64 - center[i]) / down[i];
                entry = entry.max(a.min(b));
                exit = exit.min(a.max(b));
            }
        }
        // The forward-depth-normalized screen ray varies linearly along the
        // image line. Clip where its horizontal world direction would reverse.
        let dh =
            (dot(self.right, heading) * down[0] - dot(self.up, heading) * down[1]) / self.focal;
        if dh > 0. {
            entry = entry.max((1e-8 - horizontal) / dh);
        }
        if dh < 0. {
            exit = exit.min((1e-8 - horizontal) / dh);
        }
        (exit > entry).then_some(Meridian {
            view: self,
            heading,
            center,
            down,
            entry,
            exit,
        })
    }
}

pub struct Meridian<'a> {
    view: &'a View,
    heading: V3,
    center: [f64; 2],
    down: [f64; 2],
    entry: f64,
    exit: f64,
}
impl Meridian<'_> {
    fn pixel(&self, distance: f64) -> [f64; 2] {
        std::array::from_fn(|i| self.center[i] + self.down[i] * distance)
    }

    pub fn reference(&self, spacing: f64, height: impl Fn(f64, f64) -> f64) -> f64 {
        let v = self.view;
        let endpoint = |t| {
            let [x, y] = self.pixel(t);
            let ray = v.ray(x, y);
            let horizontal = dot(ray, self.heading).max(1e-9);
            (ray[1] / horizontal, horizontal)
        };
        let (s_hi, r_hi) = endpoint(self.entry);
        let (s_lo, r_lo) = endpoint(self.exit);
        let max_radius = v.far * r_hi.max(r_lo);
        let forward = dot(self.heading, v.forward);
        let pitch = v.forward[1];
        let mut best = s_lo;
        let mut radius = 0.025 * r_hi.min(r_lo);
        loop {
            let h = height(
                v.eye[0] + radius * self.heading[0],
                v.eye[2] + radius * self.heading[2],
            );
            let mut top = ((h - v.eye[1]) / radius).min(s_hi);
            let mut bottom = s_lo;
            // For a point down this vertical column, hardware view depth is
            // radius*(horizontal_forward + elevation_slope*forward.y).
            if pitch.abs() > 1e-12 {
                let a = (0.025 / radius - forward) / pitch;
                let b = (v.far / radius - forward) / pitch;
                top = top.min(a.max(b));
                bottom = bottom.max(a.min(b));
            } else if radius * forward < 0.025 || radius * forward > v.far {
                top = f64::NEG_INFINITY;
            }
            if top >= bottom {
                best = best.max(top);
            }
            if best >= s_hi || radius >= max_radius {
                break;
            }
            radius = (radius + spacing).min(max_radius);
        }
        let mut d = self.heading;
        d[1] = best;
        let depth = dot(d, v.forward);
        let x = v.width as f64 * 0.5 + v.focal * dot(d, v.right) / depth;
        let y = v.height as f64 * 0.5 - v.focal * dot(d, v.up) / depth;
        ((x - self.center[0]) * self.down[0] + (y - self.center[1]) * self.down[1])
            .clamp(self.entry, self.exit)
    }

    pub fn measured(&self, occupied: impl Fn(usize, usize) -> bool) -> f64 {
        let mut t = self.entry;
        while t < self.exit {
            let p = self.pixel((t + 1e-7).min((t + self.exit) * 0.5));
            let x = (p[0].floor() as usize).min(self.view.width - 1);
            let y = (p[1].floor() as usize).min(self.view.height - 1);
            if occupied(x, y) {
                return t;
            }
            let mut next = self.exit;
            for (axis, i) in [x, y].into_iter().enumerate() {
                let step = self.down[axis];
                if step.abs() > 1e-12 {
                    let edge = (i + usize::from(step > 0.)) as f64;
                    let crossing = (edge - self.center[axis]) / step;
                    if crossing > t {
                        next = next.min(crossing);
                    }
                }
            }
            t = next;
        }
        self.exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projected_meridians_measure_an_independently_raycast_flat_sea() {
        for (pitch, roll) in [(0_f64, 0.), (-0.25, 35.), (0.15, -32.), (-0.9, 54.)] {
            let eye = [33., 80., -44.];
            let target = [
                eye[0] + 0.6 * pitch.cos(),
                eye[1] + pitch.sin(),
                eye[2] + 0.8 * pitch.cos(),
            ];
            let view = View::new(eye, target, roll, 60., 600, 400, 1600.);
            for x in (0..600).step_by(5) {
                let Some(ray) = view.meridian(x) else {
                    continue;
                };
                let expected = ray.reference(0.2, |_, _| 0.);
                let measured = ray.measured(|x, y| {
                    let d = view.ray(x as f64 + 0.5, y as f64 + 0.5);
                    let distance = -eye[1] / d[1];
                    (0.025..=1600.).contains(&distance)
                });
                assert!(
                    (expected - measured).abs() < 1.6,
                    "pitch {pitch} roll {roll} x {x} predicted {expected}, observed {measured}"
                );
            }
        }
    }
}
