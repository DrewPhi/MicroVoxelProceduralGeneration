//! A world-anchored flight shading field. A moving window retains canonical
//! samples on the overlap and evaluates only newly revealed strips.
use frontier_world::{voxel_size, Cell, FbmParams, FbmRecipe, FbmTerrain, Material};
use std::sync::{
    mpsc::{self, Receiver, TryRecvError},
    Arc,
};

const SIDE: usize = 257;
const HALO: usize = 16;
#[cfg(test)]
const HALO_SIDE: usize = SIDE + 2 * HALO;
const STEP_M: f64 = 8.;
const RECENTER: i64 = 32;
const RIVER_SIDE: usize = SIDE * 2;
const RIVER_STEP: f64 = 16.;

// Each word stores the number of wet probes and their mean water elevation.
// The logarithmic height bin retains precision from lowlands to high plateaus.
// The lattice spans 8.2 km and shares its center with the fine grid.
struct Rivers {
    min: [i64; 2],
    texels: Vec<u16>,
}
impl Rivers {
    fn make(terrain: &FbmTerrain, center: [i64; 2], previous: Option<&Self>) -> Self {
        let min = center.map(|i| i.div_euclid(2) - (SIDE as i64 - 1));
        let candidates = river_candidates(terrain, min);
        let match_stage = std::env::var_os("FRONTIER_RIVER_STAGE").is_none_or(|v| v != "0");
        let texels = candidates
            .into_iter()
            .enumerate()
            .map(|(i, possible)| {
                if !possible {
                    return 0;
                }
                let x = min[0] + (i % RIVER_SIDE) as i64;
                let z = min[1] + (i / RIVER_SIDE) as i64;
                if let Some(old) = previous {
                    let (u, v) = (x - old.min[0], z - old.min[1]);
                    if (0..RIVER_SIDE as i64).contains(&u) && (0..RIVER_SIDE as i64).contains(&v) {
                        return old.texels[v as usize * RIVER_SIDE + u as usize];
                    }
                }
                let mut wet = 0u16;
                let mut stage_sum = 0.;
                for dz in [-6., -2., 2., 6.] {
                    for dx in [-6., -2., 2., 6.] {
                        let cell = Cell::from_meters([
                            x as f64 * RIVER_STEP + dx,
                            0.,
                            z as f64 * RIVER_STEP + dz,
                        ]);
                        let [bed, top] = terrain.column_tops(cell.x, cell.z);
                        if top > 0 && top > bed {
                            wet += 1;
                            stage_sum += top as f64 * voxel_size();
                        }
                    }
                }
                if wet == 0 {
                    return 0;
                }
                let stage = if match_stage {
                    ((1. + stage_sum / (64. * f64::from(wet))).log2() * 128.)
                        .floor()
                        .clamp(0., 2047.) as u16
                } else {
                    2047
                };
                wet | (stage << 5)
            })
            .collect();
        Self { min, texels }
    }
}

// The broad phase only skips texels beyond every possible inland wet core.
// Half a footprint and one native cell cover all sixteen canonical probes.
fn river_candidates(terrain: &FbmTerrain, min: [i64; 2]) -> Vec<bool> {
    let mut mask = vec![false; RIVER_SIDE * RIVER_SIDE];
    let pad = 0.5 * RIVER_STEP + voxel_size();
    let supported = 9600. * terrain.params().horizontal_scale / 1800.;
    let block = (((supported - 2. * pad) / RIVER_STEP).floor() as usize).clamp(1, RIVER_SIDE);
    for z in (0..RIVER_SIDE).step_by(block) {
        for x in (0..RIVER_SIDE).step_by(block) {
            let start = [x, z];
            let end = start.map(|i| (i + block).min(RIVER_SIDE));
            let lo = [0, 1].map(|a| (min[a] + start[a] as i64) as f64 * RIVER_STEP - pad);
            let hi = [0, 1].map(|a| (min[a] + end[a] as i64 - 1) as f64 * RIVER_STEP + pad);
            let boxes = terrain
                .inland_water_core_boxes(lo, hi)
                .unwrap_or_else(|| vec![[lo, hi]]);
            for [a, b] in boxes {
                let first = [0, 1].map(|i| {
                    (((a[i] - pad) / RIVER_STEP).ceil() as i64 - min[i])
                        .clamp(start[i] as i64, end[i] as i64) as usize
                });
                let past = [0, 1].map(|i| {
                    (((b[i] + pad) / RIVER_STEP).floor() as i64 + 1 - min[i])
                        .clamp(start[i] as i64, end[i] as i64) as usize
                });
                for row in first[1]..past[1] {
                    mask[row * RIVER_SIDE + first[0]..row * RIVER_SIDE + past[0]].fill(true);
                }
            }
        }
    }
    mask
}

#[derive(Clone, Copy, PartialEq)]
struct Source {
    seed: u64,
    params: FbmParams,
    edge: f64,
    vegetation: bool,
    blocks: bool,
}
#[derive(Clone, Copy, Default)]
struct Sample {
    height: f32,
    albedo: [u8; 4],
}
#[derive(Clone, Copy)]
struct Resolution {
    side: usize,
    halo: usize,
    step: f64,
    occluders: [isize; 3],
}
const NEAR: Resolution = Resolution {
    side: SIDE,
    halo: HALO,
    step: STEP_M,
    occluders: [2, 6, HALO as isize],
};
// The next texture mip spans 16 km in world space. Its material and normal
// come from this coarser lattice, independently of the fine window.
const RIDGES: Resolution = Resolution {
    side: SIDE / 2,
    halo: 4,
    step: STEP_M * 16.,
    occluders: [1, 2, 4],
};
// Valley walls and tributaries are narrower than a 128 m ridge sample.
// This lattice shares mip one with the ridge field in two additional layers.
const VALLEY: Resolution = Resolution {
    side: SIDE / 2,
    halo: 6,
    step: STEP_M * 4.,
    occluders: [1, 3, 6],
};

struct ShadingGrid {
    min: [i64; 2],
    resolution: Resolution,
    samples: Vec<Sample>,
}

impl ShadingGrid {
    fn get(&self, x: i64, z: i64) -> Option<Sample> {
        let r = self.resolution;
        let stride = r.side + 2 * r.halo;
        let u = x - self.min[0] + r.halo as i64;
        let v = z - self.min[1] + r.halo as i64;
        if (0..stride as i64).contains(&u) && (0..stride as i64).contains(&v) {
            Some(self.samples[v as usize * stride + u as usize])
        } else {
            None
        }
    }

    fn make(
        terrain: &FbmTerrain,
        source: Source,
        min: [i64; 2],
        r: Resolution,
        previous: Option<&Self>,
    ) -> Self {
        let stride = r.side + 2 * r.halo;
        let mut samples = Vec::with_capacity(stride * stride);
        for v in 0..stride {
            for u in 0..stride {
                let x = min[0] + u as i64 - r.halo as i64;
                let z = min[1] + v as i64 - r.halo as i64;
                let sample = previous.and_then(|g| g.get(x, z)).unwrap_or_else(|| {
                    let c = Cell::from_meters([x as f64 * r.step, 0., z as f64 * r.step]);
                    let (col, rgb) = terrain.landscape_column(c.x, c.z, source.vegetation);
                    let wet = col.top_material() == Material::Water;
                    let [r, g, b] = if wet { col.ground_surface.color() } else { rgb }.map(unorm);
                    let depth =
                        ((col.visible_top - col.ground_top) as f64 * source.edge).clamp(0., 12.);
                    let code = if wet {
                        (depth * (254. / 12.)).round() as u8
                    } else {
                        255
                    };
                    Sample {
                        height: (col.visible_top as f64 * source.edge) as f32,
                        albedo: [r, g, b, code],
                    }
                });
                samples.push(sample);
            }
        }
        Self {
            min,
            samples,
            resolution: r,
        }
    }

    fn encode(&self, pixels: &mut [u8], terrain_sky: bool) {
        let r = self.resolution;
        let stride = r.side + 2 * r.halo;
        let area = r.side * r.side;
        for v in 0..r.side {
            for u in 0..r.side {
                let j = (v + r.halo) * stride + u + r.halo;
                let n = glam::Vec3::new(
                    self.samples[j - 1].height - self.samples[j + 1].height,
                    2. * r.step as f32,
                    self.samples[j - stride].height - self.samples[j + stride].height,
                )
                .normalize();
                let at = (v * r.side + u) * 4;
                let oct = n / n.abs().element_sum();
                let sky = if terrain_sky {
                    unorm(sky_exposure(&self.samples, j, n, r))
                } else {
                    255
                };
                let albedo = self.samples[j].albedo;
                pixels[at..at + 4].copy_from_slice(&[
                    unorm(0.5 + 0.5 * oct.x),
                    unorm(0.5 + 0.5 * oct.z),
                    sky,
                    if albedo[3] == 255 { 255 } else { 0 },
                ]);
                pixels[4 * area + at..4 * area + at + 4].copy_from_slice(&albedo);
            }
        }
    }
}

/// The lowest ray height that clears the sampled ridges toward the sun.
/// Children inherit heights in world metres so a narrow summit can rise
/// above a shadow that covers the surrounding valley on the coarser grid.
struct SolarHorizon {
    min: [i64; 2],
    side: usize,
    step: f64,
    distant: Vec<f32>,
}

impl SolarHorizon {
    fn sample(&self, world: [f64; 2]) -> Option<f32> {
        let p: [f64; 2] = std::array::from_fn(|i| world[i] / self.step - self.min[i] as f64);
        if p.iter().any(|&x| x < 0. || x >= (self.side - 1) as f64) {
            return None;
        }
        let [x, z] = p.map(|v| v as usize);
        let [u, v] = p.map(|v| v.fract() as f32);
        let at = z * self.side + x;
        let h = &self.distant;
        Some(
            (h[at] * (1. - u) + h[at + 1] * u) * (1. - v)
                + (h[at + self.side] * (1. - u) + h[at + self.side + 1] * u) * v,
        )
    }

    fn survey(
        grid: &ShadingGrid,
        sun: [f32; 3],
        parent: Option<&Self>,
        image: &mut [u8],
        channel: usize,
    ) -> Self {
        let r = grid.resolution;
        let mut field = Self {
            min: grid.min,
            side: r.side,
            step: r.step,
            distant: vec![-1e8; r.side * r.side],
        };
        let horizontal = sun[0].hypot(sun[2]);
        let source_side = (r.side + 2 * r.halo) as isize;
        // Each address and bilinear weight is shared by the whole grid.
        let mut taps = Vec::new();
        if horizontal > 1e-4 && sun[1] > 0. {
            for k in 1..=40 {
                let d = k as f32;
                let x = d * sun[0] / horizontal;
                let z = d * sun[2] / horizontal;
                let u = x - x.floor();
                let v = z - z.floor();
                taps.push((
                    x.floor() as isize,
                    z.floor() as isize,
                    [(1. - u) * (1. - v), u * (1. - v), (1. - u) * v, u * v],
                    d * r.step as f32 * sun[1] / horizontal,
                    k >= 8,
                ));
            }
        }
        for z in 0..r.side {
            for x in 0..r.side {
                let world = [
                    (grid.min[0] + x as i64) as f64 * r.step,
                    (grid.min[1] + z as i64) as f64 * r.step,
                ];
                let mut distant = parent.and_then(|p| p.sample(world)).unwrap_or(-1e8);
                let mut horizon = distant;
                let sx = (x + r.halo) as isize;
                let sz = (z + r.halo) as isize;
                for &(dx, dz, w, rise, far) in &taps {
                    let (u, v) = (sx + dx, sz + dz);
                    if u < 0 || v < 0 || u + 1 >= source_side || v + 1 >= source_side {
                        break;
                    }
                    let at = (v * source_side + u) as usize;
                    let next = at + source_side as usize;
                    let a = &grid.samples;
                    let height = a[at].height * w[0]
                        + a[at + 1].height * w[1]
                        + a[next].height * w[2]
                        + a[next + 1].height * w[3]
                        - rise;
                    horizon = horizon.max(height);
                    if far {
                        distant = distant.max(height);
                    }
                }
                field.distant[z * r.side + x] = distant;
                let height = grid.samples[(sz * source_side + sx) as usize].height;
                // Source gradients between samples leave uncertainty near a
                // crest. Fade across that interval instead of stamping a cell.
                let radius = (r.step as f32 * 0.16).max(0.125);
                let t = (0.5 + 0.5 * (height - horizon) / radius).clamp(0., 1.);
                image[4 * (z * SIDE + x) + channel] = unorm(t * t * (3. - 2. * t));
            }
        }
        field
    }
}

struct Grid {
    source: Source,
    sun: [f32; 3],
    fine: ShadingGrid,
    ridges: Option<ShadingGrid>,
    valley: Option<ShadingGrid>,
    rivers: Option<Rivers>,
}

impl Grid {
    fn make(
        source: Source,
        min: [i64; 2],
        sun: [f32; 3],
        previous: Option<&Grid>,
    ) -> (Self, Vec<u8>) {
        let terrain =
            FbmTerrain::new(source.seed, source.params).expect("validated flight terrain");
        let reusable = previous.filter(|g| g.source == source);
        let fine = ShadingGrid::make(&terrain, source, min, NEAR, reusable.map(|g| &g.fine));
        let center = min.map(|i| i + SIDE as i64 / 2);
        let ridges = source.blocks.then(|| {
            let min = center.map(|v| v.div_euclid(16) - RIDGES.side as i64 / 2);
            ShadingGrid::make(
                &terrain,
                source,
                min,
                RIDGES,
                reusable.and_then(|g| g.ridges.as_ref()),
            )
        });
        let valley = source.blocks.then(|| {
            let min = center.map(|v| v.div_euclid(4) - VALLEY.side as i64 / 2);
            ShadingGrid::make(
                &terrain,
                source,
                min,
                VALLEY,
                reusable.and_then(|g| g.valley.as_ref()),
            )
        });
        let rivers = (!source.blocks && source.params.recipe == FbmRecipe::Highlands)
            .then(|| Rivers::make(&terrain, center, reusable.and_then(|g| g.rivers.as_ref())));
        let area = SIDE * SIDE;
        let layers = if ridges.is_some() {
            3
        } else if rivers.is_some() {
            4
        } else {
            2
        };
        let base_bytes = area * 4 * layers;
        let ridge_bytes = if ridges.is_some() {
            RIDGES.side * RIDGES.side * 16
        } else {
            0
        };
        let mut pixels = vec![0; base_bytes + ridge_bytes];
        let terrain_sky = std::env::var_os("FRONTIER_HIGHLANDS_SKYLIGHT").is_none_or(|v| v != "0");
        fine.encode(&mut pixels[..area * 8], terrain_sky);
        if let Some(grid) = &ridges {
            grid.encode(&mut pixels[base_bytes..], terrain_sky);
        }
        if let Some(grid) = &valley {
            grid.encode(
                &mut pixels[base_bytes + RIDGES.side * RIDGES.side * 8..],
                terrain_sky,
            );
        }
        // The third base layer contains the three solar surveys. The two
        // local layers are uploaded separately when their own window moves.
        if let (Some(ridges), Some(valley)) = (&ridges, &valley) {
            let data = &mut pixels[8 * area..12 * area];
            let broad = SolarHorizon::survey(ridges, sun, None, data, 2);
            let middle = SolarHorizon::survey(valley, sun, Some(&broad), data, 1);
            SolarHorizon::survey(&fine, sun, Some(&middle), data, 0);
        }
        if let Some(rivers) = &rivers {
            for v in 0..SIDE {
                for u in 0..SIDE {
                    let at = (v * SIDE + u) * 4;
                    for k in 0..4 {
                        let [lo, hi] = rivers.texels[(2 * v + k / 2) * RIVER_SIDE + 2 * u + k % 2]
                            .to_le_bytes();
                        pixels[8 * area + at + k] = lo;
                        pixels[12 * area + at + k] = hi;
                    }
                }
            }
        }
        (
            Self {
                source,
                sun,
                fine,
                ridges,
                valley,
                rivers,
            },
            pixels,
        )
    }
}
// Angular fraction of diffuse sky hidden by the surrounding terrain, relative
// to the sample's own tangent plane. A planar face already receives the
// renderer's orientation-dependent sky light, so it gets no extra darkening.
fn sky_exposure(samples: &[Sample], center: usize, n: glam::Vec3, r: Resolution) -> f32 {
    let h = samples[center].height;
    let mut blocked = 0.;
    for (dx, dz) in [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ] {
        let stride = dx + dz * (r.side + 2 * r.halo) as isize;
        let length = if dx == 0 || dz == 0 {
            1.
        } else {
            std::f32::consts::SQRT_2
        };
        let tangent = (-(n.x * dx as f32 + n.z * dz as f32) / (n.y * length)).max(0.);
        let mut horizon = tangent;
        for distance in r.occluders {
            let neighbor = (center as isize + distance * stride) as usize;
            let slope = (samples[neighbor].height - h) / (distance as f32 * r.step as f32 * length);
            horizon = horizon.max(slope);
        }
        let sky_blocked = |slope: f32| slope * slope / (1. + slope * slope);
        blocked += sky_blocked(horizon) - sky_blocked(tangent);
    }
    1. - blocked / 8.
}

fn unorm(value: f32) -> u8 {
    (value.clamp(0., 1.) * 255.).round() as u8
}

pub(crate) struct Texture {
    texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) sampler: wgpu::Sampler,
}
impl Texture {
    pub(super) fn bytes(&self) -> u64 {
        let layers = u64::from(self.texture.depth_or_array_layers());
        (0..self.texture.mip_level_count())
            .map(|mip| {
                u64::from(self.texture.width() >> mip)
                    * u64::from(self.texture.height() >> mip)
                    * layers
                    * 4
            })
            .sum()
    }
    pub(crate) fn new(device: &wgpu::Device, side: u32, layers: u32, mips: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flight terrain normals and albedo"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: layers,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            texture,
            view,
            sampler,
        }
    }
}

// One-metre source elevations resolve narrow folds just beyond the exact
// volume. The ordinary block texture reserves its fourth base layer for this
// independent 257 m moving window; a shift only evaluates newly exposed rows.
struct LocalRelief {
    source: Source,
    min: [i64; 2],
    spacing: f64,
    grid: ShadingGrid,
    sun: [f32; 3],
    base: f32,
    unit: f32,
    pixels: Vec<u8>,
}
// At altitude the detailed terrain lies ahead of the aircraft. Survey a ray
// through the lower-middle viewport and place the small source window there.
fn local_target(
    terrain: &FbmTerrain,
    camera: crate::Camera,
    ground: f32,
) -> Option<([i64; 2], f64)> {
    let eye = camera.position;
    let mut center = [eye.x, eye.z];
    let mut spacing = 1.;
    let clearance = eye.y - f64::from(ground);
    let telephoto = camera.fov_y < 32.;
    if clearance <= 12. && !telephoto {
        // Keep the exact ownership radius inside the survey while bringing
        // the opposite bank into its finer half-metre samples.
        let direction = camera.direction();
        spacing = 0.5;
        center = [
            eye.x + 32. * f64::from(direction.x),
            eye.z + 32. * f64::from(direction.z),
        ];
    }
    if clearance > 95. || telephoto {
        let forward = camera.direction().as_dvec3();
        let right = forward
            .cross(camera.view_up().as_dvec3())
            .normalize_or_zero();
        let down = forward.cross(right);
        let ray = (forward + down * (0.28 * (0.5 * f64::from(camera.fov_y).to_radians()).tan()))
            .normalize();
        let mut last = 0.;
        let mut t = 32.;
        let limit = f64::from(camera.far).min(5000.);
        loop {
            let p = eye + t * ray;
            if p.y <= terrain.height_m(p.x, p.z) {
                for _ in 0..6 {
                    let mid = 0.5 * (last + t);
                    let p = eye + mid * ray;
                    if p.y > terrain.height_m(p.x, p.z) {
                        last = mid;
                    } else {
                        t = mid;
                    }
                }
                let p = eye + t * ray;
                center = [p.x, p.z];
                spacing = if telephoto && t < 100. {
                    0.25
                } else if t < 180. {
                    0.5
                } else if t < 420. {
                    1.
                } else if t < 1350. {
                    2.
                } else {
                    4.
                };
                break;
            }
            if t >= limit {
                return None;
            }
            last = t;
            t = (t + (0.12 * t).clamp(32., 160.)).min(limit);
        }
    }
    let min = center.map(|v| (v / (spacing * 32.)).round() as i64 * 32 - SIDE as i64 / 2);
    Some((min, spacing))
}
impl LocalRelief {
    fn make(
        source: Source,
        min: [i64; 2],
        spacing: f64,
        sun: [f32; 3],
        old: Option<&Self>,
    ) -> Self {
        const STRIDE: usize = SIDE + 2;
        let terrain = FbmTerrain::new(source.seed, source.params).expect("validated local terrain");
        let old = old.filter(|g| g.source == source && g.spacing == spacing);
        let resolution = Resolution {
            side: SIDE,
            halo: 1,
            step: spacing,
            occluders: [1; 3],
        };
        let grid = ShadingGrid::make(&terrain, source, min, resolution, old.map(|g| &g.grid));
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for sample in &grid.samples {
            lo = lo.min(sample.height);
            hi = hi.max(sample.height);
        }
        let unit = ((hi - lo) / 65535.).max(source.edge as f32);
        let layer = SIDE * SIDE * 4;
        let mut pixels = vec![0; 2 * layer];
        for z in 0..SIDE {
            for x in 0..SIDE {
                let i = (z + 1) * STRIDE + x + 1;
                let n = glam::Vec3::new(
                    grid.samples[i - 1].height - grid.samples[i + 1].height,
                    2. * spacing as f32,
                    grid.samples[i - STRIDE].height - grid.samples[i + STRIDE].height,
                );
                let oct = n / n.abs().element_sum();
                let word = ((grid.samples[i].height - lo) / unit)
                    .round()
                    .clamp(0., 65535.) as u16;
                let [high, low] = word.to_be_bytes();
                let at = 4 * (z * SIDE + x);
                pixels[layer + at..layer + at + 3].copy_from_slice(&grid.samples[i].albedo[..3]);
                pixels[at..at + 4].copy_from_slice(&[
                    unorm(0.5 + 0.5 * oct.x),
                    unorm(0.5 + 0.5 * oct.z),
                    high,
                    low,
                ]);
            }
        }
        SolarHorizon::survey(&grid, sun, None, &mut pixels[layer..], 3);
        Self {
            source,
            min,
            spacing,
            grid,
            sun,
            base: lo,
            unit,
            pixels,
        }
    }
    fn upload(&self, queue: &wgpu::Queue, texture: &wgpu::Texture) {
        let mut dst = texture.as_image_copy();
        dst.origin.z = 3;
        queue.write_texture(
            dst,
            &self.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIDE as u32 * 4),
                rows_per_image: Some(SIDE as u32),
            },
            wgpu::Extent3d {
                width: SIDE as u32,
                height: SIDE as u32,
                depth_or_array_layers: 2,
            },
        );
    }
}

#[derive(Default)]
pub(super) struct Detail {
    grid: Option<Arc<Grid>>,
    local: Option<Arc<LocalRelief>>,
    pending_local: Option<Receiver<LocalRelief>>,
    local_aim: Option<(crate::Camera, Option<([i64; 2], f64)>)>,
    local_transition: Option<(std::time::Instant, Option<[i64; 2]>)>,
    pending: Option<Receiver<(Grid, Vec<u8>)>>,
    source: Option<Source>,
    transition: Option<(std::time::Instant, Option<[i64; 2]>)>,
}

impl super::AdaptiveGpu {
    pub(crate) fn update_landscape_detail(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &frontier_world::World,
        camera: crate::Camera,
        enabled: bool,
    ) {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let allowed = enabled
            && *ENABLED.get_or_init(|| {
                std::env::var_os("FRONTIER_FLIGHT_DETAIL").is_none_or(|v| v != "0")
            });
        let Some(terrain) = world
            .fbm_terrain()
            .filter(|t| allowed && t.params().recipe.uses_watersheds())
        else {
            self.landscape_detail = Detail::default();
            return;
        };
        // Block rings carry explicit wet faces and their native bed depths.
        // Only the surface proxy needs the additional river coverage layers.
        use crate::adaptive_geometry::distant_voxels::Mode;
        let source = Source {
            blocks: Mode::current() == Mode::Blocks,
            seed: terrain.seed(),
            params: terrain.params(),
            edge: voxel_size(),
            vegetation: world.vegetation_enabled(),
        };
        let sun = [self.lighting[0], self.lighting[1], self.lighting[2]];
        if self.landscape_detail.source != Some(source) {
            self.landscape_detail.source = Some(source);
            self.landscape_detail.grid = None;
            self.landscape_detail.local = None;
            self.landscape_detail.pending_local = None;
            self.landscape_detail.local_aim = None;
        }
        let center = [camera.position.x, camera.position.z]
            .map(|v| (v / (STEP_M * RECENTER as f64)).round() as i64 * RECENTER);
        let min = center.map(|v| v - SIDE as i64 / 2);
        if let Some(rx) = &self.landscape_detail.pending {
            match rx.try_recv() {
                Ok((grid, data)) => {
                    self.landscape_detail.pending = None;
                    // Retain alignment while flying. Obsolete or teleported
                    // patches never cover the current eye by coincidence.
                    if grid.source == source
                        && grid.sun == sun
                        && (0..2).all(|a| grid.fine.min[a].abs_diff(min[a]) <= SIDE as u64 / 3)
                    {
                        let layers = if grid.ridges.is_some() {
                            5
                        } else if grid.rivers.is_some() {
                            4
                        } else {
                            2
                        };
                        let texture = &self.shadows.near.detail.texture;
                        let mips = 1 + u32::from(grid.ridges.is_some());
                        if texture.width() != SIDE as u32
                            || texture.depth_or_array_layers() != layers
                            || texture.mip_level_count() != mips
                        {
                            self.shadows.near.detail =
                                Texture::new(device, SIDE as u32, layers, mips);
                            self.refresh_visibility_bind(device);
                        }
                        let texture = &self.shadows.near.detail.texture;
                        let mut offset = 0;
                        for mip in 0..mips {
                            let side = SIDE as u32 >> mip;
                            // Local pigment and relief use their own moving window.
                            let copied_layers = if grid.ridges.is_some() {
                                3 + mip
                            } else {
                                layers
                            };
                            let len = (side * side * 4 * copied_layers) as usize;
                            let mut target = texture.as_image_copy();
                            target.mip_level = mip;
                            queue.write_texture(
                                target,
                                &data[offset..offset + len],
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(side * 4),
                                    rows_per_image: Some(side),
                                },
                                wgpu::Extent3d {
                                    width: side,
                                    height: side,
                                    depth_or_array_layers: copied_layers,
                                },
                            );
                            offset += len;
                        }
                        let shift = self
                            .landscape_detail
                            .grid
                            .as_ref()
                            .map(|old| [0, 1].map(|a| grid.fine.min[a] - old.fine.min[a]));
                        self.landscape_detail.transition = Some((std::time::Instant::now(), shift));
                        self.landscape_detail.grid = Some(Arc::new(grid));
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.landscape_detail.pending = None,
            }
        }
        if self
            .landscape_detail
            .grid
            .as_ref()
            .is_some_and(|g| g.fine.min == min && g.sun == sun)
        {
            return;
        }
        let old = self.landscape_detail.grid.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.landscape_detail.pending = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(Grid::make(source, min, sun, old.as_deref()));
        });
    }
    pub(crate) fn update_local_relief(
        &mut self,
        queue: &wgpu::Queue,
        terrain: &FbmTerrain,
        camera: crate::Camera,
        ground: f32,
    ) {
        let Some(source) = self.landscape_detail.grid.as_ref().map(|g| g.source) else {
            return;
        };
        if !source.blocks {
            return;
        }
        let sun = [self.lighting[0], self.lighting[1], self.lighting[2]];
        let detail = &mut self.landscape_detail;
        if detail.local_aim.is_none_or(|(c, _)| {
            c.position.distance_squared(camera.position) > 16. * 16.
                || c.direction().dot(camera.direction()) < 0.998
                || (c.roll - camera.roll).abs() > 0.08
                || c.fov_y != camera.fov_y
                || c.far != camera.far
        }) {
            detail.local_aim = Some((camera, local_target(terrain, camera, ground)));
        }
        let target = detail.local_aim.and_then(|(_, aim)| aim);
        if let Some(rx) = &detail.pending_local {
            match rx.try_recv() {
                Ok(local) => {
                    detail.pending_local = None;
                    if local.source == source
                        && local.sun == sun
                        && target.is_some_and(|(min, step)| {
                            step == local.spacing
                                && (0..2).all(|a| local.min[a].abs_diff(min[a]) <= 64)
                        })
                    {
                        local.upload(queue, &self.shadows.near.detail.texture);
                        let shift = detail
                            .local
                            .as_ref()
                            .filter(|g| {
                                g.spacing == local.spacing
                                    && (0..2).all(|a| local.min[a].abs_diff(g.min[a]) < SIDE as u64)
                            })
                            .map(|g| [0, 1].map(|a| local.min[a] - g.min[a]));
                        detail.local_transition = Some((std::time::Instant::now(), shift));
                        detail.local = Some(Arc::new(local));
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => detail.pending_local = None,
            }
        }
        let Some((min, spacing)) = target else {
            return;
        };
        if detail
            .local
            .as_ref()
            .is_some_and(|g| g.min == min && g.spacing == spacing && g.sun == sun)
        {
            return;
        }
        let old = detail.local.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        detail.pending_local = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(LocalRelief::make(source, min, spacing, sun, old.as_deref()));
        });
    }
    pub(super) fn local_inverse_spacing(&self) -> f32 {
        self.landscape_detail
            .local
            .as_ref()
            .map_or(0., |g| (1. / g.spacing) as f32)
    }
    pub(super) fn local_relief_params(&self, eye: [f64; 3]) -> [f32; 8] {
        let Some(g) = self
            .landscape_detail
            .local
            .as_ref()
            .filter(|g| g.sun == self.lighting[..3])
        else {
            return [0.; 8];
        };
        let [sx, sz, t, w] = transition(self.landscape_detail.local_transition);
        [
            (eye[0] / g.spacing - g.min[0] as f64) as f32,
            (eye[2] / g.spacing - g.min[1] as f64) as f32,
            (f64::from(g.base) - eye[1]) as f32,
            g.unit,
            sx,
            sz,
            t,
            w,
        ]
    }

    pub(super) fn landscape_sun_ready(&self) -> bool {
        self.landscape_detail
            .grid
            .as_ref()
            .is_some_and(|g| g.ridges.is_some() && g.sun == self.lighting[..3])
    }
    pub(super) fn landscape_offset(&self, eye: [f64; 3]) -> [f32; 2] {
        self.landscape_detail.grid.as_ref().map_or([-1e20; 2], |g| {
            [
                (eye[0] - g.fine.min[0] as f64 * STEP_M) as f32,
                (eye[2] - g.fine.min[1] as f64 * STEP_M) as f32,
            ]
        })
    }
    pub(super) fn landscape_transition(&self) -> [f32; 4] {
        transition(self.landscape_detail.transition)
    }
    pub(crate) fn settle_landscape_transition(&mut self) {
        self.landscape_detail.transition = None;
        self.landscape_detail.local_transition = None;
    }

    pub(crate) fn landscape_detail_pending(&self) -> bool {
        self.landscape_detail.source.is_some()
            && (self.landscape_detail.pending.is_some()
                || self.landscape_detail.pending_local.is_some())
    }
}

fn transition(value: Option<(std::time::Instant, Option<[i64; 2]>)>) -> [f32; 4] {
    if let Some((started, shift)) = value {
        let t = (started.elapsed().as_secs_f32() / 0.45).min(1.);
        let offset = shift.unwrap_or([0; 2]);
        return [
            offset[0] as f32,
            offset[1] as f32,
            t * t * (3. - 2. * t),
            f32::from(shift.is_some()),
        ];
    }
    [0., 0., 1., 0.]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn far_river_coverage_matches_brute_force_native_probes_in_a_drainage_basin() {
        let terrain = FbmTerrain::new(
            19,
            FbmParams {
                recipe: FbmRecipe::Highlands,
                ..Default::default()
            },
        )
        .unwrap();
        let center = [800, 800];
        let rivers = Rivers::make(&terrain, center, None);
        let mut mixed = 0;
        for z in (160..350).step_by(3) {
            for x in (160..350).step_by(3) {
                let mut wet = 0;
                let mut height = 0.;
                for dz in [-6., -2., 2., 6.] {
                    for dx in [-6., -2., 2., 6.] {
                        let p = Cell::from_meters([
                            (rivers.min[0] + x as i64) as f64 * 16. + dx,
                            0.,
                            (rivers.min[1] + z as i64) as f64 * 16. + dz,
                        ]);
                        let c = terrain.column_cells(p.x, p.z);
                        if c.visible_top > 0 && c.visible_top > c.ground_top {
                            wet += 1;
                            height += c.visible_top as f64 * voxel_size();
                        }
                    }
                }
                let word = rivers.texels[z * RIVER_SIDE + x];
                assert_eq!(u32::from(word & 31), wet);
                if wet > 0 && word >> 5 < 2047 {
                    let lo = f64::from(word >> 5);
                    let avg = height / f64::from(wet);
                    assert!(
                        avg >= 64. * (2f64.powf(lo / 128.) - 1.) - 1e-9
                            && avg <= 64. * (2f64.powf((lo + 1.) / 128.) - 1.) + 1e-9
                    );
                }
                mixed += u32::from(wet > 0 && wet < 16);
            }
        }
        assert!(mixed > 10);
    }

    #[test]
    fn skylight_tracks_the_horizon_and_leaves_planar_slopes_unchanged() {
        let center = HALO * HALO_SIDE + HALO;
        let field = |f: fn(f32, f32) -> f32| {
            (0..HALO_SIDE * (2 * HALO + 1))
                .map(|i| {
                    let x = (i % HALO_SIDE) as f32 - HALO as f32;
                    let z = (i / HALO_SIDE) as f32 - HALO as f32;
                    Sample {
                        height: f(x * STEP_M as f32, z * STEP_M as f32),
                        albedo: [0; 4],
                    }
                })
                .collect::<Vec<_>>()
        };
        let plane = field(|x, z| 0.7 * x - 0.3 * z);
        assert!(
            (sky_exposure(
                &plane,
                center,
                glam::Vec3::new(-0.7, 1., 0.3).normalize(),
                NEAR
            ) - 1.)
                .abs()
                < 1e-5
        );
        let bowl = field(|x, z| x.hypot(z));
        assert!((sky_exposure(&bowl, center, glam::Vec3::Y, NEAR) - 0.5).abs() < 1e-5);
        let dome = field(|x, z| -x.hypot(z));
        assert_eq!(sky_exposure(&dome, center, glam::Vec3::Y, NEAR), 1.);
    }

    #[test]
    fn sun_survey_shadows_a_valley_but_clears_a_peak_rising_above_the_same_ray() {
        let field = |r: Resolution, height: fn(f64, f64) -> f64| {
            let stride = r.side + 2 * r.halo;
            let min = [-(r.side as i64 / 2); 2];
            let samples = (0..stride * stride)
                .map(|i| {
                    let x = (min[0] + (i % stride) as i64 - r.halo as i64) as f64 * r.step;
                    let z = (min[1] + (i / stride) as i64 - r.halo as i64) as f64 * r.step;
                    Sample {
                        height: height(x, z) as f32,
                        ..Default::default()
                    }
                })
                .collect();
            ShadingGrid {
                min,
                resolution: r,
                samples,
            }
        };
        let coarse = field(VALLEY, |x, _| if x == 320. { 200. } else { 0. });
        let fine = field(NEAR, |x, z| if x == 0. && z == 0. { 300. } else { 0. });
        let mut bytes = vec![0; SIDE * SIDE * 4];
        let sun = [1., 0.25, 0.];
        let parent = SolarHorizon::survey(&coarse, sun, None, &mut bytes, 1);
        SolarHorizon::survey(&fine, sun, Some(&parent), &mut bytes, 0);
        let at =
            |x: i64, z: i64| 4 * ((z - fine.min[1]) as usize * SIDE + (x - fine.min[0]) as usize);
        assert_eq!(bytes[at(0, 0)], 255);
        assert_eq!(bytes[at(0, 2)], 0);
        assert_eq!(bytes[at(52, 2)], 255);
        SolarHorizon::survey(&fine, [0., 1., 0.], None, &mut bytes, 0);
        assert_eq!(bytes[at(0, 2)], 255);
    }

    #[test]
    fn local_relief_preserves_native_elevations_and_shared_normals_after_a_shift() {
        let source = Source {
            blocks: true,
            seed: 997,
            params: FbmParams {
                recipe: FbmRecipe::Watershed,
                ..Default::default()
            },
            edge: voxel_size(),
            vegetation: false,
        };
        let terrain = FbmTerrain::new(source.seed, source.params).unwrap();
        for spacing in [0.25, 0.5, 1., 4.] {
            let a = LocalRelief::make(source, [-1816, 2016], spacing, [0., 1., 0.], None);
            let b = LocalRelief::make(source, [-1784, 1984], spacing, [0., 1., 0.], Some(&a));
            for z in (32..SIDE).step_by(7) {
                for x in (0..SIDE - 32).step_by(11) {
                    let old = 4 * ((z - 32) * SIDE + x + 32);
                    let new = 4 * (z * SIDE + x);
                    assert_eq!(a.pixels[old..old + 2], b.pixels[new..new + 2]);
                    let encoded = u16::from_be_bytes([b.pixels[new + 2], b.pixels[new + 3]]);
                    let h = terrain.height_m(
                        (b.min[0] + x as i64) as f64 * spacing,
                        (b.min[1] + z as i64) as f64 * spacing,
                    );
                    assert!(
                        (f64::from(b.base + f32::from(encoded) * b.unit) - h).abs()
                            <= f64::from(b.unit) * 0.55
                    );
                }
            }
        }
    }

    #[test]
    fn ridge_lighting_stays_world_aligned_across_a_negative_window_shift() {
        let source = Source {
            blocks: true,
            seed: 673,
            params: FbmParams {
                recipe: FbmRecipe::Highlands,
                ..Default::default()
            },
            edge: voxel_size(),
            vegetation: true,
        };
        let (old, old_bytes) = Grid::make(source, [-160, -192], [0., 1., 0.], None);
        let (moved, moved_bytes) = Grid::make(source, [-128, -224], [0., 1., 0.], Some(&old));
        let a = old.ridges.as_ref().unwrap();
        let b = moved.ridges.as_ref().unwrap();
        assert_eq!(a.min, [-66, -68]);
        assert_eq!(b.min, [-64, -70]);
        let base = SIDE * SIDE * 12;
        let area = RIDGES.side * RIDGES.side * 4;
        for v in 2..RIDGES.side {
            for u in 0..RIDGES.side - 2 {
                let i = 4 * ((v - 2) * RIDGES.side + u + 2);
                let j = 4 * (v * RIDGES.side + u);
                for layer in [base, base + area] {
                    assert_eq!(
                        old_bytes[layer + i..layer + i + 4],
                        moved_bytes[layer + j..layer + j + 4]
                    );
                }
            }
        }
    }

    #[test]
    fn moving_window_preserves_shared_height_normal_and_albedo() {
        let source = Source {
            blocks: false,
            seed: 40961,
            params: FbmParams {
                recipe: FbmRecipe::Highlands,
                ..Default::default()
            },
            edge: voxel_size(),
            vegetation: true,
        };
        let (old, before) = Grid::make(source, [-64, -128], [0., 1., 0.], None);
        let (blocks, block_upload) = Grid::make(
            Source {
                blocks: true,
                ..source
            },
            old.fine.min,
            [0., 1., 0.],
            None,
        );
        assert!(blocks.rivers.is_none());
        let near_bytes = SIDE * SIDE * 8;
        assert_eq!(
            block_upload.len(),
            SIDE * SIDE * 12 + RIDGES.side * RIDGES.side * 16
        );
        assert_eq!(before[..near_bytes], block_upload[..near_bytes]);
        let (new, after) = Grid::make(
            source,
            [-64 + RECENTER, -128 - RECENTER],
            [0., 1., 0.],
            Some(&old),
        );
        let shift = RECENTER as usize;
        let area = SIDE * SIDE * 4;
        let river_shift = shift / 2;
        for v in river_shift..RIVER_SIDE {
            for u in 0..RIVER_SIDE - river_shift {
                assert_eq!(
                    old.rivers.as_ref().unwrap().texels
                        [(v - river_shift) * RIVER_SIDE + u + river_shift],
                    new.rivers.as_ref().unwrap().texels[v * RIVER_SIDE + u]
                );
            }
        }
        for v in shift..SIDE {
            for u in 0..SIDE - shift {
                let i = ((v - shift) * SIDE + u + shift) * 4;
                let j = (v * SIDE + u) * 4;
                for layer in [0, area] {
                    assert_eq!(
                        &before[layer + i..layer + i + 4],
                        &after[layer + j..layer + j + 4]
                    );
                }
                let x = new.fine.min[0] + u as i64;
                let z = new.fine.min[1] + v as i64;
                assert_eq!(after[area + j + 3], new.fine.get(x, z).unwrap().albedo[3]);
                assert_eq!(
                    new.fine.get(x, z).unwrap().height.to_bits(),
                    old.fine.get(x, z).unwrap().height.to_bits()
                );
            }
        }
    }
}
