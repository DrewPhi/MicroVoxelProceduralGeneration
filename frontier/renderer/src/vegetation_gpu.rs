//! Reusable opaque voxel template geometry, one instanced draw per model/LOD.
//! Canonical roots remain integers until after origin subtraction.
use crate::vegetation::{MeshKey, VegetationFrame, VegetationInstance, VegetationVertex};
use bytemuck::{Pod, Zeroable};
use std::collections::{HashMap, HashSet};
use wgpu::util::DeviceExt;

const CACHE_BYTES: u64 = 256 * 1024 * 1024;
const SHADOW_SHADER: &str = include_str!("vegetation_shadow.wgsl");
const SHADER: &str = include_str!("vegetation.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Instance {
    root: [f32; 3],
    turn: u32,
    fade: f32,
    seed: u32,
    padding: [u32; 2],
}
fn pack(instance: &VegetationInstance, origin: [i64; 3]) -> Instance {
    let root = std::array::from_fn(|i| {
        ((instance.root_cell[i] as i128 - origin[i] as i128) as f64 * 0.0625) as f32
    });
    let mut seed = 0x9e3779b9u32;
    for coordinate in instance.root_cell {
        seed = seed.wrapping_mul(1597334677) ^ coordinate as u32 ^ (coordinate >> 32) as u32;
    }
    Instance {
        root,
        turn: instance.quarter_turn & 3,
        fade: instance.fade.clamp(0., 1.),
        seed,
        padding: [0; 2],
    }
}

// Bit two identifies ground tiles only in the shadow shader. Both vertex
// rotations mask the low two bits, so the scene transform remains identical.
fn pack_for_species(instance: &VegetationInstance, origin: [i64; 3], species: u8) -> Instance {
    let mut packed = pack(instance, origin);
    if species != 2 {
        packed.turn |= 4;
    }
    packed
}

/// Exact packed contents include order and length: a changed shadow prefix,
/// floating origin, canonical root, rotation or fade must upload again.
fn cache_instance_upload(cache: &mut Vec<u8>, instances: &[Instance]) -> bool {
    let bytes = bytemuck::cast_slice(instances);
    if cache.as_slice() == bytes {
        return false;
    }
    cache.clear();
    cache.extend_from_slice(bytes);
    true
}

/// Scalar-only layout matches WGSL's 44-byte storage-array stride. Keep the
/// material floats unchanged; quantizing RGB changes canonical voxel pigments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Quad {
    origin: [i32; 3],
    extent: [u32; 2],
    face: u32,
    color: [f32; 3],
    ao: f32,
    transmission: f32,
}
fn pack_quads(mesh: &crate::vegetation::VegetationMesh) -> Result<Vec<Quad>, String> {
    if mesh.vertices.len() % 4 != 0 || mesh.indices.len() != mesh.vertices.len() / 4 * 6 {
        return Err("vegetation mesh is not a complete quad stream".into());
    }
    let mut packed = Vec::with_capacity(mesh.vertices.len() / 4);
    for (index, vertices) in mesh.vertices.chunks_exact(4).enumerate() {
        let first = &vertices[0];
        let axis = (0..3)
            .find(|&d| first.normal[d].abs() == 1.)
            .ok_or("vegetation quad has no axis normal")?;
        let positive = first.normal[axis] > 0.;
        let mut normal = [0.; 3];
        normal[axis] = if positive { 1. } else { -1. };
        let mut cells = [[0i32; 3]; 4];
        for (corner, vertex) in vertices.iter().enumerate() {
            if vertex.normal != normal
                || vertex.color.map(f32::to_bits) != first.color.map(f32::to_bits)
                || vertex.ao.to_bits() != first.ao.to_bits()
                || vertex.transmission.to_bits() != first.transmission.to_bits()
            {
                return Err("vegetation quad has varying face material or normal".into());
            }
            for d in 0..3 {
                let cell = vertex.position[d] as f64 * 16.;
                if !cell.is_finite()
                    || cell.fract() != 0.
                    || cell < i32::MIN as f64
                    || cell > i32::MAX as f64
                {
                    return Err("vegetation quad is outside the native i32 lattice".into());
                }
                cells[corner][d] = cell as i32;
            }
        }
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let width = cells[1][u] as i64 - cells[0][u] as i64;
        let height = cells[3][v] as i64 - cells[0][v] as i64;
        if width <= 0 || height <= 0 || width > u32::MAX as i64 || height > u32::MAX as i64 {
            return Err("vegetation quad has invalid extents".into());
        }
        let quad = Quad {
            origin: cells[0],
            extent: [width as u32, height as u32],
            face: (axis as u32) * 2 + u32::from(positive),
            color: first.color,
            ao: first.ao,
            transmission: first.transmission,
        };
        let corners = if positive {
            [0, 1, 2, 0, 2, 3]
        } else {
            [0, 2, 1, 0, 3, 2]
        };
        for (offset, corner) in corners.into_iter().enumerate() {
            if mesh.indices[index * 6 + offset] as usize != index * 4 + corner {
                return Err("vegetation quad has noncanonical winding".into());
            }
        }
        for corner in 0..4 {
            let mut expected = cells[0];
            if corner == 1 || corner == 2 {
                expected[u] = cells[1][u];
            }
            if corner == 2 || corner == 3 {
                expected[v] = cells[3][v];
            }
            if cells[corner] != expected {
                return Err("vegetation quad is not an axis-aligned rectangle".into());
            }
        }
        packed.push(quad);
    }
    Ok(packed)
}
fn quad_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("packed native vegetation quads"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

enum Geometry {
    Compact {
        _quads: wgpu::Buffer,
        bind: wgpu::BindGroup,
        indices: Option<wgpu::Buffer>,
    },
    Expanded {
        vertices: wgpu::Buffer,
        indices: wgpu::Buffer,
    },
}
struct Template {
    geometry: Geometry,
    vertex_count: u32,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    last_instance_bytes: Vec<u8>,
    instance_count: u32,
    shadow_count: u32,
    geometry_bytes: u64,
}
impl Template {
    fn bytes(&self) -> u64 {
        self.geometry_bytes + self.instance_capacity
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VegetationGpuStats {
    pub cached_templates: usize,
    pub draw_calls: usize,
    pub instances: usize,
    pub shadow_instances: usize,
    pub shadow_triangles: usize,
    pub shadow_admitted: usize,
    pub shadow_removed: usize,
    pub shadow_budget_rejected: usize,
    pub resident_bytes: u64,
    pub uploaded_bytes: u64,
    pub cache_budget_bytes: u64,
}

pub struct VegetationGpu {
    pipeline: wgpu::RenderPipeline,
    expanded: bool,
    compact_indexed: bool,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_enabled: bool,
    lighting_uniform: wgpu::Buffer,
    lighting_bind: wgpu::BindGroup,
    quad_layout: wgpu::BindGroupLayout,
    templates: HashMap<MeshKey, Template>,
    active: Vec<MeshKey>,
    shadow_active: Vec<MeshKey>,
    previous_casters: HashSet<CasterKey>,
    scratch: Vec<Instance>,
    stats: VegetationGpuStats,
}
impl VegetationGpu {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sample_count: u32,
        camera_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let mode = std::env::var("FRONTIER_VEGETATION_GPU").unwrap_or_default();
        Self::new_with_mode(device, format, sample_count, camera_layout, &mode)
    }
    fn new_with_mode(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sample_count: u32,
        camera_layout: &wgpu::BindGroupLayout,
        mode: &str,
    ) -> Self {
        let lighting_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vegetation lighting"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let lighting_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vegetation lighting"),
            contents: bytemuck::cast_slice(&[0f32; 8]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let lighting_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vegetation lighting"),
            layout: &lighting_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: lighting_uniform.as_entire_binding(),
            }],
        });
        // A/B switch retains the identical expanded content and shading path.
        let expanded = mode == "expanded";
        let compact_indexed = mode == "compact-indexed";
        let quad_layout = quad_layout(device);
        let sampling_layout = crate::adaptive_gpu::adaptive_shadow::near_sampling_layout(device);
        let mut color_layouts = vec![camera_layout, &lighting_layout, &sampling_layout];
        if !expanded {
            color_layouts.push(&quad_layout);
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("instanced voxel vegetation"),
            bind_group_layouts: &color_layouts,
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced voxel vegetation"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let instance_attributes =
            wgpu::vertex_attr_array![3=>Float32x3,4=>Uint32,5=>Float32,6=>Uint32];
        let vertex_attributes =
            wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3,7=>Float32,8=>Float32];
        let mut buffers = Vec::new();
        if expanded {
            buffers.push(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<VegetationVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vertex_attributes,
            });
        }
        buffers.push(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &instance_attributes,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("instanced voxel vegetation"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(if expanded {
                    "vs_expanded"
                } else if compact_indexed {
                    "vs_compact_indexed"
                } else {
                    "vs_main"
                }),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced plant shadow depth"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let depth_layout = crate::adaptive_gpu::adaptive_shadow::near_depth_layout(device);
        let mut shadow_layouts = vec![camera_layout, &depth_layout];
        if !expanded {
            shadow_layouts.push(&quad_layout);
        }
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("instanced plant shadow depth"),
            bind_group_layouts: &shadow_layouts,
            push_constant_ranges: &[],
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("instanced plant shadow depth"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some(if expanded {
                    "vs_shadow_expanded"
                } else if compact_indexed {
                    "vs_shadow_compact_indexed"
                } else {
                    "vs_shadow"
                }),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shadow_shader,
                entry_point: Some("fs_shadow"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.,
                    clamp: 0.,
                },
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            shadow_pipeline,
            expanded,
            compact_indexed,
            shadow_enabled: false,
            pipeline,
            lighting_uniform,
            lighting_bind,
            quad_layout,
            templates: HashMap::new(),
            active: vec![],
            shadow_active: vec![],
            previous_casters: HashSet::new(),
            scratch: vec![],
            stats: Default::default(),
        }
    }

    pub fn set_visual_lighting(
        &mut self,
        queue: &wgpu::Queue,
        settings: &crate::settings::VisualSettings,
        regional: bool,
    ) {
        self.shadow_enabled = settings.beauty && settings.terrain_shadows;
        let sun = crate::atmosphere::sun_direction(settings.sun_elevation, settings.sun_azimuth);
        let lighting = [
            sun.x,
            sun.y,
            sun.z,
            if regional { 1. } else { 0. },
            settings.overcast,
            if settings.beauty { 1. } else { 0. },
            0.,
            0.,
        ];
        queue.write_buffer(&self.lighting_uniform, 0, bytemuck::cast_slice(&lighting));
    }

    /// Geometry stays resident across frames; instance storage grows geometrically.
    /// Inactive templates are evicted only when needed to satisfy the fixed budget.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &VegetationFrame,
        origin: [i64; 3],
        camera_local: [f32; 3],
    ) -> Result<(), String> {
        // A refused new frame must never draw old instance positions with the
        // new camera origin. Immutable templates remain available for retry.
        self.active.clear();
        self.shadow_active.clear();
        self.stats.draw_calls = 0;
        self.stats.instances = 0;
        self.stats.shadow_instances = 0;
        self.stats.shadow_triangles = 0;
        self.stats.shadow_admitted = 0;
        self.stats.shadow_removed = 0;
        self.stats.shadow_budget_rejected = 0;
        self.stats.uploaded_bytes = 0;
        let cache_budget = if frame.stats.reference {
            1024 * 1024 * 1024
        } else {
            CACHE_BYTES
        };
        self.stats.cache_budget_bytes = cache_budget;
        let batches = frame_batches(frame);
        let wanted: HashSet<_> = batches
            .iter()
            .filter(|entry| {
                (entry.color || self.shadow_enabled)
                    && !entry.batch.instances.is_empty()
                    && !entry.batch.mesh.indices.is_empty()
            })
            .map(|entry| entry.batch.key)
            .collect();
        let mut new_quads = HashMap::new();
        let mut needed = 0u64;
        for entry in &batches {
            let batch = entry.batch;
            if !wanted.contains(&batch.key) {
                continue;
            }
            if batch.mesh.indices.len() > u32::MAX as usize
                || batch.instances.len() > u32::MAX as usize
            {
                return Err("vegetation draw exceeds u32 counts".into());
            }
            let geometry = if self.expanded {
                (batch.mesh.vertices.len() * std::mem::size_of::<VegetationVertex>()
                    + batch.mesh.indices.len() * 4) as u64
            } else {
                (batch.mesh.vertices.len() / 4 * std::mem::size_of::<Quad>()
                    + if self.compact_indexed {
                        batch.mesh.indices.len() * 4
                    } else {
                        0
                    }) as u64
            };
            if !self.expanded && !self.templates.contains_key(&batch.key) {
                new_quads.insert(batch.key, pack_quads(&batch.mesh)?);
            }
            let instances = ((batch.instances.len() * std::mem::size_of::<Instance>()) as u64)
                .next_power_of_two()
                .max(32);
            let bytes = self
                .templates
                .get(&batch.key)
                .map_or(geometry + instances, |t| {
                    t.geometry_bytes + t.instance_capacity.max(instances)
                });
            if (!self.expanded
                && (batch.mesh.vertices.len() / 4 * std::mem::size_of::<Quad>()) as u64
                    > device.limits().max_storage_buffer_binding_size as u64)
                || geometry > device.limits().max_buffer_size
                || instances > device.limits().max_buffer_size
            {
                return Err("vegetation exceeds device buffer limit".into());
            }
            needed = needed
                .checked_add(bytes)
                .ok_or("vegetation memory overflow")?;
        }
        if needed > cache_budget {
            return Err(format!(
                "vegetation active templates exceed {} MiB budget",
                cache_budget / (1024 * 1024)
            ));
        }
        let mut resident: u64 = self.templates.values().map(Template::bytes).sum();
        let added = needed.saturating_sub(
            self.templates
                .iter()
                .filter(|(k, _)| wanted.contains(*k))
                .map(|(_, t)| t.bytes())
                .sum(),
        );
        if resident + added > cache_budget {
            let mut unused: Vec<_> = self
                .templates
                .keys()
                .filter(|k| !wanted.contains(*k))
                .copied()
                .collect();
            unused.sort();
            for key in unused {
                resident -= self.templates.remove(&key).unwrap().bytes();
                if resident + added <= cache_budget {
                    break;
                }
            }
        }
        self.active.clear();
        self.stats.uploaded_bytes = 0;
        self.stats.instances = 0;
        self.stats.shadow_instances = 0;
        self.stats.shadow_triangles = 0;
        // Allocate globally by root distance, before grouping by template. A
        // high-resolution variant must not consume the budget simply because
        // its mesh key sorts before the tree beside the camera.
        let (shadow_selection, shadow_triangles, rejected) = if self.shadow_enabled {
            select_shadow_casters_stable(
                frame,
                origin,
                camera_local,
                200_000,
                &self.previous_casters,
            )
        } else {
            (HashSet::new(), 0, 0)
        };
        self.stats.shadow_budget_rejected = rejected;
        let casters: HashSet<_> = shadow_selection
            .iter()
            .map(|&(batch_index, instance_index)| {
                let batch = frame_batch(frame, batch_index);
                caster_key(batch, &batch.instances[instance_index])
            })
            .collect();
        self.stats.shadow_admitted = casters.difference(&self.previous_casters).count();
        self.stats.shadow_removed = self.previous_casters.difference(&casters).count();
        self.previous_casters = casters;
        for entry in &batches {
            let batch = entry.batch;
            let batch_index = entry.index;
            if !wanted.contains(&batch.key) {
                continue;
            }
            self.scratch.clear();
            // Reuse one instance allocation: selected roots form the depth-draw
            // prefix; every instance still appears exactly once in scene draws.
            for (index, instance) in batch.instances.iter().enumerate() {
                if shadow_selection.contains(&(batch_index, index)) {
                    self.scratch
                        .push(pack_for_species(instance, origin, batch.key.species));
                }
            }
            let shadow_count = self.scratch.len() as u32;
            for (index, instance) in batch.instances.iter().enumerate() {
                if !shadow_selection.contains(&(batch_index, index)) {
                    self.scratch
                        .push(pack_for_species(instance, origin, batch.key.species));
                }
            }
            let bytes = bytemuck::cast_slice(&self.scratch);
            let capacity = (bytes.len() as u64).next_power_of_two().max(32);
            let template = self.templates.entry(batch.key).or_insert_with(|| {
                let (geometry, geometry_bytes) = if self.expanded {
                    let vertices = bytemuck::cast_slice(&batch.mesh.vertices);
                    let indices = bytemuck::cast_slice(&batch.mesh.indices);
                    (
                        Geometry::Expanded {
                            vertices: device.create_buffer_init(
                                &wgpu::util::BufferInitDescriptor {
                                    label: Some("A/B expanded vegetation vertices"),
                                    contents: vertices,
                                    usage: wgpu::BufferUsages::VERTEX,
                                },
                            ),
                            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("A/B expanded vegetation indices"),
                                contents: indices,
                                usage: wgpu::BufferUsages::INDEX,
                            }),
                        },
                        (vertices.len() + indices.len()) as u64,
                    )
                } else {
                    let quads = new_quads
                        .remove(&batch.key)
                        .expect("new template was packed before allocation");
                    let bytes = bytemuck::cast_slice(&quads);
                    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("cached vegetation packed quads"),
                        contents: bytes,
                        usage: wgpu::BufferUsages::STORAGE,
                    });
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("cached vegetation packed quads"),
                        layout: &self.quad_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        }],
                    });
                    let indices = self.compact_indexed.then(|| {
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("indexed compact vegetation topology"),
                            contents: bytemuck::cast_slice(&batch.mesh.indices),
                            usage: wgpu::BufferUsages::INDEX,
                        })
                    });
                    (
                        Geometry::Compact {
                            _quads: buffer,
                            bind,
                            indices,
                        },
                        bytes.len() as u64
                            + if self.compact_indexed {
                                batch.mesh.indices.len() as u64 * 4
                            } else {
                                0
                            },
                    )
                };
                self.stats.uploaded_bytes += geometry_bytes;
                Template {
                    geometry,
                    vertex_count: batch.mesh.indices.len() as u32,
                    instances: instance_buffer(device, capacity),
                    instance_capacity: capacity,
                    last_instance_bytes: Vec::new(),
                    instance_count: 0,
                    shadow_count: 0,
                    geometry_bytes,
                }
            });
            if capacity > template.instance_capacity {
                template.instances = instance_buffer(device, capacity);
                template.instance_capacity = capacity;
                // A replacement GPU buffer has no contents even if the next
                // packed sequence happens to equal a formerly uploaded frame.
                template.last_instance_bytes.clear();
            }
            if cache_instance_upload(&mut template.last_instance_bytes, &self.scratch) {
                queue.write_buffer(&template.instances, 0, bytes);
                self.stats.uploaded_bytes += bytes.len() as u64;
            }
            template.instance_count = self.scratch.len() as u32;
            template.shadow_count = shadow_count;
            self.stats.shadow_instances += shadow_count as usize;
            self.stats.shadow_triangles = shadow_triangles;
            if entry.color {
                self.stats.instances += self.scratch.len();
                self.active.push(batch.key);
            }
            if shadow_count > 0 {
                self.shadow_active.push(batch.key);
            }
        }
        self.stats.cached_templates = self.templates.len();
        self.stats.draw_calls = self.active.len();
        self.stats.resident_bytes = self.templates.values().map(Template::bytes).sum();
        Ok(())
    }
    pub fn stats(&self) -> VegetationGpuStats {
        self.stats
    }
    pub fn representation(&self) -> &'static str {
        if self.expanded {
            "expanded"
        } else if self.compact_indexed {
            "compact-indexed"
        } else {
            "compact"
        }
    }
    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
        near_shadow: &'a wgpu::BindGroup,
    ) {
        if self.active.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, &self.lighting_bind, &[]);
        pass.set_bind_group(2, near_shadow, &[]);
        for key in &self.active {
            let template = &self.templates[key];
            match &template.geometry {
                Geometry::Compact { bind, indices, .. } => {
                    pass.set_bind_group(3, bind, &[]);
                    pass.set_vertex_buffer(0, template.instances.slice(..));
                    if let Some(indices) = indices {
                        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..template.vertex_count, 0, 0..template.instance_count);
                    } else {
                        pass.draw(0..template.vertex_count, 0..template.instance_count);
                    }
                }
                Geometry::Expanded { vertices, indices } => {
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_vertex_buffer(1, template.instances.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..template.vertex_count, 0, 0..template.instance_count);
                }
            }
        }
    }
    pub(crate) fn draw_shadows(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        camera: &wgpu::BindGroup,
        view: &wgpu::TextureView,
        depth_bind: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bounded close plant shadow depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, depth_bind, &[]);
        for key in &self.shadow_active {
            let t = &self.templates[key];
            if t.shadow_count == 0 {
                continue;
            }
            match &t.geometry {
                Geometry::Compact { bind, indices, .. } => {
                    pass.set_bind_group(2, bind, &[]);
                    pass.set_vertex_buffer(0, t.instances.slice(..));
                    if let Some(indices) = indices {
                        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..t.vertex_count, 0, 0..t.shadow_count);
                    } else {
                        pass.draw(0..t.vertex_count, 0..t.shadow_count);
                    }
                }
                Geometry::Expanded { vertices, indices } => {
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_vertex_buffer(1, t.instances.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..t.vertex_count, 0, 0..t.shadow_count);
                }
            }
        }
    }
}
struct FrameBatch<'a> {
    batch: &'a crate::vegetation::VegetationBatch,
    color: bool,
    index: usize,
}
/// Color membership wins if a canonical tree key occurs in both sets. Keys
/// identify complete seeded geometry, so upload/storage is shared once.
fn frame_batches(frame: &VegetationFrame) -> Vec<FrameBatch<'_>> {
    let mut seen = HashSet::new();
    frame
        .batches
        .iter()
        .chain(&frame.shadow_batches)
        .enumerate()
        .filter_map(|(index, batch)| {
            seen.insert(batch.key).then_some(FrameBatch {
                batch,
                color: index < frame.batches.len(),
                index,
            })
        })
        .collect()
}
fn frame_batch(frame: &VegetationFrame, index: usize) -> &crate::vegetation::VegetationBatch {
    if index < frame.batches.len() {
        &frame.batches[index]
    } else {
        &frame.shadow_batches[index - frame.batches.len()]
    }
}
/// Geometry LOD and color/frustum membership are deliberately absent: the same
/// canonical plant keeps its caster priority while either display choice changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct CasterKey {
    species: u8,
    seed: u64,
    tile: [i64; 2],
    root: [i64; 3],
}
fn caster_key(
    batch: &crate::vegetation::VegetationBatch,
    instance: &VegetationInstance,
) -> CasterKey {
    CasterKey {
        species: batch.key.species,
        seed: batch.key.seed,
        tile: batch.key.tile,
        root: instance.root_cell,
    }
}

/// Stable distance priority within tree and close ground-cover allocations.
/// Color plants and bounded near offscreen trees share this allocation.
/// Unstreamed trees and partly fading plants remain outside its coverage.
#[cfg(test)]
fn select_shadow_casters(
    frame: &VegetationFrame,
    origin: [i64; 3],
    eye: [f32; 3],
    triangle_budget: usize,
) -> (HashSet<(usize, usize)>, usize) {
    let (selected, triangles, _) =
        select_shadow_casters_stable(frame, origin, eye, triangle_budget, &HashSet::new());
    (selected, triangles)
}
fn select_shadow_casters_stable(
    frame: &VegetationFrame,
    origin: [i64; 3],
    eye: [f32; 3],
    triangle_budget: usize,
    previous: &HashSet<CasterKey>,
) -> (HashSet<(usize, usize)>, usize, usize) {
    // Streamed explicit caster geometry is independent of the color LOD. Use
    // color geometry only until that root's bounded shadow template is ready.
    let explicit: HashMap<_, _> = frame
        .shadow_batches
        .iter()
        .filter(|batch| !batch.mesh.indices.is_empty())
        .flat_map(|batch| {
            batch
                .instances
                .iter()
                .filter(|instance| instance.fade > 0.99)
                .map(move |instance| (caster_key(batch, instance), batch.key))
        })
        .collect();
    let mut candidates = Vec::new();
    for entry in frame_batches(frame) {
        let batch = entry.batch;
        let batch_index = entry.index;
        if batch.mesh.indices.is_empty() {
            continue;
        }
        for (index, instance) in batch.instances.iter().enumerate() {
            if instance.fade <= 0.99 {
                continue;
            }
            let identity = caster_key(batch, instance);
            if entry.color && explicit.get(&identity).is_some_and(|key| *key != batch.key) {
                continue;
            }
            let retained = previous.contains(&identity);
            let p = pack(instance, origin).root;
            let distance = if batch.key.species == 2 {
                (0..3)
                    .map(|axis| (p[axis] - eye[axis]).powi(2))
                    .sum::<f32>()
            } else {
                // Canonical ground-tile meshes carry surface height in their
                // vertices and deliberately use a y=0 root. A 3D root distance
                // would reject every meadow above sea level.
                // Grouped ground geometry occupies an 8m tile from its root.
                // Select by center and retain any tile intersecting the depth
                // fragment's 18m fade support (half-diagonal sqrt(32)).
                (p[0] + 4. - eye[0]).powi(2) + (p[2] + 4. - eye[2]).powi(2)
            };
            // The 96m map resolves close grass/flower contact shadows. Keep
            // their work local; reserve most depth triangles for tree canopies.
            let radius = if batch.key.species == 2 {
                if retained {
                    96.
                } else {
                    90.
                }
            } else {
                18. + 32f32.sqrt()
            };
            if distance < radius * radius {
                // A challenger must become at least 10% nearer to displace a
                // retained caster. This changes admission only, never geometry
                // or the bounded depth budget. Canonical ties survive turning.
                let priority = distance * if retained { 0.81 } else { 1. };
                candidates.push((
                    priority,
                    batch_index,
                    index,
                    batch.mesh.indices.len() / 3,
                    identity,
                ));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.4.cmp(&b.4)));
    let eligible = candidates.len();
    let mut selected = HashSet::new();
    let mut triangles = 0;
    // Give nearby ground cover a quarter of the existing cap before trees.
    // Unused space is borrowed, so sparse meadows do not waste tree coverage.
    // Do not add a second depth pass, map, or unbounded caster generation.
    let ground_reserve = triangle_budget / 4;
    for &(_, batch, index, cost, _) in &candidates {
        if frame_batch(frame, batch).key.species != 2 && cost <= ground_reserve - triangles {
            selected.insert((batch, index));
            triangles += cost;
        }
    }
    for &(_, batch, index, cost, _) in &candidates {
        if frame_batch(frame, batch).key.species == 2 && cost <= triangle_budget - triangles {
            selected.insert((batch, index));
            triangles += cost;
        }
    }
    for (_, batch, index, cost, _) in candidates {
        if !selected.contains(&(batch, index)) && cost <= triangle_budget - triangles {
            selected.insert((batch, index));
            triangles += cost;
        }
    }
    let rejected = eligible - selected.len();
    (selected, triangles, rejected)
}

fn instance_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("reused vegetation instances"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn batch(variant: u8, roots: Vec<i64>, triangles: usize) -> crate::vegetation::VegetationBatch {
        use crate::vegetation::{VegetationBatch, VegetationMesh};
        VegetationBatch {
            key: MeshKey {
                species: 2,
                variant,
                lod: 1,
                edit_id: 0,
                seed: 0,
                tile: [0; 2],
            },
            mesh: std::sync::Arc::new(VegetationMesh {
                vertices: vec![],
                indices: vec![0; triangles * 3],
            }),
            instances: roots
                .into_iter()
                .map(|x| VegetationInstance {
                    root_cell: [x * 16, 0, 0],
                    quarter_turn: 0,
                    fade: 1.,
                })
                .collect(),
        }
    }
    #[test]
    fn offscreen_tree_casts_without_color_membership_and_shares_budget() {
        let frame = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            batches: vec![batch(0, vec![10], 4)],
            shadow_batches: vec![batch(1, vec![2], 4)],
        };
        let entries = frame_batches(&frame);
        assert_eq!(entries.iter().filter(|e| e.color).count(), 1);
        assert_eq!(entries.iter().filter(|e| !e.color).count(), 1);
        let (selected, triangles) = select_shadow_casters(&frame, [0; 3], [0.; 3], 4);
        assert_eq!(triangles, 4);
        assert_eq!(selected, HashSet::from([(1, 0)]));
        assert!(!entries
            .iter()
            .any(|e| e.color && e.batch.key == frame.shadow_batches[0].key));
    }
    #[test]
    fn turning_changes_color_membership_without_changing_caster_identity() {
        let a = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            batches: vec![batch(0, vec![5], 4)],
            shadow_batches: vec![batch(1, vec![6], 4)],
        };
        let b = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            batches: vec![batch(1, vec![6], 4)],
            shadow_batches: vec![batch(0, vec![5], 4)],
        };
        let keys = |frame: &VegetationFrame| {
            let (selected, triangles) = select_shadow_casters(frame, [0; 3], [0.; 3], 8);
            assert_eq!(triangles, 8);
            selected
                .into_iter()
                .map(|(index, _)| frame_batch(frame, index).key)
                .collect::<HashSet<_>>()
        };
        assert_eq!(keys(&a), keys(&b));
        let duplicate = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            batches: vec![batch(0, vec![5], 4)],
            shadow_batches: vec![batch(0, vec![5], 4)],
        };
        assert_eq!(frame_batches(&duplicate).len(), 1);
        assert!(frame_batches(&duplicate)[0].color);
        assert_eq!(select_shadow_casters(&duplicate, [0; 3], [0.; 3], 8).1, 4);
    }
    #[test]
    fn shadow_budget_prioritizes_nearest_tree_across_templates() {
        let frame = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            shadow_batches: Vec::new(),
            batches: vec![
                batch(0, vec![60, 40], 4),
                batch(1, vec![2, 5], 4),
                batch(2, vec![3], 2),
            ],
        };
        let (selected, triangles) = select_shadow_casters(&frame, [0; 3], [0.; 3], 6);
        assert_eq!(triangles, 6);
        assert_eq!(selected, HashSet::from([(1, 0), (2, 0)]));
        let (selected, triangles) = select_shadow_casters(&frame, [0; 3], [0.; 3], 0);
        assert!(selected.is_empty());
        assert_eq!(triangles, 0);
    }
    #[test]
    fn ground_shadow_reserve_is_local_and_cannot_starve_trees() {
        let mut grass = batch(0, vec![1, 2, 30], 2);
        grass.key.species = 0;
        let mut faded = batch(1, vec![0], 1);
        faded.key.species = 1;
        faded.instances[0].fade = 0.5;
        // Tile roots at y=0 must still cast on an elevated meadow.
        for instance in &mut grass.instances {
            instance.root_cell[1] = -1600;
        }
        let frame = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            shadow_batches: Vec::new(),
            batches: vec![grass, batch(0, vec![3, 4, 5, 6], 2), faded],
        };
        let (selected, triangles) = select_shadow_casters(&frame, [0; 3], [0.; 3], 8);
        assert_eq!(triangles, 8);
        assert_eq!(selected, HashSet::from([(0, 0), (1, 0), (1, 1), (1, 2)]));
        assert!(
            !selected.contains(&(0, 2)),
            "distant ground cover must not cast"
        );
        assert!(
            !selected.contains(&(2, 0)),
            "partly faded plants cannot cast solid shadows"
        );
        let (selected, triangles) = select_shadow_casters(&frame, [0; 3], [0.; 3], 1);
        assert!(selected.is_empty());
        assert_eq!(triangles, 0);
    }
    #[test]
    fn ground_shadow_support_keeps_intersecting_tiles_and_flags_only_ground() {
        let mut grass = batch(0, vec![16, 26], 2);
        grass.key.species = 0;
        let frame = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            shadow_batches: Vec::new(),
            batches: vec![grass],
        };
        let (selected, triangles) = select_shadow_casters(&frame, [0; 3], [0.; 3], 8);
        assert_eq!(selected, HashSet::from([(0, 0)]));
        assert_eq!(triangles, 2);
        let instance = VegetationInstance {
            root_cell: [0; 3],
            quarter_turn: 3,
            fade: 1.,
        };
        let ground = pack_for_species(&instance, [0; 3], 0);
        let tree = pack_for_species(&instance, [0; 3], 2);
        assert_eq!(ground.turn, 7);
        assert_eq!(tree.turn, 3);
        assert_eq!(ground.turn & 3, tree.turn);
        assert_eq!(ground.root, tree.root);
    }
    #[test]
    fn instance_upload_cache_tracks_rebase_fade_rotation_membership_and_count() {
        let mut a = VegetationInstance {
            root_cell: [160, 32, -48],
            quarter_turn: 1,
            fade: 1.,
        };
        let b = VegetationInstance {
            root_cell: [320, 32, -48],
            quarter_turn: 2,
            fade: 1.,
        };
        let mut cache = Vec::new();
        let initial = [pack(&a, [0; 3]), pack(&b, [0; 3])];
        assert!(cache_instance_upload(&mut cache, &initial));
        assert!(!cache_instance_upload(&mut cache, &initial));
        let rebased = [pack(&a, [16, 0, 0]), pack(&b, [16, 0, 0])];
        assert!(cache_instance_upload(&mut cache, &rebased));
        assert!(!cache_instance_upload(&mut cache, &rebased));
        a.fade = 0.75;
        let faded = [pack(&a, [16, 0, 0]), rebased[1]];
        assert!(cache_instance_upload(&mut cache, &faded));
        a.quarter_turn = 3;
        let rotated = [pack(&a, [16, 0, 0]), rebased[1]];
        assert!(cache_instance_upload(&mut cache, &rotated));
        // Moving a selected caster to the front changes the depth-draw prefix.
        let reordered = [rotated[1], rotated[0]];
        assert!(cache_instance_upload(&mut cache, &reordered));
        assert!(!cache_instance_upload(&mut cache, &reordered));
        assert!(cache_instance_upload(&mut cache, &reordered[..1]));
        assert!(cache_instance_upload(&mut cache, &reordered));
        // A new buffer must be populated even with the same sequence.
        cache.clear();
        assert!(cache_instance_upload(&mut cache, &reordered));
        a.root_cell[2] += 1;
        assert!(cache_instance_upload(
            &mut cache,
            &[pack(&a, [16, 0, 0]), rebased[1]]
        ));
    }
    fn test_quad(
        axis: usize,
        positive: bool,
        origin: [i32; 3],
        extent: [u32; 2],
    ) -> crate::vegetation::VegetationMesh {
        let mut mesh = crate::vegetation::VegetationMesh::default();
        let mut normal = [0.; 3];
        normal[axis] = if positive { 1. } else { -1. };
        for [a, b] in [[0, 0], [1, 0], [1, 1], [0, 1]] {
            let mut p = origin;
            p[(axis + 1) % 3] += a * extent[0] as i32;
            p[(axis + 2) % 3] += b * extent[1] as i32;
            mesh.vertices.push(VegetationVertex {
                position: p.map(|v| v as f32 * 0.0625),
                normal,
                color: [f32::from_bits(0x3e912345), 0.124781, 0.943287],
                ao: 143. / 255.,
                transmission: 0.35,
            });
        }
        mesh.indices = if positive {
            vec![0, 1, 2, 0, 2, 3]
        } else {
            vec![0, 2, 1, 0, 3, 2]
        };
        mesh
    }
    #[test]
    fn packed_quad_preserves_every_face_corner_and_float_material() {
        assert_eq!(std::mem::size_of::<Quad>(), 44);
        for axis in 0..3 {
            for positive in [false, true] {
                for origin in [[-302, -139, -35], [0; 3], [300, 121, 55]] {
                    for extent in [[1, 1], [8, 24], [300, 129]] {
                        let mesh = test_quad(axis, positive, origin, extent);
                        let quads = pack_quads(&mesh).unwrap();
                        let q = quads[0];
                        assert_eq!(q.origin, origin);
                        assert_eq!(q.extent, extent);
                        assert_eq!(q.face, axis as u32 * 2 + u32::from(positive));
                        for (vertex, &index) in mesh.indices.iter().enumerate() {
                            let corner = if positive {
                                [0, 1, 2, 0, 2, 3][vertex]
                            } else {
                                [0, 2, 1, 0, 3, 2][vertex]
                            };
                            let mut cells = q.origin;
                            if corner == 1 || corner == 2 {
                                cells[(axis + 1) % 3] += q.extent[0] as i32;
                            }
                            if corner == 2 || corner == 3 {
                                cells[(axis + 2) % 3] += q.extent[1] as i32;
                            }
                            // Indexed pulling uses the original topology: the
                            // index names one of four shared corners per quad.
                            assert_eq!(index / 4, 0);
                            assert_eq!(index % 4, corner as u32);
                            let original = &mesh.vertices[index as usize];
                            assert_eq!(
                                cells.map(|v| (v as f32 * 0.0625).to_bits()),
                                original.position.map(f32::to_bits)
                            );
                            assert_eq!(q.color.map(f32::to_bits), original.color.map(f32::to_bits));
                            assert_eq!(q.ao.to_bits(), original.ao.to_bits());
                            assert_eq!(q.transmission.to_bits(), original.transmission.to_bits());
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn indexed_quad_stream_addresses_all_faces_without_crossing_material_records() {
        let mut mesh = crate::vegetation::VegetationMesh::default();
        for axis in 0..3 {
            for positive in [false, true] {
                let mut face = test_quad(axis, positive, [-11, 7, -43], [3, 8]);
                let base = mesh.vertices.len() as u32;
                for vertex in &mut face.vertices {
                    vertex.color[0] = base as f32 / 100.;
                }
                mesh.indices.extend(face.indices.iter().map(|i| base + i));
                mesh.vertices.extend(face.vertices);
            }
        }
        let quads = pack_quads(&mesh).unwrap();
        assert_eq!(quads.len(), 6);
        assert_eq!(quads.len() * 44 + mesh.indices.len() * 4, 6 * 68);
        for &index in &mesh.indices {
            let q = quads[(index / 4) as usize];
            let corner = index % 4;
            let axis = (q.face / 2) as usize;
            let mut cells = q.origin;
            if corner == 1 || corner == 2 {
                cells[(axis + 1) % 3] += q.extent[0] as i32;
            }
            if corner == 2 || corner == 3 {
                cells[(axis + 2) % 3] += q.extent[1] as i32;
            }
            assert_eq!(
                cells.map(|v| v as f32 * 0.0625),
                mesh.vertices[index as usize].position
            );
            assert_eq!(
                q.color.map(f32::to_bits),
                mesh.vertices[index as usize].color.map(f32::to_bits)
            );
        }
    }
    #[test]
    fn packed_quad_rejects_non_native_nonrectangular_or_varying_material_meshes() {
        let original = test_quad(1, true, [-4; 3], [2, 3]);
        for defect in 0..6 {
            let mut mesh = original.clone();
            match defect {
                0 => mesh.vertices[0].position[0] += 0.01,
                1 => mesh.vertices[2].position[1] += 0.0625,
                2 => mesh.vertices[3].color[0] += 0.01,
                3 => mesh.vertices[1].ao = 1.,
                4 => mesh.indices.swap(1, 2),
                _ => mesh.vertices[2].transmission = 0.,
            }
            assert!(pack_quads(&mesh).is_err(), "defect {defect}");
        }
    }
    #[test]
    fn color_and_shadow_share_identical_native_decoder() {
        let end = "    return out;\n}";
        assert_eq!(
            SHADER
                .split(end)
                .next()
                .unwrap()
                .replace("@group(3)", "@group(2)"),
            SHADOW_SHADER.split(end).next().unwrap()
        );
    }
    #[test]
    fn explicit_shadow_lod_wins_without_drawing_duplicate_canonical_roots() {
        let color = batch(0, vec![5], 40);
        let mut explicit = batch(0, vec![5], 4);
        explicit.key.lod = 2;
        let mut frame = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            batches: vec![color],
            shadow_batches: vec![explicit],
        };
        let (selected, cost) = select_shadow_casters(&frame, [0; 3], [0.; 3], 40);
        assert_eq!(selected, HashSet::from([(1, 0)]));
        assert_eq!(cost, 4);
        // Same-key explicit geometry shares the color template and draw prefix.
        frame.batches[0].key.lod = 2;
        let (selected, cost) = select_shadow_casters(&frame, [0; 3], [0.; 3], 40);
        assert_eq!(selected, HashSet::from([(0, 0)]));
        assert_eq!(cost, 40);
        frame.shadow_batches.clear();
        let (selected, cost) = select_shadow_casters(&frame, [0; 3], [0.; 3], 40);
        assert_eq!(selected, HashSet::from([(0, 0)]));
        assert_eq!(cost, 40);
    }
    #[test]
    fn caster_admission_hysteresis_survives_lod_membership_and_near_ties() {
        let mut frame = VegetationFrame {
            origin: [0; 3],
            stats: Default::default(),
            shadow_batches: vec![],
            batches: vec![batch(0, vec![10], 4), batch(1, vec![11], 4)],
        };
        let previous = HashSet::from([caster_key(
            &frame.batches[0],
            &frame.batches[0].instances[0],
        )]);
        // Challenger becomes slightly nearer, but cannot steal the incumbent's cap.
        frame.batches[1].instances[0].root_cell[0] = 152;
        frame.batches[0].key.lod = 4;
        let old = frame.batches.remove(0);
        frame.shadow_batches.push(old);
        let (selected, cost, rejected) =
            select_shadow_casters_stable(&frame, [0; 3], [0.; 3], 4, &previous);
        assert_eq!(cost, 4);
        assert_eq!(selected, HashSet::from([(1, 0)]));
        assert_eq!(rejected, 1);
        // A meaningfully nearer caster can take priority within the same cap.
        frame.batches[0].instances[0].root_cell[0] = 128;
        let (selected, cost, _) =
            select_shadow_casters_stable(&frame, [0; 3], [0.; 3], 4, &previous);
        assert_eq!(selected, HashSet::from([(0, 0)]));
        assert_eq!(cost, 4);
        frame.shadow_batches[0].instances[0].root_cell[0] = 95 * 16;
        let retained = HashSet::from([caster_key(
            &frame.shadow_batches[0],
            &frame.shadow_batches[0].instances[0],
        )]);
        let (selected, _, _) = select_shadow_casters_stable(&frame, [0; 3], [0.; 3], 8, &retained);
        assert!(selected.contains(&(1, 0)));
        let (selected, _, _) =
            select_shadow_casters_stable(&frame, [0; 3], [0.; 3], 8, &HashSet::new());
        assert!(!selected.contains(&(1, 0)));
    }
    #[test]
    fn vegetation_material_vertex_layout_matches_shader() {
        assert_eq!(std::mem::size_of::<VegetationVertex>(), 44);
        assert_eq!(std::mem::offset_of!(VegetationVertex, ao), 36);
        assert_eq!(std::mem::offset_of!(VegetationVertex, transmission), 40);
    }
    #[test]
    fn vegetation_shader_validates() {
        for source in [SHADER, SHADOW_SHADER] {
            let module = naga::front::wgsl::parse_str(source).unwrap();
            let (_, quad_type) = module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some("Quad"))
                .unwrap();
            match &quad_type.inner {
                naga::TypeInner::Struct { members, span } => {
                    assert_eq!(*span, std::mem::size_of::<Quad>() as u32);
                    assert_eq!(
                        members.iter().map(|m| m.offset).collect::<Vec<_>>(),
                        vec![0, 12, 20, 24, 36, 40]
                    );
                }
                _ => panic!("Quad must remain a scalar-layout struct"),
            }
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap();
        }
    }
    #[test]
    fn integer_roots_rebase_before_float_conversion() {
        let distant = 1i64 << 54;
        let instance = VegetationInstance {
            root_cell: [distant + 1, -distant - 3, 6],
            quarter_turn: 7,
            fade: 0.75,
        };
        let packed = pack(&instance, [distant, -distant, 2]);
        assert_eq!(packed.root, [0.0625, -0.1875, 0.25]);
        assert_eq!(packed.turn, 3);
        assert_eq!(packed.seed, pack(&instance, [0; 3]).seed);
        assert_eq!(std::mem::size_of::<Instance>(), 32);
    }
    #[test]
    fn extreme_integer_root_subtraction_does_not_overflow() {
        let instance = VegetationInstance {
            root_cell: [i64::MAX, i64::MIN, 0],
            quarter_turn: 0,
            fade: 1.,
        };
        let packed = pack(&instance, [i64::MIN, i64::MAX, 0]);
        assert!(packed.root.into_iter().all(f32::is_finite));
    }
}

#[cfg(test)]
#[path = "vegetation_gpu_parity.rs"]
mod parity;
