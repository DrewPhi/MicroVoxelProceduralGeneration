//! Playable integration of the cached-heightfield experiment.
use crate::foreground_depth::{self, ForegroundDepth};
use crate::native_lattice::{self, NativeLattice, NativeLatticeConfig};
use crate::rolling_woodland_shadow::{self, CanopyGroundShade, WoodlandShadow};
use crate::terrain_horizon::{self, TerrainHorizon};
use crate::{
    aircraft_shadow::AircraftShadow,
    formation_shadow::{self, FormationShadow},
    settings::VisualSettings,
    Camera, Gpu, Result,
};
use bytemuck::{Pod, Zeroable};
use frontier_world::{voxel_grid, voxel_size, FbmParams, FbmRecipe, FbmTerrain, Material, World};
use glam::{DVec2, DVec3, IVec2, Vec3};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    thread::JoinHandle,
    time::Instant,
};
use wgpu::util::DeviceExt;

#[path = "rolling_cache.rs"]
mod rolling_cache;
#[path = "bin/cached_heightfield/stream.rs"]
mod stream;

const OUTER: u32 = 2048;
const INNER: u32 = 512;
const PROJECTED_MID_SIDE: u32 = 1024;
const MIN_NATIVE_SIDE: usize = 512;
pub(crate) const DEFAULT_NAADF_RADIUS: u32 = 128;
pub(crate) const MAX_NAADF_RADIUS: u32 = 512;
// Both outer caches cover this radius even immediately before a recenter.
pub(crate) const MAX_FAR: f32 = 9000.;
const SOURCE: &str = concat!(include_str!("bin/cached_heightfield.wgsl"),
    "\nfn outdoor_sun_color(elevation:f32)->vec3f {return mix(vec3f(2.12,1.44,.83),vec3f(2.,1.78,1.46),smoothstep(.06,.64,elevation));}\n",
    include_str!("water.wgsl"), "\nconst ROLLING_WATER_GROUP = 0u;\n",
    include_str!("terrain_horizon_sample.wgsl"),
    include_str!("rolling_water_bed.wgsl"),
    include_str!("aircraft_shadow.wgsl"),
    include_str!("rolling_woodland_shadow.wgsl"),
    include_str!("rolling_terrain.wgsl"));

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct View {
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    grid: [f32; 4],
    size: [u32; 4],
    cache: [i32; 4],
    inner: [f32; 4],
    inner_cache: [i32; 4],
    unused_history: [[f32; 4]; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct NativeView {
    grid: [f32; 4],
    owner_min: [f32; 4],
    owner_max: [f32; 4],
    sun: [f32; 4],
    sky: [f32; 4],
    voxel: [f32; 4],
    water: [f32; 4],
    source: [u32; 4],
    horizon_grid: [f32; 4],
    world_eye: [f32; 4],
    palette: [[f32; 4]; 10],
}
use crate::lod_curve::{LodCurve, TERRAIN_DISTANCES};

const CURVE_PROFILE_VECTORS: usize = 56;
const CURVE_NEAR_PITCH: f64 = 0.5;
const CURVE_NEAR_SIDE: u32 = 1024;

fn terrain_curve_samples(curve: &LodCurve) -> [[f32; 4]; CURVE_PROFILE_VECTORS] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            let index = (4 * row + column) as f32;
            curve.in_profile((index * 0.25).powi(2), &TERRAIN_DISTANCES)
        })
    })
}

fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}
fn readback(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}
fn fetch(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, move |status| {
        let _ = tx.send(status);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()??;
    let data = slice.get_mapped_range().to_vec();
    buffer.unmap();
    Ok(data)
}
fn texture(
    device: &wgpu::Device,
    side: u32,
    mips: u32,
    format: wgpu::TextureFormat,
    storage: bool,
) -> wgpu::Texture {
    let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rolling terrain texture"),
        size: wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: usage
            | if storage {
                wgpu::TextureUsages::STORAGE_BINDING
            } else {
                wgpu::TextureUsages::empty()
            },
        view_formats: &[],
    })
}
fn upload<T: Pod>(queue: &wgpu::Queue, texture: &wgpu::Texture, mip: u32, side: u32, data: &[T]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: mip,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(side * size_of::<T>() as u32),
            rows_per_image: Some(side),
        },
        wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
    );
}
fn reduce_maxima(side: usize, data: &[f32]) -> Vec<f32> {
    let next = side / 2;
    (0..next * next)
        .map(|i| {
            let a = 2 * (i / next) * side + 2 * (i % next);
            data[a]
                .max(data[a + 1])
                .max(data[a + side])
                .max(data[a + side + 1])
        })
        .collect()
}

fn upload_maxima(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mut side: usize,
    mut data: Vec<f32>,
) {
    for mip in 0..=side.ilog2() {
        upload(queue, texture, mip, side as u32, &data);
        if side == 1 {
            break;
        }
        data = reduce_maxima(side, &data);
        side /= 2;
    }
}

enum Source {
    Alpine(stream::Stream),
    Sampled(rolling_cache::SampledLayer),
}
struct Layer {
    source: Source,
    cache_textures: [wgpu::Texture; 3],
    heights: wgpu::TextureView,
    maxima: wgpu::TextureView,
    pigments: wgpu::TextureView,
    base: IVec2,
    side: u32,
    pitch: f64,
}
impl Layer {
    fn new(
        device: &wgpu::Device,
        side: u32,
        pitch: f64,
        terrain: &FbmTerrain,
        alpine: bool,
    ) -> Self {
        let heights = texture(device, side + 1, 1, wgpu::TextureFormat::R32Float, alpine);
        let maxima = texture(
            device,
            side,
            side.ilog2() + 1,
            wgpu::TextureFormat::R32Float,
            alpine,
        );
        let pigments = texture(
            device,
            if alpine { 1 } else { side + 1 },
            1,
            wgpu::TextureFormat::Rgba8Unorm,
            false,
        );
        let source = if alpine {
            Source::Alpine(stream::Stream::new(
                device,
                side,
                terrain.seed(),
                pitch,
                &heights,
                &maxima,
            ))
        } else {
            Source::Sampled(rolling_cache::SampledLayer::new(
                terrain.clone(),
                side,
                pitch,
            ))
        };
        Self {
            source,
            heights: heights.create_view(&Default::default()),
            maxima: maxima.create_view(&Default::default()),
            pigments: pigments.create_view(&Default::default()),
            cache_textures: [heights, maxima, pigments],
            base: IVec2::ZERO,
            side,
            pitch,
        }
    }
    fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, eye: DVec2) -> Result<()> {
        self.base = match &mut self.source {
            Source::Alpine(stream) => {
                let base = stream.target(eye);
                stream.update(device, queue, base)?;
                base
            }
            Source::Sampled(layer) => {
                let [heights, maxima, pigments] = &self.cache_textures;
                layer.update(queue, heights, maxima, pigments, eye)?
            }
        };
        Ok(())
    }
    fn grid(&self) -> [f32; 4] {
        [
            (f64::from(self.base.x) * self.pitch) as f32,
            (f64::from(self.base.y) * self.pitch) as f32,
            (1. / self.pitch) as f32,
            self.side as f32,
        ]
    }
    fn offset(&self) -> [i32; 4] {
        [
            self.base.x,
            self.base.y,
            self.side.ilog2() as i32,
            match &self.source {
                Source::Sampled(layer) => 1 + i32::from(layer.strip_uploads),
                Source::Alpine(_) => 0,
            },
        ]
    }
}

#[derive(Clone, Copy, Default)]
struct BaseColumn {
    top: i64,
    visible_top: i64,
    paint: i32,
}
struct BaseColumns {
    origin: [i64; 2],
    columns: Vec<BaseColumn>,
}

impl BaseColumns {
    fn sample(
        terrain: &FbmTerrain,
        side: usize,
        origin: [i64; 2],
        previous: Option<&Self>,
    ) -> Self {
        let mut columns = vec![BaseColumn::default(); side * side];
        std::thread::scope(|jobs| {
            for (part, rows) in columns.chunks_mut(side * side / 4).enumerate() {
                jobs.spawn(move || {
                    for (index, result) in rows.iter_mut().enumerate() {
                        let x = origin[0] + (index % side) as i64;
                        let z = origin[1] + (part * side / 4 + index / side) as i64;
                        if let Some(old) = previous {
                            let dx = x.wrapping_sub(old.origin[0]);
                            let dz = z.wrapping_sub(old.origin[1]);
                            if (0..side as i64).contains(&dx) && (0..side as i64).contains(&dz) {
                                *result = old.columns[dz as usize * side + dx as usize];
                                continue;
                            }
                        }
                        let value = terrain.column_cells(x, z);
                        *result = BaseColumn {
                            top: value.ground_top,
                            visible_top: value.visible_top,
                            paint: value.ground_surface as i32
                                | ((value.ground_subsurface as i32) << 8),
                        };
                    }
                });
            }
        });
        Self { origin, columns }
    }
}

struct PreparedColumns {
    datum: i64,
    columns: Vec<[i32; 4]>,
    maxima: Vec<Vec<f32>>,
}

impl PreparedColumns {
    fn sample(base: &BaseColumns, datum: i64, side: usize, size: f64) -> Option<Self> {
        let local = |y: i64| i32::try_from(y.checked_sub(datum)?).ok();
        let columns = base
            .columns
            .iter()
            .map(|c| Some([local(c.top)?, c.paint, local(c.visible_top)?, 0]))
            .collect::<Option<Vec<_>>>()?;
        let mut maxima = vec![columns
            .iter()
            .map(|c| (f64::from(c[2]) * size) as f32)
            .collect::<Vec<_>>()];
        for mip in 0..side.ilog2() {
            maxima.push(reduce_maxima(side >> mip, maxima.last().unwrap()));
        }
        Some(Self {
            datum,
            columns,
            maxima,
        })
    }
}

struct PendingColumns {
    base: BaseColumns,
    upload: Option<PreparedColumns>,
}

struct ExactColumns {
    base: [i64; 2],
    datum: i64,
    side: usize,
    revision: Option<u64>,
    sampled: Option<Arc<BaseColumns>>,
    pending: Option<JoinHandle<PendingColumns>>,
    prepare_upload: bool,
    columns: wgpu::Texture,
    maxima: wgpu::Texture,
    runs: wgpu::Buffer,
    last_eye: Option<[i64; 2]>,
    publications: u64,
    proxy_fallback_updates: u64,
}
fn runs_buffer(device: &wgpu::Device, runs: &[[i32; 4]]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sparse edited column intervals"),
        contents: bytemuck::cast_slice(runs),
        usage: wgpu::BufferUsages::STORAGE,
    })
}
impl ExactColumns {
    fn side_for(radius_voxels: u32) -> usize {
        MIN_NATIVE_SIDE.max((4 * radius_voxels as usize).next_power_of_two())
    }
    fn new(device: &wgpu::Device, radius_voxels: u32, detail: bool) -> Self {
        // The 2D fallback surrounds the volume through an entire origin shift.
        let side = Self::side_for(radius_voxels).max(if detail { 1024 } else { 512 });
        let n = side as u32;
        Self {
            base: [0; 2],
            datum: 0,
            side,
            revision: None,
            sampled: None,
            pending: None,
            prepare_upload: detail,
            columns: texture(device, n, 1, wgpu::TextureFormat::Rgba32Sint, false),
            maxima: texture(
                device,
                n,
                n.ilog2() + 1,
                wgpu::TextureFormat::R32Float,
                false,
            ),
            runs: runs_buffer(device, &[[0; 4]]),
            last_eye: None,
            publications: 0,
            proxy_fallback_updates: 0,
        }
    }
    fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        eye: DVec3,
    ) -> Result<bool> {
        let side = self.side;
        let size = voxel_size();
        let cell = voxel_grid()
            .checked_cell(eye.to_array())
            .ok_or("camera is outside the addressable rolling voxel grid")?;
        let snap = side as i64 / 4;
        let axis = |cell: i64| -> Result<i64> {
            let base = (cell.div_euclid(snap) * snap)
                .checked_sub(side as i64 / 2)
                .ok_or("native rolling window crosses the voxel coordinate limit")?;
            base.checked_add(side as i64)
                .ok_or("native rolling window crosses the voxel coordinate limit")?;
            Ok(base)
        };
        let eye = [cell.x, cell.z];
        let centered = [axis(cell.x)?, axis(cell.z)?];
        let previous_eye = self.last_eye.replace(eye).unwrap_or(eye);
        let shift = [0, 1].map(|i| eye[i].saturating_sub(previous_eye[i]));
        let teleported = shift.iter().any(|v| v.unsigned_abs() > side as u64 / 2);
        let margin = |origin: [i64; 2]| {
            (0..2)
                .map(|i| {
                    let offset = eye[i].saturating_sub(origin[i]);
                    offset.min((side as i64 - 1).saturating_sub(offset))
                })
                .min()
                .unwrap()
        };
        // Keep the voxel-plane datum until it has moved far enough to matter
        // for f32 precision. Descents do not repack the cache every 16 metres.
        let datum = if self.sampled.is_none() || cell.y.abs_diff(self.datum) > 2048 {
            cell.y.div_euclid(256) * 256
        } else {
            self.datum
        };
        let terrain = world.fbm_terrain().expect("rolling cache source");
        // Ordinary flight can render the coherent sampled terrain while the
        // exact window catches up. Edits and teleports require exact coverage.
        let synchronous = self.sampled.is_none()
            || (margin(self.base) < snap / 8 && (teleported || world.revision() != 0));
        let mut prepared = None;
        if self.pending.as_ref().is_some_and(JoinHandle::is_finished) {
            let ready = self
                .pending
                .take()
                .unwrap()
                .join()
                .map_err(|_| "rolling voxel worker panicked")?;
            if ready.base.origin == centered || margin(ready.base.origin) > margin(self.base) {
                self.sampled = Some(Arc::new(ready.base));
                prepared = ready.upload;
            }
        }
        if synchronous
            && self
                .sampled
                .as_ref()
                .is_none_or(|columns| columns.origin != centered)
        {
            prepared = None;
            self.sampled = Some(Arc::new(BaseColumns::sample(
                terrain,
                side,
                centered,
                self.sampled.as_deref(),
            )));
        }
        let sampled = self.sampled.as_ref().unwrap();
        let origin = sampled.origin;
        if self.pending.is_none() {
            // Lead fast flight by at most one strip. Completion still checks
            // coverage against the camera's actual position and direction.
            let eroded = terrain.params().recipe == FbmRecipe::Eroded;
            let frames = if eroded { 10 } else { 3 };
            let distance = if eroded { snap } else { snap / 2 };
            let ahead = [0, 1].map(|i| {
                eye[i].saturating_add(shift[i].saturating_mul(frames).clamp(-distance, distance))
            });
            let target = [axis(ahead[0])?, axis(ahead[1])?];
            if origin != target {
                let previous = Arc::clone(sampled);
                let terrain = terrain.clone();
                let prepare_upload = self.prepare_upload && world.revision() == 0;
                self.pending = Some(std::thread::spawn(move || {
                    let base = BaseColumns::sample(&terrain, side, target, Some(&previous));
                    let upload = prepare_upload
                        .then(|| PreparedColumns::sample(&base, datum, side, size))
                        .flatten();
                    PendingColumns { base, upload }
                }));
            }
        }
        self.proxy_fallback_updates += u64::from(margin(origin) < 0);
        if self.base == origin && self.datum == datum && self.revision == Some(world.revision()) {
            return Ok(false);
        }
        if let Some(prepared) = prepared.filter(|p| p.datum == datum && world.revision() == 0) {
            upload(queue, &self.columns, 0, side as u32, &prepared.columns);
            for (mip, data) in prepared.maxima.iter().enumerate() {
                upload(queue, &self.maxima, mip as u32, (side >> mip) as u32, data);
            }
            self.publish(origin, datum, world.revision());
            return Ok(true);
        }
        let mut edited: BTreeMap<[i64; 2], Vec<i64>> = BTreeMap::new();
        for cell in world.edit_cells_in_columns(origin, origin.map(|v| v + side as i64)) {
            if let Some(end) = cell.y.checked_add(1) {
                edited
                    .entry([cell.x, cell.z])
                    .or_default()
                    .extend([cell.y, end]);
            }
        }
        // Keep GPU heights relative to a voxel plane near the camera. In
        // particular, decimal grid edges must not cancel against a large
        // absolute altitude during a ray/roof crossing.
        let local = |y: i64| -> Result<i32> {
            y.checked_sub(datum)
                .and_then(|dy| i32::try_from(dy).ok())
                .ok_or_else(|| "rolling voxel height is outside the local GPU column range".into())
        };
        let mut columns = sampled
            .columns
            .iter()
            .map(|c| Ok([local(c.top)?, c.paint, local(c.visible_top)?, 0]))
            .collect::<Result<Vec<_>>>()?;
        let mut maxima = columns
            .iter()
            .map(|c| (f64::from(c[2]) * size) as f32)
            .collect::<Vec<_>>();
        let mut runs = vec![[0i32; 4]];
        for ([x, z], mut edges) in edited {
            let index = (z - origin[1]) as usize * side + (x - origin[0]) as usize;
            let top = sampled.columns[index].top;
            let floor = top.min(edges[0]).min(datum).saturating_sub(2048);
            edges.extend([
                floor,
                top - 12,
                top - 1,
                top,
                sampled.columns[index].visible_top,
            ]);
            edges.sort_unstable();
            edges.dedup();
            let column = world.column(x, z);
            let first = runs.len();
            for edge in edges.windows(2) {
                let material = column.material_without_fluid(edge[0]);
                if material != Material::Air {
                    runs.push([local(edge[0])?, local(edge[1])?, material as i32, 0]);
                }
            }
            columns[index][2] = first as i32;
            columns[index][3] = (runs.len() - first) as i32;
            maxima[index] = (f64::from(runs.last().unwrap()[1]) * size) as f32;
        }
        self.runs = runs_buffer(device, &runs);
        upload(queue, &self.columns, 0, side as u32, &columns);
        upload_maxima(queue, &self.maxima, side, maxima);
        self.publish(origin, datum, world.revision());
        Ok(true)
    }

    fn publish(&mut self, origin: [i64; 2], datum: i64, revision: u64) {
        self.base = origin;
        self.datum = datum;
        self.revision = Some(revision);
        self.publications += 1;
    }
}

pub(crate) struct RollingTerrain {
    outer: Layer,
    inner: Layer,
    detail: Option<Layer>,
    profile_detail: Option<Layer>,
    profile_detail_requested: bool,
    profile_detail_ready_at: Option<Instant>,
    terrain_curve: LodCurve,
    curve_signs: [f32; 2],
    curve_negative_start: f32,
    curve_uniform: wgpu::Buffer,
    exact: ExactColumns,
    lattice: Option<NativeLattice>,
    view: wgpu::Buffer,
    native_view: wgpu::Buffer,
    water_layout: wgpu::BindGroupLayout,
    water_bind: wgpu::BindGroup,
    aircraft_shadow: AircraftShadow,
    formation_shadow: FormationShadow,
    woodland_shadow: WoodlandShadow,
    canopy_ground_shade: CanopyGroundShade,
    horizon: Option<TerrainHorizon>,
    horizon_sampler: wgpu::Sampler,
    pub foreground: Option<ForegroundDepth>,
    landscapes: bool,
    pipeline: wgpu::RenderPipeline,
    source: [u32; 4],
    bind: Option<wgpu::BindGroup>,
}

fn horizon_bindings<'a>(
    outer: &'a Layer,
    horizon: Option<&'a TerrainHorizon>,
    sampler: &'a wgpu::Sampler,
) -> [wgpu::BindGroupEntry<'a>; 2] {
    [
        wgpu::BindGroupEntry {
            binding: terrain_horizon::BINDING,
            resource: wgpu::BindingResource::TextureView(
                horizon.map_or(&outer.pigments, |h| &h.view),
            ),
        },
        wgpu::BindGroupEntry {
            binding: terrain_horizon::SAMPLER_BINDING,
            resource: wgpu::BindingResource::Sampler(sampler),
        },
    ]
}

impl RollingTerrain {
    pub fn new(gpu: &Gpu, world: &World, settings: &VisualSettings) -> Result<Self> {
        let device = &gpu.device;
        let terrain = world
            .fbm_terrain()
            .ok_or("rolling terrain requires a procedural terrain source")?;
        let landscapes = terrain.params().recipe.has_landscape_relief();
        let use_lattice = terrain.native_lattice_enabled();
        let lattice_config = NativeLatticeConfig::for_source(terrain.native_detail_enabled(), settings.msaa);
        let living_woods = frontier_world::vegetation::uses_living_woodland(world);
        let close_pitch = settings
            .terrain_near_spacing
            .map_or(4. * voxel_size(), f64::from);
        let alpine = terrain.dirt_runway().is_none()
            && terrain.params()
                == (FbmParams {
                    recipe: FbmRecipe::Alpine,
                    ..Default::default()
                })
            && device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let inner_pitch = if alpine {
            0.5
        } else {
            f64::from(settings.terrain_mid_spacing)
        };
        let detail = settings.terrain_detail && (voxel_size()..inner_pitch).contains(&close_pitch);
        if !alpine
            && terrain.params().recipe == FbmRecipe::Alpine
            && !device.features().contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            eprintln!(
                "rolling terrain: GPU strip timing unavailable, using canonical CPU sampling"
            );
        }
        let foreground = settings
            .terrain_depth_occlusion
            .then(|| ForegroundDepth::new(device, settings.msaa));
        let foreground_source = if foreground.is_some() {
            foreground_depth::SAMPLE
        } else {
            "fn foreground_at(pixel:vec2u)->f32 {return 1.;}"
        };
        let lattice_source = if use_lattice {
            native_lattice::SHADER
        } else {
            ""
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rolling terrain composition"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{SOURCE}\n{foreground_source}\n{lattice_source}").into(),
            ),
        });
        let bindings = [0, 1, 2, 6, 7, 9, 10, 11, 12, 13, 14, 18, 19, 31].map(|binding| {
            wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                count: None,
                ty: if matches!(binding, 1 | 2 | 6 | 7 | 10 | 11 | 13 | 14 | 18 | 19) {
                    wgpu::BindingType::Texture {
                        sample_type: if binding == 11 {
                            wgpu::TextureSampleType::Sint
                        } else {
                            wgpu::TextureSampleType::Float { filterable: false }
                        },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    }
                } else {
                    wgpu::BindingType::Buffer {
                        ty: if binding == 12 {
                            wgpu::BufferBindingType::Storage { read_only: true }
                        } else {
                            wgpu::BufferBindingType::Uniform
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    }
                },
            }
        });
        let horizon_layout = [
            wgpu::BindGroupLayoutEntry {
                binding: terrain_horizon::BINDING,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: terrain_horizon::SAMPLER_BINDING,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        let shadow_layout = AircraftShadow::layout_entries();
        let mut rolling_layout = bindings.to_vec();
        rolling_layout.extend(shadow_layout);
        rolling_layout.extend(AircraftShadow::layout_entries_at(formation_shadow::BINDING));
        rolling_layout.extend(AircraftShadow::layout_entries_at(
            rolling_woodland_shadow::BINDING,
        ));
        rolling_layout.extend(horizon_layout);
        rolling_layout.extend(CanopyGroundShade::read_entries());
        if use_lattice {
            rolling_layout.extend(NativeLattice::layout());
        }
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rolling terrain layout"),
            entries: &rolling_layout,
        });
        let mut groups = vec![&bind_layout];
        if let Some(depth) = &foreground {
            groups.push(&depth.read_layout);
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &groups,
            push_constant_ranges: &[],
        });
        let lattice_constants = if use_lattice {
            lattice_config.shader_constants()
        } else {
            HashMap::new()
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("meshless terrain and voxel column depth"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(if use_lattice && settings.msaa > 1 {
                    "lattice_sample_triangle"
                } else {
                    "rolling_triangle"
                }),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(if use_lattice && settings.msaa > 1 {
                    "lattice_terrain_msaa"
                } else if use_lattice {
                    "lattice_terrain_fragment"
                } else {
                    "rolling_fragment"
                }),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &HashMap::from_iter(lattice_constants.into_iter().chain([
                        (
                            "METHOD".into(),
                            if settings.terrain_mid_boundary_ascent {
                                1.
                            } else {
                                2.
                            },
                        ),
                        ("BILINEAR".into(), 1.),
                        ("STREAM".into(), f64::from(u8::from(alpine))),
                        ("CANONICAL_PIGMENT".into(), f64::from(u8::from(!alpine))),
                        ("NESTED".into(), 1.),
                        ("LANDSCAPES".into(), f64::from(u8::from(landscapes))),
                        (
                            "FOREST_AERIAL_LIGHT".into(),
                            f64::from(u8::from(
                                settings.natural_terrain_light
                                    && !alpine
                                    && crate::woodland_volume::render_option(
                                        "FRONTIER_WOODLAND_AERIAL_LIGHT",
                                        living_woods,
                                    ),
                            )),
                        ),
                        (
                            "NATURAL_TERRAIN_LIGHT".into(),
                            f64::from(u8::from(settings.natural_terrain_light && !alpine)),
                        ),
                        ("PROJECTED_DETAIL".into(), f64::from(u8::from(detail))),
                        (
                            "PROJECTED_MID".into(),
                            f64::from(u8::from(settings.terrain_mid_detail)),
                        ),
                        ("DISTANCE_LOD_CURVE".into(), 1.),
                        (
                            "SCROLLED_OUTER".into(),
                            f64::from(u8::from(settings.terrain_far_detail)),
                        ),
                        (
                            "REFINE_MID".into(),
                            f64::from(u8::from(settings.terrain_mid_error_px > 0.)),
                        ),
                        (
                            "CAPTURE_DISTANCE".into(),
                            f64::from(u8::from(settings.capture_terrain_distance)),
                        ),
                    ])),
                    ..Default::default()
                },
                targets: &[Some(gpu.scene_format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: settings.msaa,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let uniform = |contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        };
        let exact = ExactColumns::new(
            device,
            if settings.naadf_enabled {
                settings.naadf_radius_voxels
            } else {
                0
            },
            detail
                && (settings.terrain_near_spacing.is_none() || settings.terrain_near_cells > 512),
        );
        let native_view = uniform(bytemuck::bytes_of(&NativeView::zeroed()));
        let aircraft_shadow = AircraftShadow::new(device);
        let formation_center = world
            .rock_formations()
            .get(1)
            .map(|rock| DVec3::from_array(rock.center_m()));
        let formation_shadow = FormationShadow::new(device, formation_center);
        let woodland_shadow = WoodlandShadow::new(device, living_woods);
        let canopy_ground_shade = CanopyGroundShade::new(
            device,
            crate::woodland_volume::render_option("FRONTIER_WOODLAND_WIDE_SHADOWS", living_woods),
        );
        let mut water_entries = vec![bindings[5], bindings[7]];
        water_entries.extend(shadow_layout);
        water_entries.extend(AircraftShadow::layout_entries_at(formation_shadow::BINDING));
        water_entries.extend(AircraftShadow::layout_entries_at(
            rolling_woodland_shadow::BINDING,
        ));
        water_entries.extend(horizon_layout);
        let water_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rolling water bed and aircraft shadow for NAADF"),
            entries: &water_entries,
        });
        let columns = exact.columns.create_view(&Default::default());
        let mut water_resources = vec![
            wgpu::BindGroupEntry {
                binding: 9,
                resource: native_view.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(&columns),
            },
        ];
        water_resources.extend(aircraft_shadow.bindings());
        water_resources.extend(
            formation_shadow
                .receiver
                .bindings_at(formation_shadow::BINDING),
        );
        water_resources.extend(
            woodland_shadow
                .receiver
                .bindings_at(rolling_woodland_shadow::BINDING),
        );
        let mut inner = Layer::new(
            device,
            settings.terrain_mid_cells.unwrap_or(
                match (settings.terrain_mid_detail, settings.terrain_mid_spacing) {
                    (true, 0.75) => PROJECTED_MID_SIDE * 2,
                    (true, 1.0 | 1.5) => PROJECTED_MID_SIDE,
                    _ => INNER,
                },
            ),
            inner_pitch,
            terrain,
            alpine,
        );
        if let Source::Sampled(layer) = &mut inner.source {
            layer.strip_uploads = settings.terrain_mid_strips;
            layer.error_bounds = settings.terrain_mid_error_px > 0.;
            layer.tile_error_m = settings.terrain_mid_tile_error_m;
        }
        if settings.terrain_mid_error_px > 0. {
            inner.cache_textures[1] = texture(
                device,
                inner.side,
                inner.side.ilog2() + 1,
                wgpu::TextureFormat::Rg32Float,
                false,
            );
            inner.maxima = inner.cache_textures[1].create_view(&Default::default());
        }
        let mut outer = if alpine {
            Layer::new(device, OUTER, 10., terrain, true)
        } else {
            Layer::new(
                device,
                if settings.terrain_far_detail {
                    2048
                } else {
                    1024
                },
                if settings.terrain_far_detail && settings.view_distance <= 6500. {
                    8.
                } else if settings.terrain_far_detail {
                    11.
                } else if landscapes && settings.view_distance <= 6500. {
                    16.
                } else if landscapes {
                    22.
                } else {
                    24.
                },
                terrain,
                false,
            )
        };
        if let Source::Sampled(layer) = &mut outer.source {
            layer.outer_coverage = true;
            layer.strip_uploads = settings.terrain_far_detail;
        }
        let horizon = (settings.natural_terrain_light && !alpine)
            .then(|| TerrainHorizon::new(device, &outer.heights));
        let horizon_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain horizon interpolation"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        water_resources.extend(horizon_bindings(&outer, horizon.as_ref(), &horizon_sampler));
        let water_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rolling columns, water and light for NAADF"),
            layout: &water_layout,
            entries: &water_resources,
        });
        Ok(Self {
            outer,
            inner,
            detail: detail.then(|| {
                Layer::new(
                    device,
                    settings.terrain_near_cells,
                    close_pitch,
                    terrain,
                    false,
                )
            }),
            profile_detail: None,
            profile_detail_requested: false,
            profile_detail_ready_at: None,
            terrain_curve: LodCurve::default(),
            curve_signs: [0.; 2],
            curve_negative_start: 0.,
            curve_uniform: uniform(bytemuck::cast_slice(&[[0_f32; 4]; CURVE_PROFILE_VECTORS])),
            exact,
            lattice: use_lattice.then(|| {
                NativeLattice::new(device, &gpu.queue, &module, world.seed(), lattice_config)
            }),
            view: uniform(bytemuck::bytes_of(&View::zeroed())),
            native_view,
            water_layout,
            water_bind,
            aircraft_shadow,
            formation_shadow,
            woodland_shadow,
            canopy_ground_shade,
            horizon,
            horizon_sampler,
            foreground,
            landscapes,
            pipeline,
            source: [world.seed() as u32, (world.seed() >> 32) as u32, 0, 0],
            bind: None,
        })
    }
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        camera: Camera,
        curve: &LodCurve,
    ) -> Result<()> {
        let eye = DVec2::new(camera.position.x, camera.position.z);
        self.outer.update(device, queue, eye)?;
        if let Some(lattice) = &mut self.lattice {
            if lattice.update(device, world, camera, &self.view)? {
                self.bind = None;
            }
        } else {
            self.inner.update(device, queue, eye)?;
        }
        if let Some(detail) = &mut self.detail {
            detail.update(device, queue, eye)?;
        }
        if self.lattice.is_none() && &self.terrain_curve != curve {
            self.terrain_curve = curve.clone();
            let samples = terrain_curve_samples(curve);
            let values = samples.iter().flatten().copied();
            let first_negative = values.clone().position(|v| v < 0.);
            self.curve_negative_start =
                first_negative.map_or(0., |i| (i.saturating_sub(1) as f32 * 0.25).powi(2));
            self.curve_signs = [
                f32::from(values.clone().any(|v| v > 0.)),
                f32::from(first_negative.is_some()),
            ];
            self.profile_detail_requested = self.detail.is_none()
                && self.inner.pitch > CURVE_NEAR_PITCH
                && values.take(64).any(|v| v > 0.);
            queue.write_buffer(&self.curve_uniform, 0, bytemuck::cast_slice(&samples));
        }
        if self.profile_detail_requested {
            if self.profile_detail.is_none() {
                let terrain = world.fbm_terrain().expect("rolling procedural terrain");
                let mut detail =
                    Layer::new(device, CURVE_NEAR_SIDE, CURVE_NEAR_PITCH, terrain, false);
                if let Source::Sampled(source) = &mut detail.source {
                    source.defer_initial = true;
                }
                self.profile_detail = Some(detail);
                self.bind = None;
            }
            let detail = self.profile_detail.as_mut().unwrap();
            detail.update(device, queue, eye)?;
            if matches!(&detail.source, Source::Sampled(source) if source.ready()) {
                self.profile_detail_ready_at
                    .get_or_insert_with(Instant::now);
            }
        } else {
            self.profile_detail_ready_at = None;
        }
        if self.exact.update(device, queue, world, camera.position)? || self.bind.is_none() {
            let maxima = self.exact.maxima.create_view(&Default::default());
            let columns = self.exact.columns.create_view(&Default::default());
            let mut resources = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.outer.maxima),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.outer.heights),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self.inner.maxima),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&self.inner.heights),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: self.native_view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(&maxima),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(&columns),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: self.exact.runs.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::TextureView(&self.outer.pigments),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: wgpu::BindingResource::TextureView(&self.inner.pigments),
                },
            ];
            let detail = self
                .detail
                .as_ref()
                .or(self.profile_detail.as_ref())
                .unwrap_or(&self.inner);
            resources.extend([
                wgpu::BindGroupEntry {
                    binding: 18,
                    resource: wgpu::BindingResource::TextureView(&detail.maxima),
                },
                wgpu::BindGroupEntry {
                    binding: 19,
                    resource: wgpu::BindingResource::TextureView(&detail.heights),
                },
                wgpu::BindGroupEntry {
                    binding: 31,
                    resource: self.curve_uniform.as_entire_binding(),
                },
            ]);
            resources.extend(self.aircraft_shadow.bindings());
            resources.extend(
                self.formation_shadow
                    .receiver
                    .bindings_at(formation_shadow::BINDING),
            );
            resources.extend(
                self.woodland_shadow
                    .receiver
                    .bindings_at(rolling_woodland_shadow::BINDING),
            );
            resources.extend(horizon_bindings(
                &self.outer,
                self.horizon.as_ref(),
                &self.horizon_sampler,
            ));
            resources.extend(self.canopy_ground_shade.bindings());
            if let Some(lattice) = &self.lattice {
                resources.extend(lattice.bindings());
            }
            self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rolling terrain resources"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &resources,
            }));
        }
        Ok(())
    }
    pub fn owns_column(&self, cell: frontier_world::Cell) -> bool {
        self.exact.revision.is_some()
            && [cell.x, cell.z]
                .into_iter()
                .zip(self.exact.base)
                .all(|(coord, lo)| (lo..lo + self.exact.side as i64).contains(&coord))
    }

    pub fn revision(&self) -> Option<u64> {
        self.exact.revision
    }
    pub fn coverage_ready(&self) -> bool {
        self.bind.is_some()
    }
    pub fn fits_settings(&self, settings: &VisualSettings) -> bool {
        (self.lattice.is_some() || !matches!(self.outer.pitch, 8. | 16.) || settings.view_distance <= 6500.)
            && (!settings.naadf_enabled
                || self.exact.side >= ExactColumns::side_for(settings.naadf_radius_voxels))
    }

    pub fn cache_report(&self) -> String {
        let layer = |l: &Layer| {
            let publishing = match &l.source {
                Source::Sampled(cache) => cache.publish_report(),
                Source::Alpine(_) => "null".to_owned(),
            };
            format!(
                r#"{{"side":{},"pitch_m":{},"cpu_cache":{}}}"#,
                l.side, l.pitch, publishing
            )
        };
        format!(
            r#"{{"gpu_bytes":{},"outer":{},"inner":{},"near":{},"exact_publications":{},"exact_proxy_fallback_updates":{},"horizon_builds":{},"native_lattice":{}}}"#,
            self.gpu_bytes(),
            layer(&self.outer),
            layer(&self.inner),
            self.detail
                .as_ref()
                .or(self.profile_detail.as_ref())
                .map_or_else(|| "null".to_owned(), layer),
            self.exact.publications,
            self.exact.proxy_fallback_updates,
            self.horizon.as_ref().map_or(0, TerrainHorizon::builds),
            self.lattice.as_ref().map_or_else(|| "null".to_owned(), NativeLattice::report)
        )
    }
    pub fn gpu_bytes(&self) -> usize {
        let layer_bytes = |layer: &Layer| {
            layer
                .cache_textures
                .iter()
                .map(|texture| {
                    (0..texture.mip_level_count())
                        .map(|mip| {
                            ((texture.width() >> mip).max(1) as usize)
                                * ((texture.height() >> mip).max(1) as usize)
                                * if texture.format() == wgpu::TextureFormat::Rg32Float {
                                    8
                                } else {
                                    4
                                }
                        })
                        .sum::<usize>()
                })
                .sum::<usize>()
        };
        let exact_side = self.exact.side;
        layer_bytes(&self.outer)
            + layer_bytes(&self.inner)
            + self.detail.as_ref().map_or(0, layer_bytes)
            + self.profile_detail.as_ref().map_or(0, layer_bytes)
            + exact_side * exact_side * 16
            + (exact_side * exact_side * 4 / 3) * 4
            + self.exact.runs.size() as usize
            + AircraftShadow::BYTES
            + self.formation_shadow.gpu_bytes()
            + self.woodland_shadow.gpu_bytes()
            + self.canopy_ground_shade.bytes()
            + usize::from(self.horizon.is_some()) * terrain_horizon::GPU_BYTES
            + self.foreground.as_ref().map_or(0, ForegroundDepth::bytes)
            + self.lattice.as_ref().map_or(0, NativeLattice::bytes)
    }
    pub fn formation_shadow(&self) -> &FormationShadow {
        &self.formation_shadow
    }

    pub fn woodland_shadow(&self) -> &WoodlandShadow {
        &self.woodland_shadow
    }

    pub(crate) fn canopy_ground_shade(&self) -> &CanopyGroundShade {
        &self.canopy_ground_shade
    }

    pub fn encode_terrain_horizon(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        settings: &VisualSettings,
    ) {
        if let Some(horizon) = self
            .horizon
            .as_ref()
            .filter(|_| settings.beauty && settings.terrain_shadows && !settings.terrain_gray)
        {
            horizon.encode(
                queue,
                encoder,
                self.outer.grid(),
                self.outer.offset(),
                settings.terrain_light_direction(),
            );
        }
    }

    pub fn terrain_horizon_resources(&self) -> Option<(&wgpu::TextureView, &wgpu::Sampler)> {
        self.horizon
            .as_ref()
            .map(|field| (&field.view, &self.horizon_sampler))
    }

    pub fn terrain_horizon_grid(&self) -> [f32; 4] {
        self.horizon.as_ref().map_or([0.; 4], |_| self.outer.grid())
    }

    pub fn aircraft_shadow(&self) -> &AircraftShadow {
        &self.aircraft_shadow
    }
    pub fn water_layout(&self) -> &wgpu::BindGroupLayout {
        &self.water_layout
    }
    pub fn water_bind(&self) -> &wgpu::BindGroup {
        &self.water_bind
    }
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        camera: Camera,
        width: u32,
        height: u32,
        owner: Option<[[f64; 3]; 2]>,
        settings: &VisualSettings,
        water_time: f32,
        ownership_probe: bool,
        tree_radius: f64,
    ) {
        let forward = camera.direction();
        let scale = (camera.fov_y.to_radians() * 0.5).tan();
        let right = forward.cross(camera.view_up()).normalize();
        let up = right.cross(forward).normalize();
        let view = View {
            eye: camera.position.as_vec3().extend(camera.far).to_array(),
            forward: forward
                .extend(f32::from(
                    settings.atmosphere && settings.fog_density > 0. && !ownership_probe,
                ))
                .to_array(),
            right: (right * scale * width as f32 / height as f32)
                .extend(0.)
                .to_array(),
            up: (up * scale)
                .extend(2. * scale / height.max(1) as f32)
                .to_array(),
            grid: self.outer.grid(),
            cache: self.outer.offset(),
            size: [
                width,
                height,
                self.outer.side.ilog2(),
                u32::from(
                    voxel_grid()
                        .checked_cell(camera.position.to_array())
                        .is_some_and(|c| self.owns_column(c)),
                ),
            ],
            inner: self.inner.grid(),
            inner_cache: self.inner.offset(),
            unused_history: [
                self.detail
                    .as_ref()
                    .or(self.profile_detail.as_ref())
                    .map_or([0.; 4], Layer::grid),
                [
                    settings.terrain_mid_error_px,
                    f32::from(self.outer.pitch % self.inner.pitch == 0.),
                    self.curve_signs[0],
                    self.curve_signs[1],
                ],
                [
                    self.profile_detail_ready_at
                        .map_or(0., |ready| (ready.elapsed().as_secs_f32() / 0.35).min(1.)),
                    self.curve_negative_start,
                    0.,
                    0.,
                ],
                [0.; 4],
            ],
        };
        queue.write_buffer(&self.view, 0, bytemuck::bytes_of(&view));
        if let Some(lattice) = &self.lattice {
            lattice.prepare(queue, camera);
        }
        let [lo, hi] = owner
            .unwrap_or([[0.; 3]; 2])
            .map(|v| glam::DVec3::from_array(v).as_vec3());
        let sun = settings.terrain_light_direction();
        let size = voxel_size();
        let eye_y = camera.position.y - self.exact.datum as f64 * size;
        let native = NativeView {
            grid: [
                (self.exact.base[0] as f64 * size) as f32,
                (self.exact.base[1] as f64 * size) as f32,
                (1. / size) as f32,
                self.exact.side as f32,
            ],
            owner_min: lo.extend(if owner.is_some() { 1. } else { 0. }).to_array(),
            owner_max: hi.extend(f32::from(settings.vegetation)).to_array(),
            sun: sun.extend(if settings.beauty { 1. } else { 0. }).to_array(),
            sky: [
                settings.overcast,
                (eye_y - f64::from(eye_y as f32)) as f32,
                ((camera.position.x - self.exact.base[0] as f64 * size) / size) as f32,
                ((camera.position.z - self.exact.base[1] as f64 * size) / size) as f32,
            ],
            voxel: [size as f32, (1. / size) as f32, eye_y as f32, scale],
            water: [
                camera.position.x.rem_euclid(128.) as f32,
                camera.position.z.rem_euclid(128.) as f32,
                water_time,
                1. / height as f32,
            ],
            source: [
                self.source[0],
                self.source[1],
                u32::from(settings.beauty && settings.terrain_shadows),
                u32::from(ownership_probe)
                    | (u32::from(
                        self.landscapes && settings.voxel_filter && self.exact.revision == Some(0),
                    ) << 1)
                    | (u32::from(settings.terrain_gray) << 2)
                    | (u32::from(self.lattice.is_some()) << 3),
            ],
            horizon_grid: self.terrain_horizon_grid(),
            world_eye: camera
                .position
                .as_vec3()
                .extend(tree_radius as f32)
                .to_array(),
            palette: [
                Material::Air,
                Material::Grass,
                Material::Soil,
                Material::Stone,
                Material::Wood,
                Material::Leaf,
                Material::Flower,
                Material::Water,
                Material::Sand,
                Material::Snow,
            ]
            .map(|m| Vec3::from_array(m.color()).extend(1.).to_array()),
        };
        queue.write_buffer(&self.native_view, 0, bytemuck::bytes_of(&native));
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, visibility: Option<&wgpu::BindGroup>) {
        if let Some(bind) = &self.bind {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind, &[]);
            if let Some(foreground) = &self.foreground {
                pass.set_bind_group(1, visibility.unwrap_or_else(|| foreground.fallback()), &[]);
            }
            pass.draw(0..3, 0..1);
        }
    }

    pub fn center_hits(&self, width: u32, height: u32) -> Option<&wgpu::Buffer> {
        self.lattice.as_ref()?.center_hits(width, height)
    }

    pub fn encode_native_visibility(&self, encoder: &mut wgpu::CommandEncoder, width: u32, height: u32) {
        if let Some(lattice) = &self.lattice {
            lattice.encode_visibility(encoder, width, height);
        }
    }
}

impl Gpu {
    pub(super) fn init_rolling(&mut self, world: &World, camera: Camera) -> Result<()> {
        // The existing vegetation and NAADF pipelines share these shadow
        // layouts. No adaptive geometry jobs or mesh draws run in this mode.
        self.adaptive = Some(crate::adaptive_gpu::AdaptiveGpu::new(
            &self.device,
            self.scene_format,
            self.sample_count,
        ));
        let rolling = RollingTerrain::new(self, world, self.visual_settings.as_ref().unwrap())?;
        self.vegetation = None; // The plant pipeline samples resources owned by this rolling cache.
        self.rolling = Some(rolling);
        self.update_rolling(world, camera)
    }

    pub(super) fn update_rolling(&mut self, world: &World, camera: Camera) -> Result<()> {
        if let Some(rolling) = &mut self.rolling {
            let settings = self
                .visual_settings
                .as_ref()
                .expect("rolling visual settings");
            rolling.update(
                &self.device,
                &self.queue,
                world,
                camera,
                &settings.terrain_lod_curve,
            )?;
        }
        let gray = self
            .visual_settings
            .as_ref()
            .is_some_and(|s| s.terrain_gray);
        if let Some(mesh) = self.formation_mesh.update(world, gray)? {
            eprintln!("Voxel formation mesh: {} triangles", mesh.indices.len() / 3);
            self.replace_mesh(&mesh);
        }
        Ok(())
    }
}
