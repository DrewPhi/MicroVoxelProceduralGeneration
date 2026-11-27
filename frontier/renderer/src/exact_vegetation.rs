//! Resident native vegetation experiment. The BVH bounds empty space only.
use super::{Camera, Gpu, Result};
use bytemuck::{Pod, Zeroable};
use frontier_world::{
    vegetation::{model_for, Species, MAX_PLANT_RADIUS_CELLS},
    voxel_size, Cell, World,
};
use glam::DVec3;
use std::{collections::HashMap, time::Instant};
use wgpu::util::DeviceExt;

const EDGE: i32 = 4;
const LEAF: usize = 4;
const MIXED_NONE: u32 = u32::MAX;
const CENTER_STEP_M: f64 = 8.;
const MARGIN_M: f64 = 12.;
pub(super) const MAX_TRIAL_HORIZON_M: f32 = 256.;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Node {
    min: [i32; 3],
    right: u32,
    max: [i32; 3],
    end: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Brick {
    min: [i32; 3],
    pigment: u32,
    mask: [u32; 2],
    bounds: u32,
    padding: u32,
}

struct Scene {
    origin: [i64; 3],
    radius_m: f64,
    nodes: Vec<Node>,
    bricks: Vec<Brick>,
    pigments: Vec<u32>,
    occupied: usize,
    uniform_bricks: usize,
    palette_entries: usize,
    plants: [usize; 3],
    build_ms: f64,
}

fn address(cell: [i32; 3]) -> ([i32; 3], usize) {
    let brick = cell.map(|v| v.div_euclid(EDGE) * EDGE);
    let [x, y, z] = cell.map(|v| v.rem_euclid(EDGE) as usize);
    (brick, x + 4 * y + 16 * z)
}

fn pack(color: [f32; 3]) -> u32 {
    let [r, g, b] = color.map(|v| (v.clamp(0., 1.).sqrt() * 255.).round() as u32);
    0xff00_0000 | b << 16 | g << 8 | r
}

// Most bricks contain one native pigment. Only multicolor bricks allocate
// per-cell colors while building, and only occupied colors reach the GPU.
struct BrickBuilder {
    mask: u64,
    primary: u32,
    mixed: u32,
}

impl Default for BrickBuilder {
    fn default() -> Self {
        Self {
            mask: 0,
            primary: 0,
            mixed: MIXED_NONE,
        }
    }
}

impl BrickBuilder {
    fn insert(&mut self, bit: usize, color: [f32; 3], mixed: &mut Vec<[u32; 64]>) {
        let flag = 1u64 << bit;
        if self.mask & flag != 0 {
            return;
        }
        let color = pack(color);
        if self.mask == 0 {
            self.primary = color;
        } else if self.mixed == MIXED_NONE && self.primary != color {
            self.mixed = mixed.len() as u32;
            mixed.push([self.primary; 64]);
        }
        if self.mixed != MIXED_NONE {
            mixed[self.mixed as usize][bit] = color;
        }
        self.mask |= flag;
    }
}

fn brick_bounds(mask: u64) -> u32 {
    let layer_masks = [
        0x1111_1111_1111_1111u64,
        0x000f_000f_000f_000f,
        0x0000_0000_0000_ffff,
    ];
    let mut bounds = 0;
    for (axis, layer) in layer_masks.into_iter().enumerate() {
        let shift = 1 << (axis * 2);
        let lo = (0..4).find(|i| mask & (layer << (i * shift)) != 0).unwrap();
        let hi = (0..4)
            .rev()
            .find(|i| mask & (layer << (i * shift)) != 0)
            .unwrap();
        bounds |= lo << (axis * 2) | hi << (6 + axis * 2);
    }
    bounds
}

fn pigment_stream(raw: &[u32], indexed: bool) -> (Vec<u32>, usize) {
    if indexed {
        let mut lookup = HashMap::new();
        let mut palette = Vec::new();
        let mut stream = vec![0];
        for pair in raw.chunks(2) {
            let mut word = 0;
            for (i, &color) in pair.iter().enumerate() {
                let next = lookup.len();
                let index = *lookup.entry(color).or_insert(next);
                if index > u16::MAX as usize {
                    return (raw_stream(raw), 0);
                }
                if index == palette.len() {
                    palette.push(color);
                }
                word |= (index as u32) << (16 * i);
            }
            stream.push(word);
        }
        if !palette.is_empty() && stream.len() + palette.len() < raw.len() + 1 {
            stream[0] = stream.len() as u32;
            let count = palette.len();
            stream.extend(palette);
            return (stream, count);
        }
    }
    (raw_stream(raw), 0)
}

fn raw_stream(raw: &[u32]) -> Vec<u32> {
    let mut stream = Vec::with_capacity(1 + raw.len());
    stream.push(0);
    stream.extend(raw);
    stream
}

fn build_bvh(bricks: &mut [Brick], first: usize, nodes: &mut Vec<Node>) {
    let mut min = [i32::MAX; 3];
    let mut max = [i32::MIN; 3];
    for brick in bricks.iter() {
        for k in 0..3 {
            let lo = ((brick.bounds >> (k * 2)) & 3) as i32;
            let hi = ((brick.bounds >> (6 + k * 2)) & 3) as i32 + 1;
            min[k] = min[k].min(brick.min[k] + lo);
            max[k] = max[k].max(brick.min[k] + hi);
        }
    }
    let index = nodes.len();
    nodes.push(Node {
        min,
        max,
        right: 0,
        end: 0,
    });
    if bricks.len() <= LEAF {
        nodes[index].right = first as u32;
        nodes[index].end = (first + bricks.len()) as u32 | 0x8000_0000;
        return;
    }
    let axis = (0..3).max_by_key(|&k| max[k] - min[k]).unwrap();
    let middle = bricks.len() / 2;
    bricks.select_nth_unstable_by_key(middle, |brick| (brick.min[axis], brick.min));
    let (left, right) = bricks.split_at_mut(middle);
    build_bvh(left, first, nodes);
    nodes[index].right = nodes.len() as u32;
    build_bvh(right, first + middle, nodes);
    nodes[index].end = nodes.len() as u32;
}

impl Scene {
    fn build(world: &World, camera: Camera, cpu_budget: usize) -> Result<Self> {
        let start = Instant::now();
        let voxel = voxel_size();
        let origin = camera.position.to_array().map(|a| {
            ((a / CENTER_STEP_M).round() * CENTER_STEP_M / (voxel * f64::from(EDGE))).floor() as i64
                * i64::from(EDGE)
        });
        let radius_m = f64::from(camera.far) + MARGIN_M;
        let radius = (radius_m / voxel).ceil() as i64;
        let lower = origin.map(|axis| axis - radius);
        let upper = origin.map(|axis| axis + radius + 1);
        let min = [lower[0], lower[2]];
        let max = [upper[0], upper[2]];

        let mut cells: HashMap<[i32; 3], BrickBuilder> = HashMap::new();
        let mut mixed = Vec::new();
        let mut plants = [0; 3];
        // Same overlap priority as World::vegetation_voxel.
        for (kind, species) in [Species::Rowan, Species::Grass, Species::Edelweiss]
            .into_iter()
            .enumerate()
        {
            let halo = if species == Species::Rowan {
                MAX_PLANT_RADIUS_CELLS
            } else {
                8
            };
            for record in world.vegetation_records_species(
                min.map(|a| a - halo),
                max.map(|a| a + halo),
                species,
            ) {
                let (_, height) = record.extent_cells();
                if record.root.y >= upper[1] || record.root.y + height < lower[1] {
                    continue;
                }
                plants[kind] += 1;
                for v in &model_for(&record).voxels {
                    let c = record.world_cell(v.cell);
                    if c.x < min[0]
                        || c.x >= max[0]
                        || c.z < min[1]
                        || c.z >= max[1]
                        || c.y < lower[1]
                        || c.y >= upper[1]
                    {
                        continue;
                    }
                    let (key, bit) = address(relative(c, origin));
                    cells
                        .entry(key)
                        .or_default()
                        .insert(bit, v.color, &mut mixed);
                }
                // Includes hash-table capacity, pigments, brick/node temporaries
                // and conversion headroom. Reject the scene before publishing.
                if cells.capacity() * 112 + mixed.capacity() * 512 > cpu_budget {
                    return Err("exact vegetation exceeds the CPU brick budget; reduce the shared view distance or increase --plant-cpu-mib".into());
                }
            }
        }
        let mut bricks = Vec::with_capacity(cells.len());
        let mut pigments = Vec::new();
        let mut occupied = 0;
        let mut uniform_bricks = 0;
        for (min, builder) in cells {
            occupied += builder.mask.count_ones() as usize;
            let pigment = if builder.mixed == MIXED_NONE {
                uniform_bricks += 1;
                builder.primary
            } else {
                let offset = pigments.len() as u32;
                let colors = &mixed[builder.mixed as usize];
                let mut remaining = builder.mask;
                while remaining != 0 {
                    pigments.push(colors[remaining.trailing_zeros() as usize]);
                    remaining &= remaining - 1;
                }
                offset
            };
            bricks.push(Brick {
                min,
                pigment,
                mask: [builder.mask as u32, (builder.mask >> 32) as u32],
                bounds: brick_bounds(builder.mask),
                padding: 0,
            });
        }
        drop(mixed);
        let indexed =
            std::env::var_os("FRONTIER_EXACT_VEGETATION_PALETTE").is_none_or(|v| v != "0");
        let (pigments, palette_entries) = pigment_stream(&pigments, indexed);
        let mut nodes = Vec::new();
        if !bricks.is_empty() {
            build_bvh(&mut bricks, 0, &mut nodes);
        }
        Ok(Self {
            origin,
            radius_m,
            nodes,
            bricks,
            pigments,
            occupied,
            uniform_bricks,
            palette_entries,
            plants,
            build_ms: start.elapsed().as_secs_f64() * 1000.,
        })
    }
}

fn relative(cell: Cell, origin: [i64; 3]) -> [i32; 3] {
    [
        (cell.x - origin[0]) as i32,
        (cell.y - origin[1]) as i32,
        (cell.z - origin[2]) as i32,
    ]
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct View {
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    sun: [f32; 4],
    clip: [f32; 4],
    counts: [u32; 4],
}

struct Buffers {
    nodes: wgpu::Buffer,
    bricks: wgpu::Buffer,
    pigments: wgpu::Buffer,
    bytes: usize,
}

impl Buffers {
    fn upload(gpu: &Gpu, scene: &Scene, budget: usize) -> Result<Self> {
        let parts = [
            bytemuck::cast_slice(&scene.nodes),
            bytemuck::cast_slice(&scene.bricks),
            bytemuck::cast_slice(&scene.pigments),
        ];
        let sizes = parts.map(|part| part.len().max(32));
        let limits = gpu.device.limits();
        let bytes = sizes.iter().sum();
        if bytes > budget
            || sizes.into_iter().any(|size| {
                size as u64 > limits.max_buffer_size
                    || size as u64 > u64::from(limits.max_storage_buffer_binding_size)
            })
        {
            return Err("exact vegetation exceeds the GPU brick budget; reduce the shared view distance or increase --plant-gpu-mib".into());
        }
        Ok(Self {
            nodes: storage(gpu, "exact vegetation BVH", parts[0]),
            bricks: storage(gpu, "exact vegetation bricks", parts[1]),
            pigments: storage(gpu, "exact vegetation pigments", parts[2]),
            bytes,
        })
    }
}

fn storage(gpu: &Gpu, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    gpu.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: if bytes.is_empty() { &[0; 32] } else { bytes },
            usage: wgpu::BufferUsages::STORAGE,
        })
}

pub(super) struct ExactVegetation {
    uniform: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    depth_layout: wgpu::BindGroupLayout,
    empty_depth: wgpu::BindGroup,
    occlusion: bool,
    used_terrain_depth: std::cell::Cell<bool>,
    buffers: Buffers,
    origin: [i64; 3],
    radius_m: f64,
    node_count: u32,
    brick_count: usize,
    occupied: usize,
    uniform_bricks: usize,
    palette_entries: usize,
    plants: [usize; 3],
    build_ms: f64,
    revision: u64,
    revision_valid: bool,
    edit_count: u32,
    edits: wgpu::Buffer,
    threaded: bool,
}

fn bind(
    gpu: &Gpu,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    buffers: &Buffers,
    edits: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let bindings = [
        uniform,
        &buffers.nodes,
        &buffers.bricks,
        &buffers.pigments,
        edits,
    ];
    let entries = [0, 1, 2, 3, 4].map(|binding| wgpu::BindGroupEntry {
        binding,
        resource: bindings[binding as usize].as_entire_binding(),
    });
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("exact vegetation scene"),
        layout,
        entries: &entries,
    })
}

fn terrain_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    hits: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("exact vegetation terrain occluders"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: hits.as_entire_binding(),
        }],
    })
}

impl ExactVegetation {
    pub(super) fn update(
        old: Option<Self>,
        gpu: &Gpu,
        world: &World,
        camera: Camera,
    ) -> Result<Self> {
        let settings = gpu.visual_settings.as_ref().unwrap();
        if camera.far > MAX_TRIAL_HORIZON_M {
            return Err(
                "exact vegetation trial requires a shared horizon up to 256 m (--capture-far)"
                    .into(),
            );
        }
        let mut state = if let Some(mut state) = old {
            let movement_m = state
                .eye(camera)
                .into_iter()
                .map(f64::abs)
                .fold(0., f64::max)
                * voxel_size();
            if movement_m + f64::from(camera.far) > state.radius_m {
                let scene = Scene::build(world, camera, settings.plant_cpu_mib << 20)?;
                state.buffers = Buffers::upload(gpu, &scene, settings.plant_gpu_mib << 20)?;
                state.install(&scene);
                state.revision_valid = false;
            }
            state
        } else {
            let scene = Scene::build(world, camera, settings.plant_cpu_mib << 20)?;
            Self::new(gpu, &scene, settings.plant_gpu_mib << 20)?
        };
        if !state.revision_valid || state.revision != world.revision() {
            let mut edited: Vec<_> = world
                .terrain_edits()
                .filter_map(|(cell, _)| {
                    let largest_delta = [cell.x, cell.y, cell.z]
                        .into_iter()
                        .zip(state.origin)
                        .map(|(axis, origin)| axis.abs_diff(origin))
                        .max()
                        .unwrap();
                    (largest_delta as f64 * voxel_size() <= state.radius_m + voxel_size()).then(
                        || {
                            let [x, y, z] = relative(cell, state.origin);
                            [x, y, z, 0]
                        },
                    )
                })
                .collect();
            edited.sort_unstable();
            let bytes = bytemuck::cast_slice(&edited);
            if bytes.len() as u64 > u64::from(gpu.device.limits().max_storage_buffer_binding_size) {
                return Err("exact vegetation edits exceed the GPU storage binding limit".into());
            }
            state.edits = storage(gpu, "exact vegetation edit cells", bytes);
            state.edit_count = edited.len() as u32;
            state.bind = bind(
                gpu,
                &state.layout,
                &state.uniform,
                &state.buffers,
                &state.edits,
            );
            state.revision = world.revision();
            state.revision_valid = true;
        }

        Ok(state)
    }

    pub(super) fn prepare(
        &self,
        gpu: &Gpu,
        camera: Camera,
        width: u32,
        height: u32,
    ) -> Option<wgpu::BindGroup> {
        let settings = gpu.visual_settings.as_ref().unwrap();
        let depth = self
            .occlusion
            .then(|| gpu.rolling.as_ref()?.center_hits(width, height))
            .flatten();
        let forward = camera.direction();
        let right = forward.cross(camera.view_up()).normalize();
        let up = right.cross(forward).normalize();
        let scale = (camera.fov_y.to_radians() * 0.5).tan();
        let sun = settings.terrain_light_direction();
        let near = 0.025;
        let far = camera.far;
        let a = far / (far - near);
        let voxel = voxel_size() as f32;
        let view = View {
            eye: DVec3::from_array(self.eye(camera))
                .as_vec3()
                .extend(f32::from(depth.is_some()))
                .to_array(),
            forward: forward
                .extend(f32::from(settings.atmosphere && settings.fog_density > 0.))
                .to_array(),
            right: (right * scale * width as f32 / height as f32)
                .extend(f32::from(gpu.sample_count > 1))
                .to_array(),
            up: (up * scale).extend(voxel.recip()).to_array(),
            sun: sun.extend(0.).to_array(),
            clip: [near / voxel, far / voxel, a, near * a / voxel],
            counts: [self.node_count, self.edit_count, width, height],
        };
        gpu.queue
            .write_buffer(&self.uniform, 0, bytemuck::bytes_of(&view));
        self.used_terrain_depth.set(depth.is_some());
        depth.map(|hits| terrain_bind(&gpu.device, &self.depth_layout, hits))
    }

    fn eye(&self, camera: Camera) -> [f64; 3] {
        (camera.position / voxel_size() - DVec3::from_array(self.origin.map(|v| v as f64)))
            .to_array()
    }

    fn install(&mut self, scene: &Scene) {
        self.origin = scene.origin;
        self.radius_m = scene.radius_m;
        self.node_count = scene.nodes.len() as u32;
        self.brick_count = scene.bricks.len();
        self.occupied = scene.occupied;
        self.uniform_bricks = scene.uniform_bricks;
        self.palette_entries = scene.palette_entries;
        self.plants = scene.plants;
        self.build_ms = scene.build_ms;
    }

    fn new(gpu: &Gpu, scene: &Scene, budget: usize) -> Result<Self> {
        let device = &gpu.device;
        let buffers = Buffers::upload(gpu, scene, budget)?;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("exact vegetation camera"),
            size: std::mem::size_of::<View>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let threaded =
            std::env::var("FRONTIER_EXACT_VEGETATION_TRAVERSAL").as_deref() == Ok("threaded");
        let constants = [("THREADED".to_owned(), f64::from(threaded))]
            .into_iter()
            .collect();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("exact vegetation rays"),
            source: wgpu::ShaderSource::Wgsl(include_str!("exact_vegetation.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("exact vegetation visibility"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.scene_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: gpu.sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        let edits = storage(gpu, "exact vegetation empty edits", &[]);
        let depth_layout = pipeline.get_bind_group_layout(1);
        let empty_depth = terrain_bind(device, &depth_layout, &edits);
        let occlusion =
            std::env::var_os("FRONTIER_EXACT_VEGETATION_DEPTH").is_none_or(|v| v != "0");
        let bind = bind(gpu, &layout, &uniform, &buffers, &edits);
        let mut state = Self {
            uniform,
            layout,
            pipeline,
            bind,
            depth_layout,
            empty_depth,
            occlusion,
            used_terrain_depth: std::cell::Cell::new(false),
            buffers,
            origin: scene.origin,
            radius_m: scene.radius_m,
            node_count: 0,
            brick_count: 0,
            occupied: 0,
            uniform_bricks: 0,
            palette_entries: 0,
            plants: scene.plants,
            build_ms: 0.,
            revision: 0,
            revision_valid: false,
            edit_count: 0,
            edits,
            threaded,
        };
        state.install(scene);
        Ok(state)
    }

    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, terrain: Option<&wgpu::BindGroup>) {
        if self.node_count != 0 {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_bind_group(1, terrain.unwrap_or(&self.empty_depth), &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub(super) fn json(&self) -> String {
        format!("\"vegetation\":{{\"enabled\":true,\"mode\":\"exact_native_brick_bvh\",\"source_version\":{},\"canonical_voxel_m\":{},\"traversal\":{:?},\"terrain_depth_clip\":{},\"records_tree_grass_flower\":{:?},\"occupied_cells\":{},\"bricks\":{},\"uniform_pigment_bricks\":{},\"palette_entries\":{},\"nodes\":{},\"edit_count\":{},\"gpu_bytes\":{},\"latest_build_ms\":{:.3}}}",
            frontier_world::vegetation::source_version(), voxel_size(),
            if self.threaded { "threaded" } else { "near_ordered" }, self.used_terrain_depth.get(), self.plants, self.occupied,
            self.brick_count, self.uniform_bricks, self.palette_entries, self.node_count, self.edit_count, self.buffers.bytes + self.edits.size() as usize, self.build_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::Material;

    #[test]
    fn negative_cells_address_the_correct_occupancy_word() {
        assert_eq!(address([-1, -4, -5]), ([-4, -4, -8], 51));
        assert_eq!(address([4, 7, 12]), ([4, 4, 12], 12));
    }

    // Test the actual visibility shader, current edit upload, rolled projection
    // and depth writes against the engine's independent f64 native-cell picker.
    #[test]
    #[ignore = "needs a wgpu adapter"]
    fn gpu_first_hit_follows_canonical_voxels_and_live_edits() {
        let instance = wgpu::Instance::default();
        let origin = [0, 1024, 0];
        let mesh = super::super::mesh::Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            exposed_native_faces: 0,
            origin: Cell { x: 0, y: 0, z: 0 },
        };
        let mut gpu = pollster::block_on(Gpu::new(
            &instance,
            None,
            Some(wgpu::TextureFormat::Rgba8Unorm),
            &mesh,
            1,
            false,
            false,
        ))
        .unwrap();
        gpu.visual_settings = Some(super::super::settings::VisualSettings::default());

        let camera = Camera {
            position: DVec3::new(0., 64., 0.),
            yaw: 0.,
            pitch: 0.,
            roll: 0.19,
            far: 4.,
            fov_y: 60.,
        };
        let mut cells = Vec::new();
        let mut slots: HashMap<[i32; 3], [u32; 2]> = HashMap::new();
        for z in [9, 34] {
            for y in -4..4 {
                for x in -5..5 {
                    if (x + y) % 3 == 0 || z == 34 {
                        let (key, bit) = address([x, y, z]);
                        slots.entry(key).or_default()[bit / 32] |= 1u32 << (bit % 32);
                        cells.push(Cell {
                            x: i64::from(x),
                            y: origin[1] + i64::from(y),
                            z: i64::from(z),
                        });
                    }
                }
            }
        }
        let mut pigments = Vec::new();
        let mut bricks: Vec<_> = slots
            .into_iter()
            .map(|(min, mask)| {
                let pigment = pigments.len() as u32;
                pigments.extend(std::iter::repeat_n(
                    pack([0.15, 0.38, 0.07]),
                    (mask[0].count_ones() + mask[1].count_ones()) as usize,
                ));
                Brick {
                    min,
                    mask,
                    pigment,
                    bounds: brick_bounds(u64::from(mask[0]) | (u64::from(mask[1]) << 32)),
                    padding: 0,
                }
            })
            .collect();
        let mut nodes = Vec::new();
        build_bvh(&mut bricks, 0, &mut nodes);
        let (pigments, palette_entries) = pigment_stream(&pigments, true);
        assert_eq!(palette_entries, 1);
        let fixture = Scene {
            origin,
            radius_m: 8.,
            nodes,
            bricks,
            pigments,
            occupied: cells.len(),
            uniform_bricks: 0,
            palette_entries,
            plants: [0; 3],
            build_ms: 0.,
        };
        let mut renderer = ExactVegetation::new(&gpu, &fixture, 1 << 20).unwrap();
        let mut authoritative = World::new(7);
        authoritative.set_batch(cells.iter().copied().map(|cell| (cell, Material::Leaf)));
        let mut edits = World::new(7);

        const WIDTH: u32 = 31;
        const HEIGHT: u32 = 23;
        let mut first_near_hits = 0;
        let mut first_far_hits = 0;
        for remove in [false, true] {
            if remove {
                // World::set sees a real leaf before generating the Air tombstone.
                let cut = cells.iter().copied().filter(|cell| cell.z == 9);
                edits.set_batch(cut.clone().map(|cell| (cell, Material::Leaf)));
                edits.set_batch(cut.clone().map(|cell| (cell, Material::Air)));
                authoritative.set_batch(cut.map(|cell| (cell, Material::Air)));
            }
            renderer = ExactVegetation::update(Some(renderer), &gpu, &edits, camera).unwrap();
            assert_eq!(renderer.build_ms, 0.);
            let terrain = renderer.prepare(&gpu, camera, WIDTH, HEIGHT);
            assert!(terrain.is_none());
            let data = render_depth(&gpu, &renderer, WIDTH, HEIGHT);
            let forward = camera.direction();
            let up = camera.view_up();
            let scale = (camera.fov_y.to_radians() * 0.5).tan();
            let right = forward.cross(up) * scale * (WIDTH as f32 / HEIGHT as f32);
            let up = up * scale;
            let mut near_hits = 0;
            let mut far_hits = 0;
            for row in 0..HEIGHT {
                for col in 0..WIDTH {
                    let x = (col as f32 + 0.5) * 2. / WIDTH as f32 - 1.;
                    let y = 1. - (row as f32 + 0.5) * 2. / HEIGHT as f32;
                    let direction = (forward + right * x + up * y).as_dvec3().normalize();
                    let hit = frontier_world::raycast(
                        &authoritative,
                        camera.position.to_array(),
                        direction.to_array(),
                        f64::from(camera.far),
                    );
                    let expected = hit.map_or(1., |hit| {
                        if hit.cell.z == 9 {
                            near_hits += 1;
                        } else {
                            assert_eq!(hit.cell.z, 34);
                            far_hits += 1;
                        }
                        let z = hit.distance * direction.dot(forward.as_dvec3());
                        let a = f64::from(camera.far) / (f64::from(camera.far) - 0.025);
                        (a - 0.025 * a / z) as f32
                    });
                    let index = (row * 256 + col * 4) as usize;
                    let depth = f32::from_le_bytes(data[index..index + 4].try_into().unwrap());
                    assert!(
                        (depth - expected).abs() < 3e-6,
                        "pixel ({col},{row}): GPU {depth}, CPU {expected}"
                    );
                }
            }
            if remove {
                assert_eq!(near_hits, 0);
                assert!(far_hits > first_far_hits);
            } else {
                first_near_hits = near_hits;
                first_far_hits = far_hits;
                assert!(first_near_hits > 20);
            }
        }
        assert!(first_near_hits < WIDTH * HEIGHT);
    }

    fn render_depth(gpu: &Gpu, scene: &ExactVegetation, width: u32, height: u32) -> Vec<u8> {
        let color = gpu.texture(width, height, false);
        let depth = gpu.texture(width, height, true);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native plant depth fixture"),
            size: u64::from(height) * 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color.create_view(&Default::default()),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth.create_view(&Default::default()),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            scene.draw(&mut pass, None);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &depth,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
        gpu.device.poll(wgpu::Maintain::Wait);
        receiver.recv().unwrap().unwrap();
        let data = slice.get_mapped_range().to_vec();
        data
    }
}
