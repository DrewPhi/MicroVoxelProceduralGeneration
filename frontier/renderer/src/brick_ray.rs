//! Bounded exact 6.25 cm volume comparison. Greenfield implementation; no LOD.
//!
//! 8³ bricks are uniform words or densely packed 16-bit cells. A cell stores the
//! canonical material and six exposed-face bits sampled with the mesh's halo.
//! Thus traversal never invents a face at a cropped solid boundary. Optional
//! Chebyshev occupancy distances skip only proven-empty brick cubes; this is a
//! small conservative cache experiment, NOT the published NAADF implementation.
use bytemuck::{Pod, Zeroable};
use frontier_world::{Cell, Material};
use glam::{Mat4, Vec3};
use std::collections::VecDeque;
use wgpu::util::DeviceExt;

const BRICK: usize = 8;
const DENSE: u32 = 1 << 31;
pub const SHADER: &str = include_str!("brick_ray.wgsl");

#[derive(Clone, Debug)]
pub struct BrickSnapshot {
    pub origin: Cell,
    pub size: [usize; 3],
    pub brick_size: [usize; 3],
    /// Interleaved uniform-word/dense-offset and safe empty-cube radius.
    pub headers: Vec<u32>,
    pub dense: Vec<u32>,
    pub dense_bricks: usize,
    pub empty_bricks: usize,
}
fn material_id(m: Material) -> u16 {
    match m {
        Material::Air => 0,
        Material::Grass => 1,
        Material::Soil => 2,
        Material::Stone => 3,
        Material::Wood => 4,
    }
}
impl BrickSnapshot {
    /// Call with an immutable World snapshot. The caller atomically publishes
    /// the resulting GPU object together with its mesh/world revision.
    pub fn build(mut sample: impl FnMut(Cell) -> Material, min: Cell, size: [usize; 3]) -> Self {
        assert!(
            size.iter().all(|&n| n > 0 && n <= 4096),
            "bounded comparison extent must be 1..4096 cells per axis"
        );
        let halo = size.map(|n| n + 2);
        let index = |p: [usize; 3]| (p[2] * halo[1] + p[1]) * halo[0] + p[0];
        let mut voxels = vec![0u8; halo.iter().product()];
        for z in 0..halo[2] {
            for y in 0..halo[1] {
                for x in 0..halo[0] {
                    voxels[index([x, y, z])] = material_id(sample(Cell {
                        x: min.x + x as i64 - 1,
                        y: min.y + y as i64 - 1,
                        z: min.z + z as i64 - 1,
                    })) as u8;
                }
            }
        }
        let dims = size.map(|n| n.div_ceil(BRICK));
        let count = dims.iter().product::<usize>();
        let mut out = Self {
            origin: min,
            size,
            brick_size: dims,
            headers: vec![0; count * 2],
            dense: vec![],
            dense_bricks: 0,
            empty_bricks: 0,
        };
        for bz in 0..dims[2] {
            for by in 0..dims[1] {
                for bx in 0..dims[0] {
                    let mut words = [0u16; 512];
                    for z in 0..BRICK {
                        for y in 0..BRICK {
                            for x in 0..BRICK {
                                let p = [bx * BRICK + x, by * BRICK + y, bz * BRICK + z];
                                if (0..3).any(|a| p[a] >= size[a]) {
                                    continue;
                                }
                                let q = p.map(|v| v + 1);
                                let material = voxels[index(q)] as u16;
                                if material == 0 {
                                    continue;
                                }
                                let mut word = material;
                                for axis in 0..3 {
                                    for positive in [false, true] {
                                        let mut n = q;
                                        if positive {
                                            n[axis] += 1;
                                        } else {
                                            n[axis] -= 1;
                                        }
                                        if voxels[index(n)] == 0 {
                                            word |= 1 << (8 + axis * 2 + usize::from(positive));
                                        }
                                    }
                                }
                                words[(z * BRICK + y) * BRICK + x] = word;
                            }
                        }
                    }
                    let bi = (bz * dims[1] + by) * dims[0] + bx;
                    if words.iter().all(|&w| w == words[0]) {
                        out.headers[bi * 2] = words[0] as u32;
                        out.empty_bricks += usize::from(words[0] == 0);
                    } else {
                        out.headers[bi * 2] = DENSE | out.dense.len() as u32;
                        out.dense.extend(
                            words
                                .chunks_exact(2)
                                .map(|p| p[0] as u32 | ((p[1] as u32) << 16)),
                        );
                        out.dense_bricks += 1;
                    }
                }
            }
        }
        // Multi-source 26-neighbor BFS computes exact Chebyshev distance to any
        // nonempty brick. Radius d-1 encloses only empty bricks. Even a uniform
        // interior solid counts occupied, independent of its exposed-face mask.
        let mut distances = vec![u32::MAX; count];
        let mut queue = VecDeque::new();
        for (i, d) in distances.iter_mut().enumerate() {
            if out.headers[i * 2] != 0 {
                *d = 0;
                queue.push_back(i);
            }
        }
        while let Some(i) = queue.pop_front() {
            let p = [
                i % dims[0],
                (i / dims[0]) % dims[1],
                i / (dims[0] * dims[1]),
            ];
            for dz in -1isize..=1 {
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let n = [p[0] as isize + dx, p[1] as isize + dy, p[2] as isize + dz];
                        if (0..3).any(|a| n[a] < 0 || n[a] >= dims[a] as isize) {
                            continue;
                        }
                        let j = (n[2] as usize * dims[1] + n[1] as usize) * dims[0] + n[0] as usize;
                        if distances[j] > distances[i] + 1 {
                            distances[j] = distances[i] + 1;
                            queue.push_back(j);
                        }
                    }
                }
            }
        }
        for (i, d) in distances.into_iter().enumerate() {
            out.headers[i * 2 + 1] = if d == u32::MAX {
                *dims.iter().max().unwrap() as u32
            } else {
                d.saturating_sub(1)
            };
        }
        out
    }
    pub fn gpu_bytes(&self) -> usize {
        (self.headers.len() + self.dense.len().max(1)) * 4
    }
    #[cfg(test)]
    fn word(&self, p: [usize; 3]) -> u32 {
        let b = p.map(|v| v / 8);
        let h = self.headers[2 * ((b[2] * self.brick_size[1] + b[1]) * self.brick_size[0] + b[0])];
        if h & DENSE == 0 {
            h
        } else {
            let i = ((p[2] % 8) * 8 + p[1] % 8) * 8 + p[0] % 8;
            (self.dense[(h & !DENSE) as usize + i / 2] >> ((i % 2) * 16)) & 65535
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RayCamera {
    matrix: [[f32; 4]; 4],
    inverse: [[f32; 4]; 4],
    eye: [f32; 4],
    extent: [u32; 4],
    bricks: [u32; 4],
    viewport: [f32; 4],
}
pub struct BrickRayGpu {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera: wgpu::Buffer,
    size: [usize; 3],
    bricks: [usize; 3],
    pub resident_bytes: usize,
}
impl BrickRayGpu {
    pub fn new(
        device: &wgpu::Device,
        snapshot: &BrickSnapshot,
        format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Result<Self, String> {
        let limit = device.limits().max_storage_buffer_binding_size as usize;
        if snapshot.headers.len() * 4 > limit || snapshot.dense.len() * 4 > limit {
            return Err(format!(
                "ray snapshot exceeds storage binding limit {limit}: headers={} dense={}",
                snapshot.headers.len() * 4,
                snapshot.dense.len() * 4
            ));
        }
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bounded brick ray camera"),
            size: std::mem::size_of::<RayCamera>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let headers = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("brick uniform headers and distance cache"),
            contents: bytemuck::cast_slice(&snapshot.headers),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let dense = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("packed exact material and exposed faces"),
            contents: bytemuck::cast_slice(if snapshot.dense.is_empty() {
                &[0]
            } else {
                &snapshot.dense
            }),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let entries = (0..3)
            .map(|i| wgpu::BindGroupLayoutEntry {
                binding: i,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: if i == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: true }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect::<Vec<_>>();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brick ray layout"),
            entries: &entries,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bounded immutable brick snapshot"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: headers.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dense.as_entire_binding(),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("exact hierarchical brick ray"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brick ray"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bounded exact brick ray"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
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
            primitive: wgpu::PrimitiveState::default(),
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
        Ok(Self {
            pipeline,
            bind_group,
            camera,
            size: snapshot.size,
            bricks: snapshot.brick_size,
            resident_bytes: snapshot.gpu_bytes() + std::mem::size_of::<RayCamera>(),
        })
    }
    /// Matrix/eye use the snapshot's integer origin in local meters, exactly as
    /// the native mesh. Full resolution, no temporal reconstruction or filtering.
    pub fn update_camera(
        &self,
        queue: &wgpu::Queue,
        matrix: Mat4,
        eye_local: Vec3,
        width: u32,
        height: u32,
        distance_skip: bool,
    ) {
        let data = RayCamera {
            matrix: matrix.to_cols_array_2d(),
            inverse: matrix.inverse().to_cols_array_2d(),
            eye: eye_local.extend(1.).to_array(),
            extent: [
                self.size[0] as u32,
                self.size[1] as u32,
                self.size[2] as u32,
                u32::from(distance_skip),
            ],
            bricks: [
                self.bricks[0] as u32,
                self.bricks[1] as u32,
                self.bricks[2] as u32,
                0,
            ],
            viewport: [width as f32, height as f32, 0., 0.],
        };
        queue.write_buffer(&self.camera, 0, bytemuck::bytes_of(&data));
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests;
