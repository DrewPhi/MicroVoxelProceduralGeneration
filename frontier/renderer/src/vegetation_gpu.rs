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

struct Template {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
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
    pub resident_bytes: u64,
    pub uploaded_bytes: u64,
    pub cache_budget_bytes: u64,
}

pub struct VegetationGpu {
    pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_enabled: bool,
    lighting_uniform: wgpu::Buffer,
    lighting_bind: wgpu::BindGroup,
    templates: HashMap<MeshKey, Template>,
    active: Vec<MeshKey>,
    shadow_active: Vec<MeshKey>,
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("instanced voxel vegetation"),
            bind_group_layouts: &[
                camera_layout,
                &lighting_layout,
                &crate::adaptive_gpu::adaptive_shadow::near_sampling_layout(device),
            ],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced voxel vegetation"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let vertex_attributes =
            wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3,7=>Float32,8=>Float32];
        let instance_attributes =
            wgpu::vertex_attr_array![3=>Float32x3,4=>Uint32,5=>Float32,6=>Uint32];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("instanced voxel vegetation"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<VegetationVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attributes,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attributes,
                    },
                ],
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
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("instanced plant shadow depth"),
            bind_group_layouts: &[
                camera_layout,
                &crate::adaptive_gpu::adaptive_shadow::near_depth_layout(device),
            ],
            push_constant_ranges: &[],
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("instanced plant shadow depth"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<VegetationVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attributes,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attributes,
                    },
                ],
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
            shadow_enabled: false,
            pipeline,
            lighting_uniform,
            lighting_bind,
            templates: HashMap::new(),
            active: vec![],
            shadow_active: vec![],
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
            let geometry = (batch.mesh.vertices.len() * std::mem::size_of::<VegetationVertex>()
                + batch.mesh.indices.len() * 4) as u64;
            let instances = ((batch.instances.len() * std::mem::size_of::<Instance>()) as u64)
                .next_power_of_two()
                .max(32);
            let bytes = self
                .templates
                .get(&batch.key)
                .map_or(geometry + instances, |t| {
                    t.geometry_bytes + t.instance_capacity.max(instances)
                });
            if geometry > device.limits().max_buffer_size
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
        let (shadow_selection, shadow_triangles) = if self.shadow_enabled {
            select_shadow_casters(frame, origin, camera_local, 200_000)
        } else {
            (HashSet::new(), 0)
        };
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
                let vertex_bytes = bytemuck::cast_slice(&batch.mesh.vertices);
                let index_bytes = bytemuck::cast_slice(&batch.mesh.indices);
                self.stats.uploaded_bytes += (vertex_bytes.len() + index_bytes.len()) as u64;
                Template {
                    vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("cached vegetation template vertices"),
                        contents: vertex_bytes,
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                    indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("cached vegetation template indices"),
                        contents: index_bytes,
                        usage: wgpu::BufferUsages::INDEX,
                    }),
                    index_count: batch.mesh.indices.len() as u32,
                    instances: instance_buffer(device, capacity),
                    instance_capacity: capacity,
                    last_instance_bytes: Vec::new(),
                    instance_count: 0,
                    shadow_count: 0,
                    geometry_bytes: (vertex_bytes.len() + index_bytes.len()) as u64,
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
            pass.set_vertex_buffer(0, template.vertices.slice(..));
            pass.set_vertex_buffer(1, template.instances.slice(..));
            pass.set_index_buffer(template.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..template.index_count, 0, 0..template.instance_count);
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
            pass.set_vertex_buffer(0, t.vertices.slice(..));
            pass.set_vertex_buffer(1, t.instances.slice(..));
            pass.set_index_buffer(t.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..t.index_count, 0, 0..t.shadow_count);
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
/// Stable distance priority within tree and close ground-cover allocations.
/// Color plants and bounded near offscreen trees share this allocation.
/// Unstreamed trees and partly fading plants remain outside its coverage.
fn select_shadow_casters(
    frame: &VegetationFrame,
    origin: [i64; 3],
    eye: [f32; 3],
    triangle_budget: usize,
) -> (HashSet<(usize, usize)>, usize) {
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
                90.
            } else {
                18. + 32f32.sqrt()
            };
            if distance < radius * radius {
                candidates.push((distance, batch_index, index, batch.mesh.indices.len() / 3));
            }
        }
    }
    candidates.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| (a.1, a.2).cmp(&(b.1, b.2)))
    });
    let mut selected = HashSet::new();
    let mut triangles = 0;
    // Give nearby ground cover a quarter of the existing cap before trees.
    // Unused space is borrowed, so sparse meadows do not waste tree coverage.
    // Do not add a second depth pass, map, or unbounded caster generation.
    let ground_reserve = triangle_budget / 4;
    for &(_, batch, index, cost) in &candidates {
        if frame_batch(frame, batch).key.species != 2 && cost <= ground_reserve - triangles {
            selected.insert((batch, index));
            triangles += cost;
        }
    }
    for &(_, batch, index, cost) in &candidates {
        if frame_batch(frame, batch).key.species == 2 && cost <= triangle_budget - triangles {
            selected.insert((batch, index));
            triangles += cost;
        }
    }
    for (_, batch, index, cost) in candidates {
        if !selected.contains(&(batch, index)) && cost <= triangle_budget - triangles {
            selected.insert((batch, index));
            triangles += cost;
        }
    }
    (selected, triangles)
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
