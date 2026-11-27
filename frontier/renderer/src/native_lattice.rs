//! Sliding GPU window over the world's fixed native-column generator.
use crate::{Camera, Result};
use bytemuck::{Pod, Zeroable};
use frontier_world::{
    native_lattice::{self, detail, DRY_HEAD_DROP, PITCH_CELLS, RELIEF_MAX},
    FbmTerrain, Material, World, VOXEL_SIZE,
};
use glam::Vec3Swizzles;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    thread::JoinHandle,
};
use wgpu::util::DeviceExt;

pub(crate) const SHADER: &str = include_str!("native_lattice.wgsl");
pub(crate) const MAX_FAR: f32 = 6000.;
const SIDE: usize = 2048;
const PITCH_M: f64 = PITCH_CELLS as f64 * VOXEL_SIZE;
const RECENTER_CELLS: i64 = 128 * PITCH_CELLS;
const ROOT: u32 = SIDE.ilog2();
const WET: i32 = 1 << 30;
const RELIEF: i32 = i32::MIN;
const EDITED_ROOF: i32 = 1 << 30;
const HEIGHT_MASK: i32 = 0x00ff_ffff;
const HEAD_MASK: i32 = 0x003f_ffff;
const MAX_BEAM_EXTENT: u32 = 4096;
const MAX_COMPUTE_PIXELS: u32 = 2_097_152;

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct View {
    eye: [f32; 4],
    origin: [i32; 4],
    config: [u32; 4],
    eye_cell: [i32; 4],
    eye_fraction: [f32; 4],
}

struct Window {
    origin: [i64; 2],
    controls: Vec<[i32; 2]>,
    roofs: Vec<i32>,
    generated: usize,
}

fn storage(device: &wgpu::Device, label: &str, data: &[u8]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: data,
        usage: wgpu::BufferUsages::STORAGE,
    })
}

fn height_bits(height: i64) -> i32 {
    assert!((-(1 << 23)..1 << 23).contains(&height));
    height as i32 & HEIGHT_MASK
}

fn unpack_height(packed: i32) -> i32 {
    (packed << 8) >> 8
}

fn pack_head(delta: i64) -> i32 {
    assert!((-(1 << 21)..1 << 21).contains(&delta));
    delta as i32 & HEAD_MASK
}

fn roof_offset(n: usize) -> usize {
    (n * n - 1) / 3
}

impl Window {
    fn sample(terrain: &FbmTerrain, origin: [i64; 2], previous: Option<&Self>) -> Self {
        let width = SIDE + 1;
        let mut controls = vec![[0; 2]; width * width];
        let shift = previous.map(|old| [0, 1].map(|i| (origin[i] - old.origin[i]) / PITCH_CELLS));
        let detailed = terrain.native_detail_enabled();
        let seed = native_lattice::relief_seed(terrain.seed());
        let rows = width.div_ceil(8);
        std::thread::scope(|scope| {
            for (chunk, block) in controls.chunks_mut(rows * width).enumerate() {
                scope.spawn(move || {
                    for (index, sample) in block.iter_mut().enumerate() {
                        let x = (index % width) as i64;
                        let z = (chunk * rows + index / width) as i64;
                        if let Some((old, [dx, dz])) = previous.zip(shift) {
                            let [x, z] = [x + dx, z + dz];
                            if (0..width as i64).contains(&x) && (0..width as i64).contains(&z) {
                                *sample = old.controls[z as usize * width + x as usize];
                                continue;
                            }
                        }
                        let point = [origin[0] + x * PITCH_CELLS, origin[1] + z * PITCH_CELLS];
                        let c = terrain.lattice_control(point);
                        let wet = c.visible_top > c.ground_top;
                        let head = if wet {
                            c.visible_top - c.ground_top
                        } else {
                            -DRY_HEAD_DROP
                        };
                        let flags = (if wet { WET } else { 0 })
                            | (if terrain.lattice_relief_at(point) {
                                RELIEF
                            } else {
                                0
                            });
                        let tile = if detailed {
                            detail::tile_index(point.map(|v| v / PITCH_CELLS), seed) as i32
                        } else {
                            0
                        };
                        *sample = [
                            height_bits(c.ground_top) | (tile << 24),
                            pack_head(head)
                                | ((c.ground_surface as i32) << 22)
                                | ((c.ground_subsurface as i32) << 26)
                                | flags,
                        ];
                    }
                });
            }
        });
        let mut roofs = vec![0; (4 * SIDE * SIDE - 1) / 3];
        let offset = roof_offset(SIDE);
        for z in 0..SIDE {
            for x in 0..SIDE {
                let i = z * width + x;
                let corners = [
                    controls[i],
                    controls[i + 1],
                    controls[i + width],
                    controls[i + width + 1],
                ];
                let margin = if corners[0][1] & RELIEF != 0 {
                    if detailed {
                        detail::roof((corners[0][0] as u32 >> 24) as usize) as i32
                    } else {
                        RELIEF_MAX as i32
                    }
                } else {
                    0
                };
                let mut top = i32::MIN;
                for [control, packed] in corners {
                    let ground = unpack_height(control);
                    top = top.max(ground + margin);
                    if packed & WET != 0 {
                        top = top.max(ground + ((packed << 10) >> 10));
                    }
                }
                roofs[offset + z * SIDE + x] = top;
            }
        }
        let mut n = SIDE;
        while n > 1 {
            let p = n / 2;
            let a = roof_offset(n);
            let b = roof_offset(p);
            for z in 0..p {
                for x in 0..p {
                    let i = a + 2 * (z * n + x);
                    roofs[b + z * p + x] = roofs[i]
                        .max(roofs[i + 1])
                        .max(roofs[i + n])
                        .max(roofs[i + n + 1]);
                }
            }
            n = p;
        }
        for roof in &mut roofs[offset..] {
            *roof = height_bits(i64::from(*roof));
        }
        let reused = shift.map_or(0, |[dx, dz]| {
            width.saturating_sub(dx.unsigned_abs() as usize)
                * width.saturating_sub(dz.unsigned_abs() as usize)
        });
        Self {
            origin,
            controls,
            roofs,
            generated: width * width - reused,
        }
    }

    fn covers(&self, xz: [f64; 2], radius: f64) -> bool {
        xz.into_iter().zip(self.origin).all(|(point, base)| {
            let base = base as f64 * VOXEL_SIZE;
            point - radius >= base && point + radius < base + SIDE as f64 * PITCH_M
        })
    }

    fn edit_buffers(
        &self,
        device: &wgpu::Device,
        world: &World,
    ) -> (wgpu::Buffer, wgpu::Buffer, usize) {
        let mut patches = BTreeMap::<i32, BTreeMap<i32, Vec<i64>>>::new();
        for cell in world.edit_cells_in_columns(
            self.origin,
            self.origin.map(|v| v + SIDE as i64 * PITCH_CELLS),
        ) {
            let local = [cell.x - self.origin[0], cell.z - self.origin[1]];
            let [px, pz] = local.map(|p| p / PITCH_CELLS);
            let [x, z] = local.map(|p| p % PITCH_CELLS);
            patches
                .entry((px + pz * SIDE as i64) as i32)
                .or_default()
                .entry((x + z * PITCH_CELLS) as i32)
                .or_default()
                .extend([cell.y, cell.y.saturating_add(1)]);
        }
        let mut data = vec![[0i32; 4]; patches.len() + 1];
        data[0][0] = patches.len() as i32;
        let mut edited_roofs = None;
        for (idx, (patch, columns)) in patches.into_iter().enumerate() {
            let patch_x = patch as i64 % SIDE as i64;
            let patch_z = patch as i64 / SIDE as i64;
            let first = data.len();
            data[idx + 1] = [patch, first as i32, columns.len() as i32, 0];
            data.resize(first + columns.len(), [0; 4]);
            let mut roof = unpack_height(self.roofs[roof_offset(SIDE) + patch as usize]);
            for (col, (local, mut edges)) in columns.into_iter().enumerate() {
                let x = self.origin[0] + patch_x * PITCH_CELLS + i64::from(local) % PITCH_CELLS;
                let z = self.origin[1] + patch_z * PITCH_CELLS + i64::from(local) / PITCH_CELLS;
                let base = world.fbm_terrain().unwrap().column_cells(x, z);
                let floor = i64::from(i32::MIN);
                edges.extend([
                    floor,
                    base.ground_top - 12,
                    base.ground_top - 1,
                    base.ground_top,
                    base.visible_top,
                ]);
                edges.sort_unstable();
                edges.dedup();
                let start = data.len();
                let column = world.column(x, z);
                for span in edges.windows(2) {
                    let material = column.material_without_fluid(span[0]);
                    if material != Material::Air {
                        let top = i32::try_from(span[1]).unwrap();
                        data.push([i32::try_from(span[0]).unwrap(), top, material as i32, 0]);
                        roof = roof.max(top);
                    }
                }
                data[first + col] = [local, start as i32, (data.len() - start) as i32, 0];
            }
            let bounds = edited_roofs.get_or_insert_with(|| self.roofs.clone());
            let [mut x, mut z] = [patch_x as usize, patch_z as usize];
            let leaf = roof_offset(SIDE) + patch as usize;
            bounds[leaf] = height_bits(i64::from(roof)) | EDITED_ROOF;
            let mut n = SIDE / 2;
            while n > 0 {
                x /= 2;
                z /= 2;
                let parent = roof_offset(n) + z * n + x;
                bounds[parent] = bounds[parent].max(roof);
                n /= 2;
            }
        }
        let roofs = edited_roofs.as_deref().unwrap_or(&self.roofs);
        (
            storage(device, "native lattice roofs", bytemuck::cast_slice(roofs)),
            storage(
                device,
                "native lattice sparse column edits",
                bytemuck::cast_slice(&data),
            ),
            data[0][0] as usize,
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) struct NativeLatticeConfig {
    beams: bool,
    compute: bool,
    relief_bounds: bool,
    detailed: bool,
    split: bool,
    detail_step: u32,
    tile_width: u32,
}

impl NativeLatticeConfig {
    pub fn for_source(detailed: bool, msaa: u32) -> Self {
        let flag = |name, default| std::env::var_os(name).map_or(default, |value| value != "0");
        let number = |name| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
        };
        Self {
            beams: flag("FRONTIER_NATIVE_LATTICE_BEAMS", detailed),
            compute: msaa == 1 && flag("FRONTIER_NATIVE_LATTICE_COMPUTE", detailed),
            relief_bounds: flag("FRONTIER_NATIVE_LATTICE_RELIEF_BOUNDS", false),
            detailed,
            split: flag("FRONTIER_NATIVE_DETAIL_SPLIT", detailed),
            detail_step: number("FRONTIER_NATIVE_DETAIL_STEP")
                .filter(|n| *n <= 3)
                .unwrap_or(1),
            tile_width: number("FRONTIER_NATIVE_DETAIL_GROUP")
                .filter(|n| [8, 16, 32, 64].contains(n))
                .unwrap_or(8),
        }
    }

    pub fn shader_constants(self) -> HashMap<String, f64> {
        [
            ("LATTICE_BEAMS", u32::from(self.beams)),
            ("LATTICE_COMPUTE", u32::from(self.compute)),
            ("LATTICE_RELIEF_BOUNDS", u32::from(self.relief_bounds)),
            ("LATTICE_DETAIL", u32::from(self.detailed)),
            ("LATTICE_SPLIT", u32::from(self.split)),
            ("LATTICE_DETAIL_STEP", self.detail_step),
            ("LATTICE_DETAIL_TILE_X", self.tile_width),
        ]
        .map(|(key, value)| (key.to_owned(), f64::from(value)))
        .into()
    }
}

fn detail_binding() -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Uint,
        view_dimension: wgpu::TextureViewDimension::D2Array,
        multisampled: false,
    }
}

fn detail_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    detailed: bool,
) -> (wgpu::TextureView, usize) {
    let side = if detailed { PITCH_CELLS as u32 } else { 1 };
    let layers = if detailed {
        (detail::atlas().len() / detail::NODES) as u32
    } else {
        1
    };
    let mut packed = Vec::with_capacity(if detailed { detail::atlas().len() } else { 1 });
    if detailed {
        for tile in detail::atlas().chunks_exact(detail::NODES) {
            for mip in 0..=side.ilog2() {
                let width = (side >> mip) as usize;
                let first = roof_offset(width);
                packed.extend_from_slice(&tile[first..first + width * width]);
            }
        }
    } else {
        packed.push(0);
    }
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("native relief atlas"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: layers,
            },
            mip_level_count: side.ilog2() + 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&packed),
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    (view, packed.len() * std::mem::size_of::<u16>())
}

pub(crate) struct NativeLattice {
    window: Option<Arc<Window>>,
    pending: Option<JoinHandle<Arc<Window>>>,
    view: wgpu::Buffer,
    prefix: wgpu::Buffer,
    beam_pipeline: Option<wgpu::ComputePipeline>,
    beam_bind: Option<wgpu::BindGroup>,
    results: wgpu::Buffer,
    detail: wgpu::TextureView,
    detail_bytes: usize,
    config: NativeLatticeConfig,
    visibility: Option<wgpu::ComputePipeline>,
    resolve: Option<wgpu::ComputePipeline>,
    visibility_bind: Option<wgpu::BindGroup>,
    resolve_bind: Option<wgpu::BindGroup>,
    buffers: Option<[wgpu::Buffer; 3]>,
    revision: Option<u64>,
    seed: u32,
    publications: u64,
    generated: usize,
    edited: usize,
}

impl NativeLattice {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        module: &wgpu::ShaderModule,
        seed: u64,
        config: NativeLatticeConfig,
    ) -> Self {
        let NativeLatticeConfig {
            beams,
            compute,
            detailed,
            ..
        } = config;
        let pipeline =
            |entry: &str, constants: &std::collections::HashMap<String, f64>, bindings: &[u32]| {
                let entries = bindings
                    .iter()
                    .map(|&binding| wgpu::BindGroupLayoutEntry {
                        binding,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: if binding == 40 {
                            detail_binding()
                        } else {
                            wgpu::BindingType::Buffer {
                                ty: if matches!(binding, 0 | 32) {
                                    wgpu::BufferBindingType::Uniform
                                } else {
                                    wgpu::BufferBindingType::Storage {
                                        read_only: !matches!(binding, 37 | 39),
                                    }
                                },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            }
                        },
                        count: None,
                    })
                    .collect::<Vec<_>>();
                let resources = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(entry),
                    entries: &entries,
                });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(entry),
                    bind_group_layouts: &[&resources],
                    push_constant_ranges: &[],
                });
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&layout),
                    module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        ..Default::default()
                    },
                    cache: None,
                })
            };
        let (detail, detail_bytes) = detail_texture(device, queue, detailed);
        Self {
            window: None,
            pending: None,
            view: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("native lattice view"),
                size: std::mem::size_of::<View>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            prefix: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("native lattice shared ray intervals"),
                size: 4 * u64::from(if beams {
                    (MAX_BEAM_EXTENT / 8).pow(2)
                } else {
                    1
                }),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            beam_pipeline: beams
                .then(|| pipeline("lattice_find_prefix", &Default::default(), &[0, 32, 34, 37])),
            results: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("native lattice full-resolution hit buffer"),
                size: 8 * u64::from(if compute { MAX_COMPUTE_PIXELS } else { 1 }),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            detail,
            detail_bytes,
            config,
            beam_bind: None,
            visibility: compute.then(|| {
                pipeline(
                    "lattice_visibility",
                    &config.shader_constants(),
                    &[0, 32, 33, 34, 35, 36, 39, 40],
                )
            }),
            resolve: (compute && config.split).then(|| {
                pipeline(
                    "lattice_resolve",
                    &config.shader_constants(),
                    &[0, 32, 33, 34, 35, 36, 39, 40],
                )
            }),
            visibility_bind: None,
            resolve_bind: None,
            buffers: None,
            revision: None,
            seed: native_lattice::relief_seed(seed),
            publications: 0,
            generated: 0,
            edited: 0,
        }
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        world: &World,
        camera: Camera,
        rolling_view: &wgpu::Buffer,
    ) -> Result<bool> {
        let xz = camera.position.xz().to_array();
        let far = f64::from(camera.far);
        let origin = xz.map(|p| {
            ((p / VOXEL_SIZE).floor() as i64).div_euclid(RECENTER_CELLS) * RECENTER_CELLS
                - SIDE as i64 / 2 * PITCH_CELLS
        });
        for cell in origin {
            i32::try_from(cell)?;
            i32::try_from(cell + SIDE as i64 * PITCH_CELLS)?;
        }
        let terrain = world.fbm_terrain().unwrap();
        let mut ready = None;
        if self.pending.as_ref().is_some_and(JoinHandle::is_finished) {
            let next = self
                .pending
                .take()
                .unwrap()
                .join()
                .map_err(|_| "native lattice worker panicked")?;
            if next.covers(xz, far) {
                ready = Some(next);
            }
        }
        if ready.is_none() && self.window.as_ref().is_none_or(|w| !w.covers(xz, far)) {
            ready = Some(Arc::new(Window::sample(
                terrain,
                origin,
                self.window.as_deref(),
            )));
        }
        let fresh = ready.is_some();
        if let Some(ready) = ready {
            self.publications += 1;
            self.generated += ready.generated;
            self.window = Some(ready);
        }
        let window = self.window.as_ref().unwrap();
        if self.pending.is_none() && window.origin != origin {
            let terrain = terrain.clone();
            let previous = Arc::clone(window);
            self.pending = Some(std::thread::spawn(move || {
                Arc::new(Window::sample(&terrain, origin, Some(&previous)))
            }));
        }
        if fresh || self.revision != Some(world.revision()) {
            let (roofs, edits, edited) = window.edit_buffers(device, world);
            let controls = if fresh {
                storage(
                    device,
                    "native lattice controls",
                    bytemuck::cast_slice(&window.controls),
                )
            } else {
                self.buffers.as_ref().unwrap()[0].clone()
            };
            self.beam_bind = self.beam_pipeline.as_ref().map(|pipeline| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("native lattice beam resources"),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: rolling_view.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 32,
                            resource: self.view.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 34,
                            resource: roofs.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 37,
                            resource: self.prefix.as_entire_binding(),
                        },
                    ],
                })
            });
            let visibility_resources = |pipeline: &wgpu::ComputePipeline| {
                let layout = pipeline.get_bind_group_layout(0);
                let buffers = [
                    (0, rolling_view),
                    (32, &self.view),
                    (33, &controls),
                    (34, &roofs),
                    (35, &edits),
                    (36, &self.prefix),
                    (39, &self.results),
                ];
                let mut resources = buffers
                    .into_iter()
                    .map(|(binding, buffer)| wgpu::BindGroupEntry {
                        binding,
                        resource: buffer.as_entire_binding(),
                    })
                    .collect::<Vec<_>>();
                resources.push(wgpu::BindGroupEntry {
                    binding: 40,
                    resource: wgpu::BindingResource::TextureView(&self.detail),
                });
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("native lattice visibility resources"),
                    layout: &layout,
                    entries: &resources,
                })
            };
            self.visibility_bind = self.visibility.as_ref().map(visibility_resources);
            self.resolve_bind = self.resolve.as_ref().map(visibility_resources);
            self.buffers = Some([controls, roofs, edits]);
            self.revision = Some(world.revision());
            self.edited = edited;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn prepare(&self, queue: &wgpu::Queue, camera: Camera) {
        let [x, z] = self.window.as_ref().unwrap().origin;
        let eye = [
            camera.position.x / VOXEL_SIZE - x as f64,
            camera.position.z / VOXEL_SIZE - z as f64,
        ];
        let cell = eye.map(f64::floor);
        let data = View {
            eye: [
                (camera.position.x - x as f64 * VOXEL_SIZE) as f32,
                camera.position.y as f32,
                (camera.position.z - z as f64 * VOXEL_SIZE) as f32,
                camera.far,
            ],
            origin: [x as i32, z as i32, self.seed as i32, 0],
            config: [SIDE as u32, ROOT, 0, 0],
            eye_cell: [cell[0] as i32, cell[1] as i32, 0, 0],
            eye_fraction: [(eye[0] - cell[0]) as f32, (eye[1] - cell[1]) as f32, 0., 0.],
        };
        queue.write_buffer(&self.view, 0, bytemuck::bytes_of(&data));
    }

    pub fn layout() -> [wgpu::BindGroupLayoutEntry; 7] {
        [32, 33, 34, 35, 36, 38, 40].map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: if binding == 40 {
                detail_binding()
            } else {
                wgpu::BindingType::Buffer {
                    ty: if binding == 32 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: true }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }
            },
            count: None,
        })
    }

    pub fn bindings(&self) -> [wgpu::BindGroupEntry<'_>; 7] {
        let [controls, roofs, edits] = self.buffers.as_ref().unwrap();
        let buffers = [
            &self.view,
            controls,
            roofs,
            edits,
            &self.prefix,
            &self.results,
        ];
        std::array::from_fn(|i| wgpu::BindGroupEntry {
            binding: [32, 33, 34, 35, 36, 38, 40][i],
            resource: if i == 6 {
                wgpu::BindingResource::TextureView(&self.detail)
            } else {
                buffers[i].as_entire_binding()
            },
        })
    }

    pub fn encode_visibility(&self, encoder: &mut wgpu::CommandEncoder, width: u32, height: u32) {
        if let Some((pipeline, bind)) = self.beam_pipeline.as_ref().zip(self.beam_bind.as_ref()) {
            if width.max(height) <= MAX_BEAM_EXTENT {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("native lattice beam prefix"),
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(width.div_ceil(64), height.div_ceil(64), 1);
            }
        }
        if width.saturating_mul(height) > MAX_COMPUTE_PIXELS {
            return;
        }
        for (pipeline, bind) in [
            self.visibility.as_ref().zip(self.visibility_bind.as_ref()),
            self.resolve.as_ref().zip(self.resolve_bind.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("native lattice visibility"),
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.dispatch_workgroups(
                width.div_ceil(self.config.tile_width),
                height.div_ceil(64 / self.config.tile_width),
                1,
            );
        }
    }

    pub fn bytes(&self) -> usize {
        (self.prefix.size() + self.results.size()) as usize
            + self.detail_bytes
            + self
                .buffers
                .as_ref()
                .map_or(0, |b| b.iter().map(|v| v.size() as usize).sum())
    }

    pub fn report(&self) -> String {
        format!(
            r#"{{"pitch_m":{PITCH_M},"side":{SIDE},"publications":{},"generated_controls":{},"edited_patches":{},"gpu_bytes":{},"beams":{},"compute":{},"relief_atlas_bytes":{},"detail_step":{},"visibility_group_width":{},"split_wet_and_edits":{}}}"#,
            self.publications,
            self.generated,
            self.edited,
            self.bytes(),
            self.beam_pipeline.is_some(),
            self.visibility.is_some(),
            if self.config.detailed {
                self.detail_bytes
            } else {
                0
            },
            self.config.detail_step,
            self.config.tile_width,
            self.resolve.is_some()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mesh, settings::VisualSettings, Gpu};
    use frontier_world::{raycast, Cell, FbmParams, FbmRecipe};
    use glam::{DVec3, Vec3};

    #[test]
    #[ignore = "requires a Vulkan GPU; captures production depth and compares with CPU voxel DDA"]
    fn production_depth_matches_native_world_and_sparse_edits() {
        capture_native_fixture(false, None);
    }

    #[test]
    #[ignore = "requires a Vulkan GPU; compares all-scale terrain with the native voxel DDA"]
    fn production_depth_matches_native_detail_and_sparse_edits() {
        capture_native_fixture(true, None);
    }

    #[test]
    #[ignore = "requires a Vulkan GPU; traces a six-kilometre horizon and a river"]
    fn production_depth_matches_native_detail_on_ridges_and_water() {
        capture_native_fixture(
            true,
            Some((
                DVec3::new(-6688., 1450., 6080.),
                DVec3::new(-5350., 900., 8360.),
                6000.,
                false,
            )),
        );
        capture_native_fixture(
            true,
            Some((
                DVec3::new(-7520.37, 149.02, 6850.07),
                DVec3::new(-7448.28, 128.70, 6912.18),
                256.,
                true,
            )),
        );
    }

    fn capture_native_fixture(detailed: bool, pose: Option<(DVec3, DVec3, f32, bool)>) {
        let source = FbmTerrain::new(
            1685576058315138427,
            FbmParams {
                recipe: FbmRecipe::Eroded,
                ..Default::default()
            },
        )
        .unwrap();
        let source = if detailed {
            source.with_native_detail()
        } else {
            source.with_native_lattice()
        };
        let mut world = World::from_fbm_terrain(source.clone());
        let xz = [-6688.03, 6080.02];
        let z = xz[1] + 34.16;
        let top = source.ground_height_m(xz[0], z);
        let (eye, target, far, water) = pose.unwrap_or((
            DVec3::new(xz[0] - 0.37, top + 22.4, xz[1]),
            DVec3::new(xz[0], top, z),
            256.,
            false,
        ));
        let direction = (target - eye).normalize();
        let camera = Camera {
            position: eye,
            yaw: direction.x.atan2(direction.z) as f32,
            pitch: direction.y.asin() as f32,
            roll: 0.15,
            far,
            fov_y: 43.,
        };
        let settings = VisualSettings {
            view_distance: camera.far,
            native_lattice: true,
            native_detail: detailed,
            capture_terrain_distance: true,
            msaa: 1,
            ..Default::default()
        };
        let mesh = mesh::Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            origin: Cell::default(),
            exposed_native_faces: 0,
        };
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let mut gpu = pollster::block_on(Gpu::new(
            &instance,
            None,
            Some(wgpu::TextureFormat::Rgba32Float),
            &mesh,
            1,
            false,
            false,
        ))
        .unwrap();
        gpu.configure_visuals(&world, &settings);
        gpu.init_rolling(&world, camera).unwrap();

        let width = 65u32;
        let height = 37u32;
        let directory = std::env::temp_dir().join(format!(
            "frontier-lattice-audit-{}-{detailed}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let forward = camera.direction();
        let right = forward.cross(camera.view_up()).normalize();
        let up = right.cross(forward).normalize();
        let fov_scale = (camera.fov_y.to_radians() * 0.5).tan();
        let right = right * fov_scale * width as f32 / height as f32;
        let up = up * fov_scale;
        let ray = |x: u32, y: u32| -> Vec3 {
            let u = (x as f32 + 0.5) * 2. / width as f32 - 1.;
            let v = (y as f32 + 0.5) * 2. / height as f32 - 1.;
            forward + right * u - up * v
        };

        let check = |gpu: &Gpu, world: &World, label: &str| -> f32 {
            let path = directory.join(format!("{label}.png"));
            gpu.capture(world, &camera, &path, width, height).unwrap();
            let bytes = std::fs::read(path.with_extension("linear_rgb.f32")).unwrap();
            let coverage = std::fs::read(path.with_extension("coverage.f32")).unwrap();
            let mut hits = 0;
            let mut wet_hits = 0;
            let mut worst = 0.0f64;
            let pixel_distance = |x: u32, y: u32| {
                let start = (y * width + x) as usize;
                f32::from_le_bytes(bytes[start * 12..start * 12 + 4].try_into().unwrap())
            };
            for y in (0..height).step_by(3).chain([height / 2]) {
                for x in (0..width).step_by(3).chain([width / 2]) {
                    let i = (y * width + x) as usize;
                    let present =
                        f32::from_le_bytes(coverage[i * 4..i * 4 + 4].try_into().unwrap()) == 1.;
                    let cpu = raycast(
                        world,
                        eye.to_array(),
                        ray(x, y).as_dvec3().to_array(),
                        f64::from(far),
                    );
                    assert_eq!(present, cpu.is_some(), "{label} coverage at ({x},{y})");
                    if let Some(hit) = cpu {
                        let error = (f64::from(pixel_distance(x, y)) - hit.distance).abs();
                        worst = worst.max(error);
                        hits += 1;
                        wet_hits += usize::from(hit.material == Material::Water);
                        assert!(
                            error < 0.025,
                            "{label} depth at ({x},{y}) differs by {error:.5} m"
                        );
                    }
                }
            }
            eprintln!(
                "{label}: {hits} CPU DDA hits ({wet_hits} wet), maximum depth error {worst:.5} m"
            );
            assert!(hits > 100);
            if water {
                assert!(wet_hits > 0);
            }
            pixel_distance(width / 2, height / 2)
        };

        let original = check(&gpu, &world, "unedited");
        if pose.is_some() {
            std::fs::remove_dir_all(directory).unwrap();
            return;
        }
        let center = raycast(
            &world,
            eye.to_array(),
            camera.direction().as_dvec3().to_array(),
            f64::from(far),
        )
        .expect("CPU center ray hits ground");
        assert!(!gpu.rolling.as_ref().unwrap().owns_column(center.cell));
        let p = center.cell;
        let mut cuts = Vec::new();
        for dz in -10..=10 {
            for dx in -10..=10 {
                for dy in -12..=8 {
                    cuts.push((
                        Cell {
                            x: p.x + dx,
                            y: p.y + dy,
                            z: p.z + dz,
                        },
                        Material::Air,
                    ));
                }
            }
        }
        world.set_batch(cuts);
        gpu.update_rolling(&world, camera).unwrap();
        let carved = check(&gpu, &world, "carved");
        assert!(carved > original + 0.2);
        let hit = eye + camera.direction().as_dvec3().normalize() * (f64::from(original) - 3.);
        let p = Cell::from_meters(hit.to_array());
        let raised = (-3..=3).flat_map(|z| {
            (-3..=3).map(move |x| {
                (
                    Cell {
                        x: p.x + x,
                        y: p.y,
                        z: p.z + z,
                    },
                    Material::Stone,
                )
            })
        });
        world.set_batch(raised);
        gpu.update_rolling(&world, camera).unwrap();
        assert!(check(&gpu, &world, "placed") < original - 2.);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
