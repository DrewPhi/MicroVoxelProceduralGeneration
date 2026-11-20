//! A world-anchored flight shading field. A moving window retains canonical
//! samples on the overlap and evaluates only newly revealed strips.
use frontier_world::{voxel_size, Cell, FbmParams, FbmRecipe, FbmTerrain, Material};
use std::sync::{
    mpsc::{self, Receiver, TryRecvError},
    Arc,
};

const SIDE: usize = 257;
const HALO_SIDE: usize = SIDE + 2;
const STEP_M: f64 = 8.;
const RECENTER: i64 = 32;

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
}
impl Grid {
    fn get(&self, x: i64, z: i64) -> Option<Sample> {
        let u = x - self.min[0] + 1;
        let v = z - self.min[1] + 1;
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
                let x = min[0] + u as i64 - 1;
                let z = min[1] + v as i64 - 1;
                let sample = reusable.and_then(|g| g.get(x, z)).unwrap_or_else(|| {
                    let c = Cell::from_meters([x as f64 * STEP_M, 0., z as f64 * STEP_M]);
                    let (col, rgb) = terrain.landscape_column(c.x, c.z, source.vegetation);
                    let [r, g, b] = rgb.map(unorm);
                    Sample {
                        height: (col.visible_top as f64 * source.edge) as f32,
                        albedo: [
                            r,
                            g,
                            b,
                            if col.top_material() == Material::Water {
                                0
                            } else {
                                255
                            },
                        ],
                    }
                });
                samples.push(sample);
            }
        }
        let area = SIDE * SIDE;
        let mut pixels = vec![0; area * 8];
        for v in 0..SIDE {
            for u in 0..SIDE {
                let j = (v + 1) * HALO_SIDE + u + 1;
                let n = glam::Vec3::new(
                    samples[j - 1].height - samples[j + 1].height,
                    2. * STEP_M as f32,
                    samples[j - HALO_SIDE].height - samples[j + HALO_SIDE].height,
                )
                .normalize();
                let at = (v * SIDE + u) * 4;
                let [x, y, z] = n.to_array().map(|v| unorm(0.5 + v * 0.5));
                pixels[at..at + 4].copy_from_slice(&[x, y, z, samples[j].albedo[3]]);
                pixels[4 * area + at..4 * area + at + 4].copy_from_slice(&samples[j].albedo);
            }
        }
        (
            Self {
                source,
                min,
                samples,
            },
            pixels,
        )
    }
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
        self.texture.width() as u64 * self.texture.height() as u64 * 8
    }
    pub(crate) fn new(device: &wgpu::Device, side: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Highlands detail normals and albedo"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 2,
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
