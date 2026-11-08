//! Reusable opaque voxel template geometry, one instanced draw per model/LOD.
//! Canonical roots remain integers until after origin subtraction.
use crate::vegetation::{MeshKey, VegetationFrame, VegetationInstance, VegetationVertex};
use bytemuck::{Pod, Zeroable};
use std::collections::{HashMap, HashSet};
use wgpu::util::DeviceExt;

const CACHE_BYTES: u64 = 256 * 1024 * 1024;
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

struct Template {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    instance_count: u32,
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
    pub resident_bytes: u64,
    pub uploaded_bytes: u64,
}

pub struct VegetationGpu {
    pipeline: wgpu::RenderPipeline,
    lighting_uniform: wgpu::Buffer,
    lighting_bind: wgpu::BindGroup,
    templates: HashMap<MeshKey, Template>,
    active: Vec<MeshKey>,
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
            bind_group_layouts: &[camera_layout, &lighting_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced voxel vegetation"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let vertex_attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3];
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
        Self {
            pipeline,
            lighting_uniform,
            lighting_bind,
            templates: HashMap::new(),
            active: vec![],
            scratch: vec![],
            stats: Default::default(),
        }
    }

    pub fn set_visual_lighting(
        &self,
        queue: &wgpu::Queue,
        settings: &crate::settings::VisualSettings,
        regional: bool,
    ) {
        let sun = crate::atmosphere::sun_direction(settings.sun_elevation, settings.sun_azimuth);
        let lighting = [
            sun.x,
            sun.y,
            sun.z,
            if regional { 1. } else { 0. },
            settings.overcast,
            0.,
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
    ) -> Result<(), String> {
        // A refused new frame must never draw old instance positions with the
        // new camera origin. Immutable templates remain available for retry.
        self.active.clear();
        self.stats.draw_calls = 0;
        self.stats.instances = 0;
        self.stats.uploaded_bytes = 0;
        let wanted: HashSet<_> = frame
            .batches
            .iter()
            .filter(|b| !b.instances.is_empty() && !b.mesh.indices.is_empty())
            .map(|b| b.key)
            .collect();
        let mut needed = 0u64;
        for batch in &frame.batches {
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
        if needed > CACHE_BYTES {
            return Err("vegetation active templates exceed 256 MiB budget".into());
        }
        let mut resident: u64 = self.templates.values().map(Template::bytes).sum();
        let added = needed.saturating_sub(
            self.templates
                .iter()
                .filter(|(k, _)| wanted.contains(*k))
                .map(|(_, t)| t.bytes())
                .sum(),
        );
        if resident + added > CACHE_BYTES {
            let mut unused: Vec<_> = self
                .templates
                .keys()
                .filter(|k| !wanted.contains(*k))
                .copied()
                .collect();
            unused.sort();
            for key in unused {
                resident -= self.templates.remove(&key).unwrap().bytes();
                if resident + added <= CACHE_BYTES {
                    break;
                }
            }
        }
        self.active.clear();
        self.stats.uploaded_bytes = 0;
        self.stats.instances = 0;
        for batch in &frame.batches {
            if !wanted.contains(&batch.key) {
                continue;
            }
            self.scratch.clear();
            self.scratch.extend(
                batch
                    .instances
                    .iter()
                    .map(|instance| pack(instance, origin)),
            );
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
                    instance_count: 0,
                    geometry_bytes: (vertex_bytes.len() + index_bytes.len()) as u64,
                }
            });
            if capacity > template.instance_capacity {
                template.instances = instance_buffer(device, capacity);
                template.instance_capacity = capacity;
            }
            queue.write_buffer(&template.instances, 0, bytes);
            template.instance_count = self.scratch.len() as u32;
            self.stats.uploaded_bytes += bytes.len() as u64;
            self.stats.instances += self.scratch.len();
            self.active.push(batch.key);
        }
        self.stats.cached_templates = self.templates.len();
        self.stats.draw_calls = self.active.len();
        self.stats.resident_bytes = self.templates.values().map(Template::bytes).sum();
        Ok(())
    }
    pub fn stats(&self) -> VegetationGpuStats {
        self.stats
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup) {
        if self.active.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, &self.lighting_bind, &[]);
        for key in &self.active {
            let template = &self.templates[key];
            pass.set_vertex_buffer(0, template.vertices.slice(..));
            pass.set_vertex_buffer(1, template.instances.slice(..));
            pass.set_index_buffer(template.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..template.index_count, 0, 0..template.instance_count);
        }
    }
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
    #[test]
    fn vegetation_shader_validates() {
        let module = naga::front::wgsl::parse_str(SHADER).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
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
