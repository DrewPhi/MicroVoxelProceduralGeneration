//! Bounded hydrology-guided reconstruction of a saved atlas triangle.
//! Shared edge controls depend only on their two global nodes, never a render
//! tile. Channels retain monotone parent profiles and global receiver identity.
//! This is analytic refinement, not a second erosion solve. River discharge is a representation of compiled runoff, not new water.
use super::data::{Data, Node};
use crate::generation::noise::{noise, smoothstep};

#[derive(Clone, Copy)]
pub struct Channel {
    pub a: [f64; 3],
    pub b: [f64; 3],
    /// Signed midpoint bend in source metres, shared by all incident patches.
    pub bend: f64,
    /// Monotone water levels at uniform curve parameters; endpoints use a/b.
    pub profile: [f32; 17],
    pub width: f64,
    pub flow: f64,
    pub local: bool,
    /// Receiver geometry validated against the shared physical bank field.
    pub compiled: bool,
}
impl Channel {
    /// Terrain style v2: physical source metres, independent of voxel resolution
    /// and display compression. A scale of ten represents a 0.3m deep creek.
    pub fn depth(&self) -> f64 {
        (3. + 0.8 * self.flow.max(0.).sqrt()).min(30.)
    }
    pub fn influence(&self) -> f64 {
        (self.width * 0.5).max(1.) * 4.
    }
    pub fn position(&self, t: f64) -> [f64; 3] {
        let t = t.clamp(0., 1.);
        let dx = self.b[0] - self.a[0];
        let dz = self.b[2] - self.a[2];
        let length = dx.hypot(dz).max(1e-12);
        let offset = 4. * t * (1. - t) * self.bend;
        [
            self.a[0] + t * dx - dz / length * offset,
            if self.compiled {
                let f = t * 16.;
                let i = (f.floor() as usize).min(15);
                let level = |j: usize| {
                    if j == 0 {
                        self.a[1]
                    } else if j == 16 {
                        self.b[1]
                    } else {
                        (self.profile[j] as f64).clamp(self.b[1], self.a[1])
                    }
                };
                level(i) + (level(i + 1) - level(i)) * (f - i as f64)
            } else {
                self.a[1] + t * (self.b[1] - self.a[1])
            },
            self.a[2] + t * dz + dx / length * offset,
        ]
    }
    /// Closest curve parameter and horizontal distance. The bounded bend makes
    /// the chord projection a reliable initial guess for this quadratic curve.
    pub fn nearest(&self, p: [f64; 2]) -> (f64, f64) {
        let delta = [self.b[0] - self.a[0], self.b[2] - self.a[2]];
        let length2 = delta[0] * delta[0] + delta[1] * delta[1];
        let length = length2.sqrt().max(1e-12);
        let normal = [-delta[1] / length, delta[0] / length];
        let mut t = (((p[0] - self.a[0]) * delta[0] + (p[1] - self.a[2]) * delta[1])
            / length2.max(1e-12))
        .clamp(0., 1.);
        for _ in 0..4 {
            let q = self.position(t);
            let residual = [q[0] - p[0], q[2] - p[1]];
            let derivative = [
                delta[0] + normal[0] * 4. * self.bend * (1. - 2. * t),
                delta[1] + normal[1] * 4. * self.bend * (1. - 2. * t),
            ];
            let denominator = derivative[0].powi(2) + derivative[1].powi(2)
                - 8. * self.bend * (residual[0] * normal[0] + residual[1] * normal[1]);
            if denominator <= 1e-12 {
                break;
            }
            t = (t - (residual[0] * derivative[0] + residual[1] * derivative[1]) / denominator)
                .clamp(0., 1.);
        }
        let q = self.position(t);
        (t, (q[0] - p[0]).hypot(q[2] - p[1]))
    }
    /// Conservative influence bounds include both curve displacement and banks.
    pub fn bounds(&self) -> [[f64; 2]; 2] {
        let radius = self.influence() + self.bend.abs();
        [
            [
                self.a[0].min(self.b[0]) - radius,
                self.a[2].min(self.b[2]) - radius,
            ],
            [
                self.a[0].max(self.b[0]) + radius,
                self.a[2].max(self.b[2]) + radius,
            ],
        ]
    }
}
#[derive(Clone)]
pub struct Patch {
    pub points: [[f64; 2]; 3],
    pub controls: [f64; 10],
    pub channels: Vec<Channel>,
    pub water: [f64; 3],
    pub wet_depth: [f64; 3],
}
#[derive(Clone, Copy)]
pub struct Surface {
    pub height: f64,
    pub water: f64,
}
fn edge(a: Node, b: Node) -> [f64; 2] {
    let dh = b.height - a.height;
    let tangent = |n: Node| {
        n.gradient[0] as f64 * (b.p[0] - a.p[0]) + n.gradient[1] as f64 * (b.p[1] - a.p[1])
    };
    // Monotonic Hermite limiter prevents invented edge pits and overshoot.
    let limit = |v: f64| {
        if dh == 0. || v * dh <= 0. {
            0.
        } else {
            v.signum() * v.abs().min(1.5 * dh.abs())
        }
    };
    [
        a.height + limit(tangent(a)) / 3.,
        b.height - limit(tangent(b)) / 3.,
    ]
}
impl Patch {
    pub fn build(data: &Data, id: u32) -> Self {
        let mut patch = Self::bank(data, id);
        let lo = std::array::from_fn(|k| {
            patch
                .points
                .iter()
                .map(|p| p[k])
                .fold(f64::INFINITY, f64::min)
        });
        let hi = std::array::from_fn(|k| {
            patch
                .points
                .iter()
                .map(|p| p[k])
                .fold(f64::NEG_INFINITY, f64::max)
        });
        patch.channels = data.channels_in_rect(lo, hi);
        patch
    }
    /// Shared uncarved cubic bank, also used once to constrain curve geometry.
    pub fn bank(data: &Data, id: u32) -> Self {
        let ids = data.triangles[id as usize];
        let n = ids.map(|i| data.nodes[i as usize]);
        let mut controls = [
            n[0].height,
            n[1].height,
            n[2].height,
            0.,
            0.,
            0.,
            0.,
            0.,
            0.,
            0.,
        ];
        for (a, b, k) in [(0, 1, 3), (1, 2, 5), (2, 0, 7)] {
            let mut e = edge(n[a], n[b]);
            let direction = if n[a].receiver == ids[b] {
                Some((a, b))
            } else if n[b].receiver == ids[a] {
                Some((b, a))
            } else {
                None
            };
            if let Some((up, down)) = direction {
                if n[up].discharge >= 0.02 && n[up].water >= n[down].water {
                    e = [
                        (2. * n[a].height + n[b].height) / 3.,
                        (n[a].height + 2. * n[b].height) / 3.,
                    ];
                }
            }
            controls[k] = e[0];
            controls[k + 1] = e[1];
        }
        let mean = controls[..3].iter().sum::<f64>() / 3.;
        let edges = controls[3..9].iter().sum::<f64>() / 6.;
        let lo = controls[..3].iter().copied().fold(f64::INFINITY, f64::min);
        let hi = controls[..3]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        controls[9] = (1.5 * edges - 0.5 * mean).clamp(lo, hi);
        let water = n.map(|n| n.water);
        let wet_depth = n.map(|n| (n.water - n.height).max(0.));
        Self {
            points: n.map(|n| n.p),
            controls,
            channels: Vec::new(),
            water,
            wet_depth,
        }
    }
    pub fn height(&self, w: [f64; 3]) -> f64 {
        let [a, b, c] = w;
        let h = self.controls;
        h[0] * a * a * a
            + h[1] * b * b * b
            + h[2] * c * c * c
            + 3. * (h[3] * a * a * b
                + h[4] * a * b * b
                + h[5] * b * b * c
                + h[6] * b * c * c
                + h[7] * c * c * a
                + h[8] * c * a * a)
            + 6. * h[9] * a * b * c
    }
    pub fn sample(&self, p: [f64; 2], w: [f64; 3], seed: u64, spacing: f64) -> Surface {
        let mut height = self.height(w);
        // Small surface grain only; all major relief comes from the saved solve.
        let grain = |wave, amp| {
            if spacing < wave {
                amp * noise(p[0], p[1], wave, seed ^ wave as u64)
                    * smoothstep(1., 3., wave / spacing.max(0.001))
            } else {
                0.
            }
        };
        height += grain(4., 0.45) + grain(0.5, 0.06) + grain(0.0625, 0.004);
        let base = height;
        let mut water = f64::NEG_INFINITY;
        let mut water_distance = f64::INFINITY;
        let mut local_water = [(0., 0.); 4];
        let mut local_count = 0;
        for s in &self.channels {
            let [lo, hi] = s.bounds();
            if p[0] < lo[0] || p[0] > hi[0] || p[1] < lo[1] || p[1] > hi[1] {
                continue;
            }
            let (t, d) = s.nearest(p);
            let half = s.width * 0.5;
            let influence = s.influence();
            if d >= influence {
                continue;
            }
            let level = s.position(t)[1];
            // Unvalidated profiles cannot excavate suspended ribbons. Compiled
            // receivers retain a bounded bed beneath bank-supported water,
            // including steep native-grid crossings and shared junctions.
            if !s.compiled && level > base + 0.25 {
                continue;
            }
            let depth = s.depth();
            let supported_level = level.min(base);
            let bed_level = if s.compiled { supported_level } else { level };
            let bed = if d <= half {
                bed_level - depth * (1. - (d / half).powi(4))
            } else {
                // A low shoulder/floodplain transitions to the original bank.
                bed_level + depth * 0.35 * smoothstep(half, influence, d)
            };
            let target = bed.max(bed_level - depth);
            let bank_support = if s.compiled {
                1.
            } else {
                1. - smoothstep(0., 0.25, level - base)
            };
            let mut blend = (1. - smoothstep(half, influence, d)) * bank_support;
            let mut fade = 1.;
            if s.local {
                fade = smoothstep(0., 0.002, w.into_iter().fold(1., f64::min));
                blend *= fade;
            }
            height = height.min(base + (target - base) * blend);
            if d < half {
                // A stored parent profile can lie above the reconstructed
                // bank between controls. Rivers only fill their excavated bed;
                // elevated stored lake levels are handled independently below.
                let supported_level = level.min(base);
                if s.local {
                    if local_count < local_water.len() {
                        local_water[local_count] = (supported_level, fade);
                        local_count += 1;
                    }
                } else if s.compiled {
                    // Overlapping incoming ribbons cannot raise another
                    // reach's centerline into a confluence hump. The nearest
                    // canonical channel owns its water; exact ties use one
                    // deterministic lower level, independent of iteration.
                    if d < water_distance || (d == water_distance && supported_level < water) {
                        water_distance = d;
                        water = supported_level;
                    }
                } else {
                    water = water.max(supported_level);
                }
            }
        }
        // Dry corner heights never tilt a compiled lake surface up its bank.
        // Equal levels pool barycentric support; unequal levels stay candidates.
        for i in 0..3 {
            if self.wet_depth[i] <= 0. {
                continue;
            }
            let level = self.water[i];
            let support: f64 = (0..3)
                .filter(|&j| self.wet_depth[j] > 0. && self.water[j] == level)
                .map(|j| w[j] * self.wet_depth[j])
                .sum();
            water = water.max(height + (level - height).max(0.) * smoothstep(0., 0.01, support));
        }
        for &(level, fade) in &local_water[..local_count] {
            water = water.max(height + (level - height).max(0.) * fade);
        }
        Surface {
            height,
            water: water.max(height),
        }
    }
}
