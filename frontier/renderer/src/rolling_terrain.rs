//! Playable integration of the cached-heightfield experiment.
use crate::{settings::VisualSettings, Camera, Gpu, Result};
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
const MIN_NATIVE_SIDE: usize = 512;
pub(crate) const DEFAULT_NAADF_RADIUS: u32 = 128;
pub(crate) const MAX_NAADF_RADIUS: u32 = 512;
pub(crate) const MAX_FAR: f32 = 6000.;
const SOURCE: &str = concat!(include_str!("bin/cached_heightfield.wgsl"),
    "\nfn outdoor_sun_color(elevation:f32)->vec3f {return mix(vec3f(2.12,1.44,.83),vec3f(2.,1.78,1.46),smoothstep(.06,.64,elevation));}\n",
    include_str!("water.wgsl"), "\nconst ROLLING_WATER_GROUP = 0u;\n",
    include_str!("rolling_water_bed.wgsl"),
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
    palette: [[f32; 4]; 10],
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
        let next = side / 2;
        data = (0..next * next)
            .map(|i| {
                let a = 2 * (i / next) * side + 2 * (i % next);
                data[a]
                    .max(data[a + 1])
                    .max(data[a + side])
                    .max(data[a + side + 1])
            })
            .collect();
        side = next;
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
            i32::from(matches!(self.source, Source::Sampled(_))),
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

struct ExactColumns {
    base: [i64; 2],
    datum: i64,
    side: usize,
    revision: Option<u64>,
    sampled: Option<Arc<BaseColumns>>,
    pending: Option<JoinHandle<BaseColumns>>,
    columns: wgpu::Texture,
    maxima: wgpu::Texture,
    runs: wgpu::Buffer,
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
    fn new(device: &wgpu::Device, radius_voxels: u32) -> Self {
        // The 2D fallback surrounds the volume through an entire origin shift.
        let side = Self::side_for(radius_voxels);
        let n = side as u32;
        Self {
            base: [0; 2],
            datum: 0,
            side,
            revision: None,
            sampled: None,
            pending: None,
            columns: texture(device, n, 1, wgpu::TextureFormat::Rgba32Sint, false),
            maxima: texture(
                device,
                n,
                n.ilog2() + 1,
                wgpu::TextureFormat::R32Float,
                false,
            ),
            runs: runs_buffer(device, &[[0; 4]]),
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
        let target = [axis(cell.x)?, axis(cell.z)?];
        let datum = cell.y.div_euclid(256) * 256;
        let needs_window = self.sampled.is_none()
            || [cell.x, cell.z]
                .into_iter()
                .zip(self.base)
                .any(|(value, base)| {
                    !(base + snap / 2..base + side as i64 - snap / 2).contains(&value)
                });
        let terrain = world.fbm_terrain().expect("rolling cache source");
        if (needs_window || self.pending.as_ref().is_some_and(JoinHandle::is_finished))
            && self.pending.is_some()
        {
            let ready = self
                .pending
                .take()
                .unwrap()
                .join()
                .map_err(|_| "rolling voxel worker panicked")?;
            if ready.origin == target {
                self.sampled = Some(Arc::new(ready));
            }
        }
        if needs_window
            && self
                .sampled
                .as_ref()
                .is_none_or(|columns| columns.origin != target)
        {
            self.sampled = Some(Arc::new(BaseColumns::sample(
                terrain,
                side,
                target,
                self.sampled.as_deref(),
            )));
        }
        let sampled = self.sampled.as_ref().unwrap();
        let origin = sampled.origin;
        if origin != target && self.pending.is_none() {
            let previous = Arc::clone(sampled);
            let terrain = terrain.clone();
            self.pending = Some(std::thread::spawn(move || {
                BaseColumns::sample(&terrain, side, target, Some(&previous))
            }));
        }
        if self.base == origin && self.datum == datum && self.revision == Some(world.revision()) {
            return Ok(false);
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
        self.base = origin;
        self.datum = datum;
        self.revision = Some(world.revision());
        Ok(true)
    }
}

pub(crate) struct RollingTerrain {
    outer: Layer,
    inner: Layer,
    exact: ExactColumns,
    view: wgpu::Buffer,
    native_view: wgpu::Buffer,
    water_layout: wgpu::BindGroupLayout,
    water_bind: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    source: [u32; 4],
    bind: Option<wgpu::BindGroup>,
}
impl RollingTerrain {
    pub fn new(gpu: &Gpu, world: &World, settings: &VisualSettings) -> Result<Self> {
        let device = &gpu.device;
        let terrain = world
            .fbm_terrain()
            .ok_or("rolling terrain requires a procedural terrain source")?;
        let alpine = terrain.params()
            == (FbmParams {
                recipe: FbmRecipe::Alpine,
                ..Default::default()
            })
            && device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        if !alpine
            && terrain.params().recipe == FbmRecipe::Alpine
            && !device.features().contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            eprintln!(
                "rolling terrain: GPU strip timing unavailable, using canonical CPU sampling"
            );
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rolling terrain composition"),
            source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
        });
        let bindings =
            [0, 1, 2, 6, 7, 9, 10, 11, 12, 13, 14].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                count: None,
                ty: if matches!(binding, 1 | 2 | 6 | 7 | 10 | 11 | 13 | 14) {
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
            });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rolling terrain layout"),
            entries: &bindings,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("meshless terrain and voxel column depth"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("rolling_triangle"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("rolling_fragment"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &HashMap::from([
                        ("METHOD".into(), 2.),
                        ("BILINEAR".into(), 1.),
                        ("STREAM".into(), f64::from(u8::from(alpine))),
                        ("CANONICAL_PIGMENT".into(), f64::from(u8::from(!alpine))),
                        ("NESTED".into(), 1.),
                    ]),
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
        );
        let native_view = uniform(bytemuck::bytes_of(&NativeView::zeroed()));
        let water_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rolling water bed for NAADF"),
            entries: &[bindings[5], bindings[7]],
        });
        let water_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rolling water bed for NAADF"),
            layout: &water_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: native_view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(
                        &exact.columns.create_view(&Default::default()),
                    ),
                },
            ],
        });
        Ok(Self {
            outer: if alpine {
                Layer::new(device, OUTER, 8., terrain, true)
            } else {
                Layer::new(device, 1024, 24., terrain, false)
            },
            inner: Layer::new(
                device,
                INNER,
                if alpine { 0.5 } else { 1.5 },
                terrain,
                alpine,
            ),
            exact,
            view: uniform(bytemuck::bytes_of(&View::zeroed())),
            native_view,
            water_layout,
            water_bind,
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
    ) -> Result<()> {
        let eye = DVec2::new(camera.position.x, camera.position.z);
        self.outer.update(device, queue, eye)?;
        self.inner.update(device, queue, eye)?;
        if self.exact.update(device, queue, world, camera.position)? || self.bind.is_none() {
            let maxima = self.exact.maxima.create_view(&Default::default());
            let columns = self.exact.columns.create_view(&Default::default());
            self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rolling terrain resources"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
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
                ],
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
    pub fn fits_naadf_radius(&self, settings: &VisualSettings) -> bool {
        !settings.naadf_enabled
            || self.exact.side >= ExactColumns::side_for(settings.naadf_radius_voxels)
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
                                * 4
                        })
                        .sum::<usize>()
                })
                .sum::<usize>()
        };
        let exact_side = self.exact.side;
        layer_bytes(&self.outer)
            + layer_bytes(&self.inner)
            + exact_side * exact_side * 16
            + (exact_side * exact_side * 4 / 3) * 4
            + self.exact.runs.size() as usize
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
    ) {
        let forward = camera.direction();
        let scale = (camera.fov_y.to_radians() * 0.5).tan();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward).normalize();
        let view = View {
            eye: camera.position.as_vec3().extend(camera.far).to_array(),
            forward: forward.extend(0.).to_array(),
            right: (right * scale * width as f32 / height as f32)
                .extend(0.)
                .to_array(),
            up: (up * scale).extend(0.).to_array(),
            grid: self.outer.grid(),
            cache: self.outer.offset(),
            size: [width, height, self.outer.side.ilog2(), 0],
            inner: self.inner.grid(),
            inner_cache: self.inner.offset(),
            unused_history: [[0.; 4]; 4],
        };
        queue.write_buffer(&self.view, 0, bytemuck::bytes_of(&view));
        let [lo, hi] = owner
            .unwrap_or([[0.; 3]; 2])
            .map(|v| glam::DVec3::from_array(v).as_vec3());
        let sun = if settings.beauty {
            crate::atmosphere::sun_direction(settings.sun_elevation, settings.sun_azimuth)
        } else {
            Vec3::new(-0.5, 0.8, -0.3).normalize()
        };
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
            owner_max: hi.extend(0.).to_array(),
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
                u32::from(ownership_probe),
            ],
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
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(bind) = &self.bind {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
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
        self.rolling = Some(RollingTerrain::new(
            self,
            world,
            self.visual_settings.as_ref().unwrap(),
        )?);
        self.update_rolling(world, camera)
    }

    pub(super) fn update_rolling(&mut self, world: &World, camera: Camera) -> Result<()> {
        if let Some(rolling) = &mut self.rolling {
            rolling.update(&self.device, &self.queue, world, camera)?;
        }
        Ok(())
    }
}
