//! A world-anchored flight shading field. A moving window retains canonical
//! samples on the overlap and evaluates only newly revealed strips.
use frontier_world::{voxel_size, Cell, FbmParams, FbmRecipe, FbmTerrain, Material};
use std::sync::{
    mpsc::{self, Receiver, TryRecvError},
    Arc,
};

const SIDE: usize = 257;
const HALO: usize = 16;
const HALO_SIDE: usize = SIDE + 2 * HALO;
const STEP_M: f64 = 8.;
const RECENTER: i64 = 32;
const RIVER_SIDE: usize = SIDE * 2;
const RIVER_STEP: f64 = 16.;

// Each byte stores the fraction of sixteen native column probes that contain
// inland water. The lattice spans 8.2 km and shares its center with the fine grid.
struct Rivers {
    min: [i64; 2],
    cover: Vec<u8>,
}
impl Rivers {
    fn make(terrain: &FbmTerrain, center: [i64; 2], previous: Option<&Self>) -> Self {
        let min = center.map(|i| i.div_euclid(2) - (SIDE as i64 - 1));
        let candidates = river_candidates(terrain, min);
        let cover = candidates
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
                        return old.cover[v as usize * RIVER_SIDE + u as usize];
                    }
                }
                let mut wet = 0;
                for dz in [-6., -2., 2., 6.] {
                    for dx in [-6., -2., 2., 6.] {
                        let cell = Cell::from_meters([
                            x as f64 * RIVER_STEP + dx,
                            0.,
                            z as f64 * RIVER_STEP + dz,
                        ]);
                        let col = terrain.column_cells(cell.x, cell.z);
                        wet += u32::from(col.visible_top > 0 && col.visible_top > col.ground_top);
                    }
                }
                ((wet * 255 + 8) / 16) as u8
            })
            .collect();
        Self { min, cover }
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
}
#[derive(Clone, Copy, Default)]
struct Sample {
    height: f32,
    albedo: [u8; 4],
}
struct Grid {
    source: Source,
    // Lattice address of the first *visible* texel; the CPU grid has a halo.
    min: [i64; 2],
    samples: Vec<Sample>,
    rivers: Rivers,
}
impl Grid {
    fn get(&self, x: i64, z: i64) -> Option<Sample> {
        let u = x - self.min[0] + HALO as i64;
        let v = z - self.min[1] + HALO as i64;
        if (0..HALO_SIDE as i64).contains(&u) && (0..HALO_SIDE as i64).contains(&v) {
            Some(self.samples[v as usize * HALO_SIDE + u as usize])
        } else {
            None
        }
    }
    fn make(source: Source, min: [i64; 2], previous: Option<&Grid>) -> (Self, Vec<u8>) {
        let terrain =
            FbmTerrain::new(source.seed, source.params).expect("validated flight terrain");
        let reusable = previous.filter(|g| g.source == source);
        let mut samples = Vec::with_capacity(HALO_SIDE * HALO_SIDE);
        for v in 0..HALO_SIDE {
            for u in 0..HALO_SIDE {
                let x = min[0] + u as i64 - HALO as i64;
                let z = min[1] + v as i64 - HALO as i64;
                let sample = reusable.and_then(|g| g.get(x, z)).unwrap_or_else(|| {
                    let c = Cell::from_meters([x as f64 * STEP_M, 0., z as f64 * STEP_M]);
                    let (col, rgb) = terrain.landscape_column(c.x, c.z, source.vegetation);
                    let wet = col.top_material() == Material::Water;
                    let [r, g, b] = if wet { col.ground_surface.color() } else { rgb }.map(unorm);
                    let depth =
                        ((col.visible_top - col.ground_top) as f64 * source.edge).clamp(0., 12.);
                    // Keep 255 for dry land. Both near shading layers can be
                    // constructed from the same compact canonical sample.
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
        let rivers = Rivers::make(
            &terrain,
            min.map(|i| i + SIDE as i64 / 2),
            reusable.map(|g| &g.rivers),
        );
        let area = SIDE * SIDE;
        let mut pixels = vec![0; area * 12];
        let terrain_sky = std::env::var_os("FRONTIER_HIGHLANDS_SKYLIGHT").is_none_or(|v| v != "0");
        for v in 0..SIDE {
            for u in 0..SIDE {
                let j = (v + HALO) * HALO_SIDE + u + HALO;
                let n = glam::Vec3::new(
                    samples[j - 1].height - samples[j + 1].height,
                    2. * STEP_M as f32,
                    samples[j - HALO_SIDE].height - samples[j + HALO_SIDE].height,
                )
                .normalize();
                let at = (v * SIDE + u) * 4;
                // Upper-hemisphere octahedral normals leave a byte for skylight.
                let oct = n / n.abs().element_sum();
                let x = unorm(0.5 + 0.5 * oct.x);
                let z = unorm(0.5 + 0.5 * oct.z);
                let sky = if terrain_sky {
                    unorm(sky_exposure(&samples, j, n))
                } else {
                    255
                };
                let [r, g, b, depth] = samples[j].albedo;
                for k in 0..4 {
                    pixels[8 * area + at + k] =
                        rivers.cover[(2 * v + k / 2) * RIVER_SIDE + 2 * u + k % 2];
                }
                let dry = depth == 255;
                pixels[at..at + 4].copy_from_slice(&[x, z, sky, if dry { 255 } else { 0 }]);
                pixels[4 * area + at..4 * area + at + 4].copy_from_slice(&[
                    r,
                    g,
                    b,
                    if dry { 0 } else { depth },
                ]);
            }
        }
        (
            Self {
                source,
                min,
                samples,
                rivers,
            },
            pixels,
        )
    }
}
// Angular fraction of diffuse sky hidden by the surrounding terrain, relative
// to the sample's own tangent plane. A planar face already receives the
// renderer's orientation-dependent sky light, so it gets no extra darkening.
fn sky_exposure(samples: &[Sample], center: usize, n: glam::Vec3) -> f32 {
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
        let stride = dx + dz * HALO_SIDE as isize;
        let length = if dx == 0 || dz == 0 {
            1.
        } else {
            std::f32::consts::SQRT_2
        };
        let tangent = (-(n.x * dx as f32 + n.z * dz as f32) / (n.y * length)).max(0.);
        let mut horizon = tangent;
        for distance in [2, 6, HALO as isize] {
            let neighbor = (center as isize + distance * stride) as usize;
            let slope = (samples[neighbor].height - h) / (distance as f32 * STEP_M as f32 * length);
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
        self.texture.width() as u64 * self.texture.height() as u64 * 12
    }
    pub(crate) fn new(device: &wgpu::Device, side: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Highlands detail normals and albedo"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 3,
            },
            mip_level_count: 1,
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
            .filter(|t| allowed && t.params().recipe == FbmRecipe::Highlands)
        else {
            self.landscape_detail = Detail::default();
            return;
        };
        let source = Source {
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
                        && (0..2).all(|a| grid.min[a].abs_diff(min[a]) <= SIDE as u64 / 3)
                    {
                        if self.shadows.near.detail.texture.width() != SIDE as u32 {
                            self.shadows.near.detail = Texture::new(device, SIDE as u32);
                            self.refresh_visibility_bind(device);
                        }
                        let texture = &self.shadows.near.detail.texture;
                        queue.write_texture(
                            texture.as_image_copy(),
                            &data,
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(SIDE as u32 * 4),
                                rows_per_image: Some(SIDE as u32),
                            },
                            texture.size(),
                        );
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
            .is_some_and(|g| g.min == min)
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
                (eye[0] - g.min[0] as f64 * STEP_M) as f32,
                (eye[2] - g.min[1] as f64 * STEP_M) as f32,
            ]
        })
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
                for dz in [-6., -2., 2., 6.] {
                    for dx in [-6., -2., 2., 6.] {
                        let p = Cell::from_meters([
                            (rivers.min[0] + x as i64) as f64 * 16. + dx,
                            0.,
                            (rivers.min[1] + z as i64) as f64 * 16. + dz,
                        ]);
                        let c = terrain.column_cells(p.x, p.z);
                        wet += u32::from(c.visible_top > 0 && c.visible_top > c.ground_top);
                    }
                }
                let cover = rivers.cover[z * RIVER_SIDE + x];
                assert_eq!(u32::from(cover), (wet * 255 + 8) / 16);
                mixed += u32::from(cover > 0 && cover < 255);
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
            (sky_exposure(&plane, center, glam::Vec3::new(-0.7, 1., 0.3).normalize()) - 1.).abs()
                < 1e-5
        );
        let bowl = field(|x, z| x.hypot(z));
        assert!((sky_exposure(&bowl, center, glam::Vec3::Y) - 0.5).abs() < 1e-5);
        let dome = field(|x, z| -x.hypot(z));
        assert_eq!(sky_exposure(&dome, center, glam::Vec3::Y), 1.);
    }

    #[test]
    fn moving_window_preserves_shared_height_normal_and_albedo() {
        let source = Source {
            seed: 40961,
            params: FbmParams {
                recipe: FbmRecipe::Highlands,
                ..Default::default()
            },
            edge: voxel_size(),
            vegetation: true,
        };
        let (old, before) = Grid::make(source, [-64, -128], None);
        let (new, after) = Grid::make(source, [-64 + RECENTER, -128 - RECENTER], Some(&old));
        let shift = RECENTER as usize;
        let area = SIDE * SIDE * 4;
        let river_shift = shift / 2;
        for v in river_shift..RIVER_SIDE {
            for u in 0..RIVER_SIDE - river_shift {
                assert_eq!(
                    old.rivers.cover[(v - river_shift) * RIVER_SIDE + u + river_shift],
                    new.rivers.cover[v * RIVER_SIDE + u]
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
                let x = new.min[0] + u as i64;
                let z = new.min[1] + v as i64;
                assert_eq!(
                    new.get(x, z).unwrap().height.to_bits(),
                    old.get(x, z).unwrap().height.to_bits()
                );
            }
        }
    }
}
