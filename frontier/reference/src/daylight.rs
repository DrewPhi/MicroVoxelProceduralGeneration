//! Offline daylight, reflections and transmission for the native terrain reference.
//! The geometry query always returns a voxel face. Optical ripples change only
//! its shading normal. Radiance is accumulated in linear light by the caller.
use frontier_reference::{
    cross, dot, landscape::LandscapeRays, material, normalized, vegetation::Vegetation, Hit,
};
use frontier_world::{terrain::TerrainColumn, voxel_size, FbmTerrain, Material};
use std::array::from_fn;

type V = [f64; 3];

/// Resolve first contact across the native terrain and occupied plant cells.
/// Solids in the procedural terrain take precedence over coincident plant cells.
pub(super) struct Scene<'a> {
    pub landscape: &'a dyn LandscapeRays,
    pub plants: &'a Vegetation,
}

impl Scene<'_> {
    pub fn ray(&self, origin: V, direction: V, near: f64, far: f64) -> Option<(Hit, Option<V>)> {
        let ground = self.landscape.ray(origin, direction, near, far);
        let end = ground.map_or(far, |h| h.distance);
        if let Some(plant) = self.plants.ray(origin, direction, near, end) {
            if ground.is_none_or(|g| plant.hit.distance < g.distance) {
                return Some((plant.hit, Some(plant.color.map(f64::from))));
            }
        }
        ground.map(|g| (g, None))
    }
}

fn offset(p: V, d: V, t: f64) -> V {
    from_fn(|i| p[i] + d[i] * t)
}
fn mix(a: V, b: V, t: f64) -> V {
    from_fn(|i| a[i] + (b[i] - a[i]) * t)
}
fn normal(hit: Hit) -> V {
    let mut n = [0.; 3];
    n[hit.face / 2] = if hit.face % 2 == 0 { -1. } else { 1. };
    n
}

// Continuous value noise, anchored to the world like the water surface.
fn noise(x: f64, z: f64) -> f64 {
    let hash = |x: i64, z: i64| {
        let mut h = (x as u32).wrapping_mul(0x8da6b343) ^ (z as u32).wrapping_mul(0xd8163841);
        h = (h ^ (h >> 16)).wrapping_mul(0x7feb352d);
        f64::from((h ^ (h >> 15)) & 65535) / 65535.
    };
    let (ix, iz) = (x.floor() as i64, z.floor() as i64);
    let ease = |v: f64| v * v * (3. - 2. * v);
    let (u, v) = (ease(x - x.floor()), ease(z - z.floor()));
    let a = hash(ix, iz) * (1. - u) + hash(ix + 1, iz) * u;
    let b = hash(ix, iz + 1) * (1. - u) + hash(ix + 1, iz + 1) * u;
    a + (b - a) * v
}

#[derive(Clone, Copy)]
struct Pigment {
    cell: [i64; 2],
    column: TerrainColumn,
    surface: V,
}

pub(super) struct Daylight<'a> {
    terrain: &'a FbmTerrain,
    pigments: Vec<Option<Pigment>>,
    sun: V,
    overcast: f64,
    sun_basis: [V; 2],
}

impl<'a> Daylight<'a> {
    pub(super) fn new(terrain: &'a FbmTerrain, sun: V, overcast: f64) -> Self {
        let t = normalized(cross(
            sun,
            if sun[1].abs() > 0.95 {
                [1., 0., 0.]
            } else {
                [0., 1., 0.]
            },
        ));
        Self {
            terrain,
            pigments: vec![None; 8192],
            sun,
            overcast,
            sun_basis: [t, cross(sun, t)],
        }
    }

    pub(super) fn sun_sample(&self, u: f64, v: f64) -> V {
        // The sun subtends about 0.53 degrees. Different subpixel rays see
        // different parts of the disk through gaps between leaves.
        let r = 0.00465 * u.sqrt();
        let (sin, cos) = (v * std::f64::consts::TAU).sin_cos();
        normalized(from_fn(|a| {
            self.sun[a] + r * (cos * self.sun_basis[0][a] + sin * self.sun_basis[1][a])
        }))
    }

    fn pigment(&mut self, cell: [i64; 2]) -> Pigment {
        let hash = (cell[0] as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (cell[1] as u64).wrapping_mul(0x94d049bb133111eb);
        let slot = (hash >> 51) as usize;
        if let Some(p) = self.pigments[slot].filter(|p| p.cell == cell) {
            return p;
        }
        let (column, color) = self.terrain.landscape_column(cell[0], cell[1], false);
        let p = Pigment {
            cell,
            column,
            surface: color.map(f64::from),
        };
        self.pigments[slot] = Some(p);
        p
    }

    pub(super) fn sky(&self, d: V) -> V {
        let elevation = d[1].max(0.).sqrt();
        let sky = mix([0.55, 0.70, 0.85], [0.095, 0.285, 0.58], elevation);
        let glow = dot(d, self.sun).max(0.).powi(16) * (1. - self.overcast);
        let sky = mix(sky, [0.55, 0.62, 0.69], self.overcast * 0.72);
        from_fn(|i| sky[i] + glow * [0.22, 0.155, 0.075][i])
    }

    fn light(&self, n: V, visibility: f64, ambient: f64) -> V {
        let hemi = 0.5 + 0.5 * n[1];
        let fill = mix([0.12, 0.145, 0.075], [0.25, 0.33, 0.43], hemi);
        let solar = mix(
            [2.12, 1.44, 0.83],
            [2., 1.78, 1.46],
            ((self.sun[1] - 0.06) / 0.58).clamp(0., 1.),
        );
        let direct = dot(n, self.sun).max(0.) * visibility * (1. - 0.72 * self.overcast);
        from_fn(|i| fill[i] * ambient * (1. + 0.3 * self.overcast) + solar[i] * direct)
    }

    fn solid(&mut self, hit: Hit, plant: Option<V>, visibility: f64, ambient: f64) -> V {
        let albedo = plant.unwrap_or_else(|| {
            let p = self.pigment([hit.cell[0], hit.cell[2]]);
            let m = material(hit.material);
            if m == p.column.top_material() {
                p.surface
            } else {
                m.color().map(f64::from)
            }
        });
        let light = self.light(normal(hit), visibility, ambient);
        from_fn(|i| albedo[i] * light[i])
    }

    // Walk the native underwater columns. A vertical roof and the submerged
    // side of its neighbour can intercept the ray on different materials.
    fn transmitted(&mut self, origin: V, d: V) -> V {
        let cell = voxel_size();
        let mut t = 0.;
        let mut ij = [0, 2].map(|a| (origin[a] / cell).floor() as i64);
        let mut side = [0., 1., 0.];
        let mut wet_length = 0.;
        let deep = [0.013, 0.063, 0.056];
        loop {
            let p = self.pigment(ij);
            let bed = p.column.ground_top as f64 * cell;
            let water = p.column.visible_top as f64 * cell;
            let next: [f64; 2] = from_fn(|i| {
                let a = i * 2;
                if d[a] == 0. {
                    return f64::INFINITY;
                }
                ((ij[i] + i64::from(d[a] > 0.)) as f64 * cell - origin[a]) / d[a]
            });
            let end = next[0].min(next[1]).min(16.);
            let y = origin[1] + t * d[1];
            let roof = (bed - origin[1]) / d[1];
            let hits_roof = d[1] < 0. && roof >= t && roof < end;
            let stop = if y < bed {
                t
            } else if hits_roof {
                roof
            } else {
                end
            };
            if water > bed {
                let crossing = (water - origin[1]) / d[1];
                wet_length += if d[1] < 0. {
                    (stop - t.max(crossing)).max(0.)
                } else if d[1] > 0. {
                    (stop.min(crossing) - t).max(0.)
                } else if y < water {
                    stop - t
                } else {
                    0.
                };
            }
            if y < bed || hits_roof {
                let (m, n) = if y < bed {
                    (p.column.material((y / cell).floor() as i64), side)
                } else {
                    (p.column.ground_surface, [0., 1., 0.])
                };
                let light = self.light(n, 0.52, 0.88);
                return from_fn(|i| {
                    let transmission = (-[0.88, 0.43, 0.39][i] * wet_length).exp();
                    deep[i] + (f64::from(m.color()[i]) * light[i] - deep[i]) * transmission
                });
            }
            if end >= 16. {
                return deep;
            }
            for i in 0..2 {
                if next[i] == end {
                    let a = i * 2;
                    ij[i] += if d[a] > 0. { 1 } else { -1 };
                    side = [0.; 3];
                    side[a] = -d[a].signum();
                }
            }
            t = end;
        }
    }

    pub(super) fn shade(
        &mut self,
        scene: &Scene,
        hit: Hit,
        plant: Option<V>,
        origin: V,
        d: V,
        visibility: f64,
        random: [f64; 2],
        far: f64,
        shadow_distance: f64,
    ) -> V {
        let p = offset(origin, d, hit.distance);
        let face = normal(hit);
        let start = offset(p, face, 0.001 * voxel_size());
        let color = if material(hit.material) == Material::Water {
            let bed_origin = offset(p, face, -0.001 * voxel_size());
            let course = self.terrain.river_course(p[0], p[2]);
            let mut n = face;
            if face[1] > 0. {
                // Use the live river's four direction-weighted wave bands at
                // time zero. Course changes steer amplitudes at fixed phase.
                let q = [p[0], p[2]].map(|v| v.rem_euclid(128.) * std::f64::consts::TAU / 128.);
                let phase = |x: f64, z: f64| x * q[0] + z * q[1];
                let eddy = phase(6., -8.).sin() + 0.64 * phase(11., 3.).sin();
                let slope = if let Some((axis, grade)) = course {
                    let packet = 0.77 + 0.23 * (phase(13., -8.) + 0.8 * eddy).sin();
                    let gain = packet * (0.014 + 0.033 * (grade / 0.14).sqrt().min(1.));
                    let mut slope = [0.; 2];
                    for (x, z, warp) in [
                        (43., 9., 1.8),
                        (-11., 49., -1.1),
                        (57., 52., 0.9),
                        (64., -51., -1.6),
                    ] {
                        let length = f64::hypot(x, z);
                        let align = (x * axis[0] + z * axis[1]) / length;
                        let a = gain
                            * (0.12 + 0.88 * align * align)
                            * (phase(x, z) + warp * eddy).sin()
                            / length;
                        slope[0] += a * x;
                        slope[1] += a * z;
                    }
                    slope
                } else {
                    let warp = phase(6., -11.).sin() * 1.8;
                    let a = (phase(79., 35.) + warp).sin();
                    let b = (phase(-43., 100.) - 0.8 * warp).sin();
                    let c = (phase(147., -88.) + 1.2 * warp).sin();
                    [
                        0.02 * (a + 0.5 * b + 0.19 * c),
                        0.02 * (0.48 * a - b - 0.21 * c),
                    ]
                };
                n = normalized([slope[0], 1., slope[1]]);
                if dot(n, d) >= 0. {
                    n = face;
                }
            }
            let incidence = (-dot(n, d)).clamp(0., 1.);
            let mirror = offset(d, n, 2. * incidence);
            let above = if dot(mirror, face) <= 0. {
                self.sky(mirror)
            } else if let Some((other, plant)) = scene.ray(start, mirror, 0., far) {
                if material(other.material) == Material::Water {
                    self.sky(mirror)
                } else {
                    let n = normal(other);
                    let bounce = offset(
                        offset(start, mirror, other.distance),
                        n,
                        0.001 * voxel_size(),
                    );
                    let blocked = dot(n, self.sun) > 0.
                        && shadow_distance > 0.
                        && scene.ray(bounce, self.sun, 0., shadow_distance).is_some();
                    self.solid(other, plant, if blocked { 0. } else { 1. }, 0.9)
                }
            } else {
                self.sky(mirror)
            };
            let eta = 1. / 1.333;
            let transmitted = from_fn(|i| {
                eta * d[i]
                    + (eta * incidence - (1. - eta * eta * (1. - incidence * incidence)).sqrt())
                        * n[i]
            });
            let below = self.transmitted(bed_origin, transmitted);
            let fresnel = 0.0204 + 0.9796 * (1. - incidence).powi(5);
            let mut water = mix(below, above, fresnel);
            let h = dot(mirror, self.sun).max(0.);
            let glint = (0.17 * h.powi(95) + 0.37 * h.powi(420))
                * (1. - 0.9 * self.overcast)
                * visibility
                * (3. * fresnel).min(1.);
            for i in 0..3 {
                water[i] += glint * [1., 0.82, 0.57][i];
            }
            // Vary aeration on metre-scale gravel riffles according to the
            // canonical reach grade. Calm lakes and sea stay clear.
            if let Some((course, grade)) = course {
                let turbulence = ((grade - 0.035) / 0.11).clamp(0., 1.);
                let along = course[0] * p[0] + course[1] * p[2];
                let across = course[1] * p[0] - course[0] * p[2];
                let pool = noise(p[0] * 0.125, p[2] * 0.125);
                let warp = noise(along * 0.53, across * 0.65);
                let streak = noise(along * 0.5, across * 2.1 + warp * 1.9);
                let churn = noise(along * 2.5, across * 2.5);
                let bubbles = noise(p[0] * 7.5, p[2] * 7.5);
                let threshold = 0.79 - 0.36 * pool;
                let density =
                    (0.51 * streak + 0.31 * churn + 0.18 * bubbles - threshold) / 0.20 + 0.25;
                let t = density.clamp(0., 1.);
                let amount = turbulence * 0.74 * t * t * (3. - 2. * t);
                let light = self.light([0., 1., 0.], visibility, 1.);
                water = mix(water, from_fn(|i| [0.72, 0.83, 0.80][i] * light[i]), amount);
            }
            water
        } else {
            // Stratified cosine-hemisphere visibility softens the open sky
            // under ledges while sun shadows retain their voxel silhouette.
            let a = random[0].sqrt();
            let angle = random[1] * std::f64::consts::TAU;
            let t = if face[1].abs() > 0.5 {
                [1., 0., 0.]
            } else {
                [0., 1., 0.]
            };
            let b = cross(face, t);
            let ray = from_fn(|i| {
                t[i] * a * angle.cos() + b[i] * a * angle.sin() + face[i] * (1. - random[0]).sqrt()
            });
            let ambient = scene
                .ray(start, ray, 0., 32.)
                .map_or(1., |(h, _)| 0.45 + 0.55 * (h.distance / 32.).sqrt());
            self.solid(hit, plant, visibility, ambient)
        };
        let air = 1. - (-0.00011 * hit.distance).exp();
        mix(color, self.sky(d), air)
    }
}
