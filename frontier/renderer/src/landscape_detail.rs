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

struct Grid {
    source: Source,
    fine: ShadingGrid,
    ridges: Option<ShadingGrid>,
    rivers: Option<Rivers>,
}

impl Grid {
    fn make(source: Source, min: [i64; 2], previous: Option<&Grid>) -> (Self, Vec<u8>) {
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
        let rivers = (!source.blocks && source.params.recipe == FbmRecipe::Highlands)
            .then(|| Rivers::make(&terrain, center, reusable.and_then(|g| g.rivers.as_ref())));
        let area = SIDE * SIDE;
        let layers = if rivers.is_some() { 4 } else { 2 };
        let base_bytes = area * 4 * layers;
        let ridge_bytes = if ridges.is_some() {
            RIDGES.side * RIDGES.side * 8
        } else {
            0
        };
        let mut pixels = vec![0; base_bytes + ridge_bytes];
        let terrain_sky = std::env::var_os("FRONTIER_HIGHLANDS_SKYLIGHT").is_none_or(|v| v != "0");
        fine.encode(&mut pixels[..area * 8], terrain_sky);
        if let Some(grid) = &ridges {
            grid.encode(&mut pixels[base_bytes..], terrain_sky);
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
                fine,
                ridges,
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

#[derive(Default)]
pub(super) struct Detail {
    grid: Option<Arc<Grid>>,
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
        if self.landscape_detail.source != Some(source) {
            self.landscape_detail.source = Some(source);
            self.landscape_detail.grid = None;
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
                        && (0..2).all(|a| grid.fine.min[a].abs_diff(min[a]) <= SIDE as u64 / 3)
                    {
                        let layers = if grid.rivers.is_some() { 4 } else { 2 };
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
                            let len = (side * side * 4 * layers) as usize;
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
                                    depth_or_array_layers: layers,
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
            .is_some_and(|g| g.fine.min == min)
        {
            return;
        }
        let old = self.landscape_detail.grid.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.landscape_detail.pending = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(Grid::make(source, min, old.as_deref()));
        });
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
        if let Some((started, shift)) = self.landscape_detail.transition {
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
    pub(crate) fn settle_landscape_transition(&mut self) {
        self.landscape_detail.transition = None;
    }

    pub(crate) fn landscape_detail_pending(&self) -> bool {
        self.landscape_detail.source.is_some() && self.landscape_detail.pending.is_some()
    }
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
        let (old, old_bytes) = Grid::make(source, [-160, -192], None);
        let (moved, moved_bytes) = Grid::make(source, [-128, -224], Some(&old));
        let a = old.ridges.as_ref().unwrap();
        let b = moved.ridges.as_ref().unwrap();
        assert_eq!(a.min, [-66, -68]);
        assert_eq!(b.min, [-64, -70]);
        let base = SIDE * SIDE * 8;
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
        let (old, before) = Grid::make(source, [-64, -128], None);
        let (blocks, two_layers) = Grid::make(
            Source {
                blocks: true,
                ..source
            },
            old.fine.min,
            None,
        );
        assert!(blocks.rivers.is_none());
        let near_bytes = SIDE * SIDE * 8;
        assert_eq!(two_layers.len(), near_bytes + RIDGES.side * RIDGES.side * 8);
        assert_eq!(before[..near_bytes], two_layers[..near_bytes]);
        let (new, after) = Grid::make(source, [-64 + RECENTER, -128 - RECENTER], Some(&old));
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
