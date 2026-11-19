//! Optional GPU direction culling and pooled indirect draws for coarse voxel
//! quads. CPU publication, visibility proofs and the shadow path stay separate.
use super::{AdaptiveGpu, Cluster, CAMERA_UNIFORM_BYTES, MAX_CLUSTERS, UNIFORM_STRIDE};
use std::{collections::BTreeMap, ops::Range};

const MAX_COMMANDS: usize = MAX_CLUSTERS * 6;
const INDIRECT_STRIDE: u64 = 20;

pub(crate) fn requested() -> bool {
    crate::adaptive_geometry::distant_voxels::enabled()
        && std::env::var_os("FRONTIER_VOXY_GPU_BATCH").is_some_and(|v| v == "1")
}

pub(crate) fn features() -> wgpu::Features {
    wgpu::Features::MULTI_DRAW_INDIRECT | wgpu::Features::INDIRECT_FIRST_INSTANCE
}

pub(super) fn supported(device: &wgpu::Device) -> bool {
    requested()
        && device.features().contains(features())
        && !crate::ground_cover::shell_enabled()
        && std::env::var("FRONTIER_NAADF_SPLIT_SHADERS").as_deref() != Ok("1")
}

pub(super) fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("coarse voxel batch raster layout"),
        entries: &[1, 3].map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }),
    })
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Candidate {
    first: u32,
    count: u32,
    slot: u32,
    axis_sign: u32,
    planes: [f32; 2],
    padding: [u32; 2],
}

struct Group {
    vertex_page: usize,
    profile_page: usize,
    commands: Range<usize>,
}

pub(super) struct Batch {
    layout: wgpu::BindGroupLayout,
    pub raster: wgpu::RenderPipeline,
    pub profile_binds: Vec<wgpu::BindGroup>,
    cameras: wgpu::Buffer,
    candidates: wgpu::Buffer,
    indirect: wgpu::Buffer,
    compute: wgpu::ComputePipeline,
    compute_bind: wgpu::BindGroup,
    groups: Vec<Group>,
    count: usize,
}

impl Batch {
    pub(super) fn new(
        device: &wgpu::Device,
        layout: wgpu::BindGroupLayout,
        raster: wgpu::RenderPipeline,
    ) -> Self {
        use wgpu::BufferUsages as U;
        let buffer = |label: &'static str, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let cameras = buffer(
            "coarse voxel cameras by slot",
            CAMERA_UNIFORM_BYTES,
            U::STORAGE | U::COPY_DST,
        );
        let candidates = buffer(
            "coarse voxel direction bins",
            (MAX_COMMANDS * size_of::<Candidate>()) as u64,
            U::STORAGE | U::COPY_DST,
        );
        let indirect = buffer(
            "coarse voxel indirect commands",
            MAX_COMMANDS as u64 * INDIRECT_STRIDE,
            U::STORAGE | U::INDIRECT,
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("coarse voxel direction culling"),
            source: wgpu::ShaderSource::Wgsl(include_str!("coarse_batch.wgsl").into()),
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("coarse voxel direction culling"),
            layout: None,
            module: &module,
            entry_point: Some("cull"),
            compilation_options: Default::default(),
            cache: None,
        });
        let compute_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("coarse voxel culling data"),
            layout: &compute.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: candidates.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: cameras.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: indirect.as_entire_binding(),
                },
            ],
        });
        Self {
            layout,
            raster,
            profile_binds: vec![],
            cameras,
            candidates,
            indirect,
            compute,
            compute_bind,
            groups: vec![],
            count: 0,
        }
    }

    pub(super) fn add_profile_page(&mut self, device: &wgpu::Device, profile: &wgpu::Buffer) {
        self.profile_binds
            .push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("coarse voxel batch profile page"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: profile.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.cameras.as_entire_binding(),
                    },
                ],
            }));
    }

    pub(super) fn accepts(cluster: &Cluster) -> bool {
        cluster.native_quads && !cluster.exact && cluster.vertices.range.start % 32 == 0
    }

    pub(super) fn bytes() -> u64 {
        CAMERA_UNIFORM_BYTES
            + MAX_COMMANDS as u64 * (size_of::<Candidate>() as u64 + INDIRECT_STRIDE)
    }

    pub(super) fn stats(&self) -> (usize, usize) {
        (self.groups.len(), self.count)
    }

    /// Runs after CPU publication/frustum culling. Each consecutive GPU group
    /// shares its quad arena and appearance table. Near terrain stays on the
    /// plain path, and views never read back the result of direction culling.
    pub(super) fn prepare(renderer: &mut AdaptiveGpu, queue: &wgpu::Queue) {
        let Some(batch) = &mut renderer.coarse_batch else {
            return;
        };
        let mut lists = BTreeMap::<(usize, usize), Vec<Candidate>>::new();
        let stride = UNIFORM_STRIDE as usize;
        let mut cameras = Vec::new();
        for (i, draw) in renderer.visible.iter().enumerate() {
            let c = &renderer.clusters[&draw.key];
            if !Self::accepts(c) {
                continue;
            }
            let camera = c.slot as usize * stride;
            if cameras.len() < camera + stride {
                cameras.resize(camera + stride, 0);
            }
            cameras[camera..camera + stride]
                .copy_from_slice(&renderer.camera_upload[i * stride..(i + 1) * stride]);
            let first = (c.vertices.range.start / 32) as u32;
            let list = lists
                .entry((c.vertices.page, c.profile.as_ref().unwrap().table.page))
                .or_default();
            if c.bins.is_empty() || c.bins.len() > 6 {
                list.push(Candidate {
                    first,
                    count: c.count,
                    slot: c.slot,
                    axis_sign: 3,
                    planes: [0.; 2],
                    padding: [0; 2],
                });
            } else {
                list.extend(c.bins.iter().map(|bin| Candidate {
                    first: first + bin.first_quad,
                    count: bin.quad_count,
                    slot: c.slot,
                    axis_sign: bin.axis_sign,
                    planes: [bin.min_plane, bin.max_plane],
                    padding: [0; 2],
                }));
            }
        }
        batch.groups.clear();
        let mut commands = Vec::new();
        for ((vertex_page, profile_page), list) in lists {
            let start = commands.len();
            commands.extend(list);
            batch.groups.push(Group {
                vertex_page,
                profile_page,
                commands: start..commands.len(),
            });
        }
        batch.count = commands.len();
        if batch.count != 0 {
            assert!(batch.count <= MAX_COMMANDS);
            queue.write_buffer(&batch.cameras, 0, &cameras);
            queue.write_buffer(&batch.candidates, 0, bytemuck::cast_slice(&commands));
        }
    }

    pub(super) fn encode_cull(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.count == 0 {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("coarse voxel direction cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.compute);
        pass.set_bind_group(0, &self.compute_bind, &[]);
        pass.dispatch_workgroups(self.count.div_ceil(64) as u32, 1, 1);
    }

    pub(super) fn draw<'a>(&'a self, renderer: &'a AdaptiveGpu, pass: &mut wgpu::RenderPass<'a>) {
        if self.groups.is_empty() {
            return;
        }
        pass.set_pipeline(&self.raster);
        pass.set_bind_group(1, &renderer.shadows.sampling_bind, &[]);
        pass.set_bind_group(2, &renderer.shadows.near.bind, &[]);
        pass.set_index_buffer(renderer.quad_indices.slice(..), wgpu::IndexFormat::Uint32);
        for group in &self.groups {
            pass.set_bind_group(0, &self.profile_binds[group.profile_page], &[]);
            pass.set_vertex_buffer(0, renderer.vertices[group.vertex_page].buffer.slice(..));
            pass.multi_draw_indexed_indirect(
                &self.indirect,
                group.commands.start as u64 * INDIRECT_STRIDE,
                group.commands.len() as u32,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn indirect_and_camera_layout_match_the_compute_shader() {
        assert_eq!(size_of::<super::Candidate>(), 32);
        assert_eq!(super::INDIRECT_STRIDE, 20);
        assert_eq!(super::UNIFORM_STRIDE, 256);
        let shader = naga::front::wgsl::parse_str(include_str!("coarse_batch.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&shader)
        .unwrap();
    }
}
