//! Bounded GPU selection of columns owned by the current adaptive view.
use super::{
    coarse_projection::Mode, profile_bind_source, AdaptiveGpu, CAMERA_UNIFORM_BYTES, MAX_CLUSTERS,
};
use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap},
    ops::Range,
};

const MAX_COLUMNS: usize = 1 << 20;
const MAX_JOB_SLOTS: usize = 2 * MAX_CLUSTERS;
const INDIRECT_BYTES: u64 = 16;
const WORD_BYTES: u64 = 4;
const MAX_REGIONS_PER_COLUMN_GROUP: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Job {
    source: u32,
    count: u32,
    draw: u32,
    output: u32,
    regions: u32,
    padding: [u32; 3],
}

struct Dispatch {
    bind: wgpu::BindGroup,
    count: u32,
}

pub(super) struct Draw {
    pub range: Range<u64>,
    source_count: u32,
}

pub(super) struct Compaction {
    pipeline: wgpu::ComputePipeline,
    normal: wgpu::RenderPipeline,
    far: Option<wgpu::RenderPipeline>,
    jobs: wgpu::Buffer,
    pub compacted: wgpu::Buffer,
    pub indirect: wgpu::Buffer,
    dispatches: Vec<Dispatch>,
    draws: Vec<Option<Draw>>,
    render_binds: HashMap<(usize, usize), wgpu::BindGroup>,
    pending: Cell<bool>,
    encoded_dispatches: Cell<usize>,
    regions: u64,
}

impl Compaction {
    pub(super) const fn bytes() -> u64 {
        (MAX_JOB_SLOTS * size_of::<Job>()) as u64
            + MAX_COLUMNS as u64 * WORD_BYTES
            + MAX_CLUSTERS as u64 * INDIRECT_BYTES
    }

    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        mode: Mode,
        constants: &HashMap<String, f64>,
        far_constants: Option<&HashMap<String, f64>>,
    ) -> Option<Self> {
        use wgpu::BufferUsages as U;
        let limits = device.limits();
        let storage_limit =
            u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        if storage_limit
            < CAMERA_UNIFORM_BYTES
                .max(MAX_COLUMNS as u64 * WORD_BYTES)
                .max(super::PAGE_BYTES)
            || limits.max_storage_buffers_per_shader_stage < 5
        {
            return None;
        }
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let jobs = buffer(
            "coarse compaction jobs",
            (MAX_JOB_SLOTS * size_of::<Job>()) as u64,
            U::STORAGE | U::COPY_DST,
        );
        let compacted = buffer(
            "selected coarse column indices",
            MAX_COLUMNS as u64 * WORD_BYTES,
            U::STORAGE | U::VERTEX,
        );
        let indirect = buffer(
            "selected coarse column indirect draws",
            MAX_CLUSTERS as u64 * INDIRECT_BYTES,
            U::STORAGE | U::INDIRECT | U::COPY_SRC,
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("coarse column selection"),
            source: wgpu::ShaderSource::Wgsl(include_str!("coarse_compaction.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("coarse column selection"),
            layout: None,
            module: &module,
            entry_point: Some(match mode {
                Mode::Quads => "compact_quads",
                Mode::Boxes => "compact_boxes",
            }),
            compilation_options: Default::default(),
            cache: None,
        });
        let (vertex, fragment, topology, cull_mode) = match mode {
            Mode::Quads => (
                "vs_coarse_generated_indexed",
                "fs_main",
                wgpu::PrimitiveTopology::TriangleList,
                Some(wgpu::Face::Back),
            ),
            Mode::Boxes => (
                "vs_coarse_splat_indexed",
                "fs_coarse_splat",
                wgpu::PrimitiveTopology::TriangleStrip,
                None,
            ),
        };
        let attributes = wgpu::vertex_attr_array![0=>Uint32];
        let raster = |constants| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("compacted coarse column projection"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vertex),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        ..Default::default()
                    },
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: WORD_BYTES,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fragment),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        Some(Self {
            pipeline,
            normal: raster(constants),
            far: far_constants.map(raster),
            jobs,
            compacted,
            indirect,
            dispatches: vec![],
            draws: vec![],
            render_binds: HashMap::new(),
            pending: Cell::new(false),
            encoded_dispatches: Cell::new(0),
            regions: 0,
        })
    }

    pub(super) fn prepare(renderer: &mut AdaptiveGpu, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(compaction) = &mut renderer.coarse_compaction else {
            return;
        };
        compaction.draws.clear();
        compaction
            .draws
            .resize_with(renderer.visible.len(), || None);
        compaction.dispatches.clear();
        compaction.regions = 0;
        let mut by_page = BTreeMap::<usize, Vec<Job>>::new();
        let mut reserved_columns = 0usize;
        for (draw, item) in renderer.visible.iter().enumerate() {
            let cluster = &renderer.clusters[&item.key];
            let Some(resident) = &cluster.projection else {
                continue;
            };
            let columns = resident.columns as usize;
            if columns == 0
                || columns > 256
                || resident.regions != resident.columns.div_ceil(MAX_REGIONS_PER_COLUMN_GROUP)
                || reserved_columns + columns > MAX_COLUMNS
            {
                continue;
            }
            let page = resident.allocation.page;
            let profile = cluster.profile.as_ref().unwrap().table.page;
            let source = &renderer.projection_vertices[page];
            if source.size > u64::from(device.limits().max_storage_buffer_binding_size) {
                continue;
            }
            compaction
                .render_binds
                .entry((page, profile))
                .or_insert_with(|| {
                    profile_bind_source(
                        device,
                        &renderer.layout,
                        &renderer.uniform,
                        &renderer.profile_tables[profile].buffer,
                        &source.buffer,
                    )
                });
            let job = Job {
                source: (resident.column_range().start / WORD_BYTES) as u32,
                count: resident.columns,
                draw: draw as u32,
                output: reserved_columns as u32,
                regions: (resident.region_range().start / WORD_BYTES) as u32,
                padding: [0; 3],
            };
            by_page.entry(page).or_default().push(job);
            reserved_columns += columns;
        }
        let job_stride = size_of::<Job>() as u64;
        let alignment =
            u64::from(device.limits().min_storage_buffer_offset_alignment).max(job_stride);
        let job_alignment = (alignment / job_stride) as usize;
        let layout = compaction.pipeline.get_bind_group_layout(0);
        let mut jobs = Vec::new();
        for (page, entries) in by_page {
            let first = jobs.len().next_multiple_of(job_alignment);
            if first + entries.len() > MAX_JOB_SLOTS {
                continue;
            }
            jobs.resize(first, bytemuck::Zeroable::zeroed());
            jobs.extend_from_slice(&entries);
            let source = &renderer.projection_vertices[page].buffer;
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("coarse selection per source page"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: source.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: renderer.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &compaction.jobs,
                            offset: first as u64 * job_stride,
                            size: std::num::NonZeroU64::new(entries.len() as u64 * job_stride),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: compaction.compacted.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: compaction.indirect.as_entire_binding(),
                    },
                ],
            });
            for job in &entries {
                let start = u64::from(job.output) * WORD_BYTES;
                let bytes = u64::from(job.count) * WORD_BYTES;
                compaction.draws[job.draw as usize] = Some(Draw {
                    range: start..start + bytes,
                    source_count: job.count,
                });
                compaction.regions += u64::from(job.count.div_ceil(MAX_REGIONS_PER_COLUMN_GROUP));
            }
            compaction.dispatches.push(Dispatch {
                bind,
                count: entries.len() as u32,
            });
        }
        if !jobs.is_empty() {
            queue.write_buffer(&compaction.jobs, 0, bytemuck::cast_slice(&jobs));
        }
        compaction.pending.set(!jobs.is_empty());
    }

    pub(super) fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        // PreparedView reuses the source buffers, camera and draw order together.
        if !self.pending.replace(false) {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("coarse projection column selection"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        for dispatch in &self.dispatches {
            pass.set_bind_group(0, &dispatch.bind, &[]);
            pass.dispatch_workgroups(dispatch.count, 1, 1);
        }
        self.encoded_dispatches.set(
            self.encoded_dispatches
                .get()
                .saturating_add(self.dispatches.len()),
        );
    }

    pub(super) fn draw(&self, draw: usize) -> Option<&Draw> {
        self.draws.get(draw)?.as_ref()
    }

    pub(super) fn render_bind(&self, page: usize, profile: usize) -> &wgpu::BindGroup {
        &self.render_binds[&(page, profile)]
    }

    pub(super) fn raster_pipeline(&self, far: bool) -> &wgpu::RenderPipeline {
        if far {
            self.far.as_ref().unwrap_or(&self.normal)
        } else {
            &self.normal
        }
    }

    pub(super) const fn indirect_offset(draw: usize) -> u64 {
        draw as u64 * INDIRECT_BYTES
    }

    pub(super) fn stats(&self) -> (u64, usize) {
        (self.regions, self.encoded_dispatches.get())
    }

    pub(super) fn capture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        expected_vertices: u32,
    ) -> Result<String, String> {
        let mut candidates = 0u64;
        let mut selected = 0u64;
        let mut compacted_tiles = 0usize;
        if self.draws.iter().any(Option::is_some) {
            let bytes = Self::indirect_offset(self.draws.len());
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("coarse indirect count capture"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(&self.indirect, 0, &staging, 0, bytes);
            queue.submit(Some(encoder.finish()));
            let (tx, rx) = std::sync::mpsc::channel();
            staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            device.poll(wgpu::Maintain::Wait);
            rx.recv()
                .map_err(|err| err.to_string())?
                .map_err(|err| err.to_string())?;
            let contents = staging.slice(..).get_mapped_range();
            let commands: &[[u32; 4]] = bytemuck::cast_slice(&contents);
            for (draw, command) in self.draws.iter().zip(commands) {
                let Some(draw) = draw else { continue };
                if command[0] != expected_vertices
                    || command[1] > draw.source_count
                    || command[2] != 0
                    || command[3] != 0
                {
                    return Err(format!(
                        "invalid coarse indirect command {command:?} for {} candidates",
                        draw.source_count
                    ));
                }
                candidates += u64::from(draw.source_count);
                selected += u64::from(command[1]);
                compacted_tiles += 1;
            }
        }
        Ok(format!(
            "{{\"compacted_tiles\":{compacted_tiles},\"source_columns\":{candidates},\"selected_columns\":{selected},\"candidate_regions\":{},\"encoded_dispatches\":{},\"commands_verified\":true}}",
            self.regions, self.encoded_dispatches.get()
        ))
    }
}
