//! Disposable, opt-in traversal hints. The occupancy hierarchy stays authoritative.
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    sync::OnceLock,
};
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Default)]
pub(super) struct Options {
    pub(super) cache: u32,
    pub(super) height_tile: Option<u32>,
    pub(super) height_mips: bool,
    pub(super) height_gate: bool,
    pub(super) height_probe: Option<u32>,
    pub(super) beam: bool,
    pub(super) diagnostics: bool,
    pub(super) local_exit_repair: bool,
}

pub(super) fn options() -> Options {
    static OPTIONS: OnceLock<Options> = OnceLock::new();
    *OPTIONS.get_or_init(|| Options {
        cache: match std::env::var("FRONTIER_NAADF_CACHE_NODES").as_deref() {
            Ok("1") => 1,
            Ok("2") => 2,
            _ => 0,
        },
        height_tile: std::env::var("FRONTIER_NAADF_HEIGHT_TILE_ROOTS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|n| matches!(n, 1 | 4 | 8)),
        height_mips: enabled("FRONTIER_NAADF_HEIGHT_MIPS"),
        height_gate: enabled("FRONTIER_NAADF_HEIGHT_GATE"),
        height_probe: std::env::var("FRONTIER_NAADF_HEIGHT_PROBE_LEVEL")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|level| *level <= 9),
        beam: enabled("FRONTIER_NAADF_BEAM"),
        diagnostics: enabled("FRONTIER_NAADF_DIAGNOSTICS"),
        local_exit_repair: enabled("FRONTIER_NAADF_LOCAL_EXIT_REPAIR")
            && super::root_distance::direct_publication(),
    })
}

fn enabled(key: &str) -> bool {
    std::env::var_os(key).is_some_and(|v| v == "1")
}

impl Options {
    pub(super) fn height_enabled(self) -> bool {
        self.height_tile.is_some() || self.mips_enabled() || self.height_gate
    }
    pub(super) fn mips_enabled(self) -> bool {
        self.height_mips || self.height_probe.is_some()
    }
    pub(super) fn tile(self) -> u32 {
        self.height_tile.unwrap_or(8)
    }
    pub(super) fn mesh_constants(self) -> HashMap<String, f64> {
        HashMap::from([
            (
                "traversal_direct_roots".into(),
                u8::from(super::root_distance::direct_publication()).into(),
            ),
            (
                "traversal_local_exit_repair".into(),
                u8::from(self.local_exit_repair).into(),
            ),
            (
                "traversal_metrics".into(),
                u8::from(self.diagnostics).into(),
            ),
            (
                "traversal_cache_nodes".into(),
                u8::from(self.cache == 1).into(),
            ),
            (
                "traversal_cache_nodes_compact".into(),
                u8::from(self.cache == 2).into(),
            ),
            (
                "traversal_height_bounds".into(),
                u8::from(self.height_enabled()).into(),
            ),
            ("traversal_height_tile".into(), self.tile().into()),
            (
                "traversal_height_mips".into(),
                u8::from(self.mips_enabled()).into(),
            ),
            (
                "traversal_height_probe".into(),
                self.height_probe.unwrap_or(u32::MAX).into(),
            ),
            (
                "traversal_height_gate".into(),
                u8::from(self.height_gate).into(),
            ),
        ])
    }
}

const HEIGHT_SHADER: &str = include_str!("height_bounds.wgsl");
const HEIGHT_REDUCE: &str = include_str!("height_reduce.wgsl");
const BEAM_SHADER: &str = concat!(
    "const NAADF_GROUP:u32=0u;const NAADF_BASE:u32=3u;\n",
    include_str!("trace.wgsl"),
    include_str!("beam.wgsl"),
);

fn uint_texture(
    device: &wgpu::Device,
    label: &'static str,
    size: [u32; 2],
    write: bool,
) -> wgpu::TextureView {
    create_uint_texture(device, label, size, write, 1).create_view(&Default::default())
}

fn create_uint_texture(
    device: &wgpu::Device,
    label: &'static str,
    size: [u32; 2],
    write: bool,
    levels: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | if write {
                wgpu::TextureUsages::STORAGE_BINDING
            } else {
                wgpu::TextureUsages::empty()
            },
        view_formats: &[],
    })
}

pub(super) fn empty_height(device: &wgpu::Device) -> wgpu::TextureView {
    uint_texture(device, "inactive NAADF height bounds", [1; 2], false)
}

fn compute(
    device: &wgpu::Device,
    label: &'static str,
    shader: &'static str,
    tile: Option<u32>,
    metrics: Option<bool>,
) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions {
            constants: &tile
                .into_iter()
                .map(|tile| ("height_tile_roots".into(), tile.into()))
                .chain(
                    metrics
                        .into_iter()
                        .map(|enabled| ("traversal_metrics".into(), u8::from(enabled).into())),
                )
                .collect(),
            ..Default::default()
        },
        cache: None,
    })
}

fn entry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

pub(super) struct Height {
    published: Cell<Option<u64>>,
    settings: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
    edge: u32,
    reduction: Option<Reduction>,
}

struct Reduction {
    pipeline: wgpu::ComputePipeline,
    levels: Vec<(wgpu::BindGroup, u32)>,
}

impl Height {
    pub(super) fn new(
        device: &wgpu::Device,
        n: u32,
        roots: &wgpu::TextureView,
        options: Options,
    ) -> (Self, wgpu::TextureView) {
        let tile = options.tile();
        let mut edge = n.div_ceil(tile);
        let mut levels = 1;
        if options.mips_enabled() {
            // Power-of-two storage preserves all partial XZ tiles through the
            // hardware mip layout. The base kernel fills the padding with zero.
            edge = edge.next_power_of_two();
            levels = edge.ilog2() + 1;
        }
        let texture = create_uint_texture(
            device,
            "NAADF occupied height bounds",
            [edge; 2],
            true,
            levels,
        );
        let mip = |level| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let view = mip(0);
        let pipeline = compute(
            device,
            "NAADF max occupied root height",
            HEIGHT_SHADER,
            Some(tile),
            None,
        );
        let settings = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("NAADF max height root layout"),
            contents: &[0; 32],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NAADF max occupied root height"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: settings.as_entire_binding(),
                },
                entry(1, roots),
                entry(2, &view),
            ],
        });
        let reduction = (levels > 1).then(|| {
            let pipeline = compute(
                device,
                "NAADF occupied height mip reduction",
                HEIGHT_REDUCE,
                None,
                None,
            );
            let layout = pipeline.get_bind_group_layout(0);
            let levels = (1..levels)
                .map(|level| {
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("NAADF occupied height mip views"),
                        layout: &layout,
                        entries: &[entry(0, &mip(level - 1)), entry(1, &mip(level))],
                    });
                    (bind, edge >> level)
                })
                .collect();
            Reduction { pipeline, levels }
        });
        (
            Self {
                published: Cell::new(None),
                settings,
                pipeline,
                bind,
                edge,
                reduction,
            },
            texture.create_view(&Default::default()),
        )
    }

    pub(super) fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        owner: u64,
        n: u32,
        sparse: bool,
        ring: [u32; 3],
        stages: Option<&std::sync::Arc<super::gpu_stages::Profiler>>,
    ) {
        if self.published.get() == Some(owner) {
            return;
        }
        self.published.set(Some(owner));
        queue.write_buffer(
            &self.settings,
            0,
            bytemuck::cast_slice(&[n, u32::from(sparse), 0, 0, ring[0], ring[1], ring[2], 0]),
        );
        let timing = stages.and_then(|p| p.begin(super::gpu_stages::Stage::HeightBase));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("NAADF occupied-height base"),
            timestamp_writes: timing.as_ref().map(|t| t.writes(true, true)),
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.dispatch_workgroups(self.edge, self.edge, 1);
        drop(pass);
        if let Some(t) = timing {
            t.resolve(encoder);
            t.defer();
        }
        if let Some(reduction) = &self.reduction {
            let timing = stages.and_then(|p| p.begin(super::gpu_stages::Stage::HeightReduce));
            for (index, (bind, edge)) in reduction.levels.iter().enumerate() {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("NAADF occupied-height reduction"),
                    timestamp_writes: timing
                        .as_ref()
                        .filter(|_| index == 0 || index + 1 == reduction.levels.len())
                        .map(|t| t.writes(index == 0, index + 1 == reduction.levels.len())),
                });
                pass.set_pipeline(&reduction.pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(edge.div_ceil(8), edge.div_ceil(8), 1);
            }
            if let Some(t) = timing {
                t.resolve(encoder);
                t.defer();
            }
        }
    }

    pub(super) fn levels(&self) -> u32 {
        1 + self.reduction.as_ref().map_or(0, |r| r.levels.len() as u32)
    }

    pub(super) fn bytes(&self) -> usize {
        (0..self.levels())
            .map(|level| ((self.edge >> level) as usize).pow(2) * 4)
            .sum::<usize>()
            + 32
    }
}

pub(super) fn beam_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("NAADF beam prefix sampling"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

struct BeamTarget {
    sample: wgpu::BindGroup,
    write: Option<wgpu::BindGroup>,
    tiles: [u32; 2],
}

pub(super) struct Beam {
    pipeline: Option<wgpu::ComputePipeline>,
    target: RefCell<BeamTarget>,
    layout: wgpu::BindGroupLayout,
}

impl Beam {
    pub(super) fn new(device: &wgpu::Device, options: Options) -> Self {
        let layout = beam_layout(device);
        let empty = uint_texture(device, "inactive NAADF beam prefix", [1; 2], false);
        let sample = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("inactive NAADF beam prefix"),
            layout: &layout,
            entries: &[entry(0, &empty)],
        });
        Self {
            pipeline: options.beam.then(|| {
                compute(
                    device,
                    "NAADF screen beam empty prefixes",
                    BEAM_SHADER,
                    Some(options.tile()),
                    Some(options.diagnostics),
                )
            }),
            target: RefCell::new(BeamTarget {
                sample,
                write: None,
                tiles: [0; 2],
            }),
            layout,
        }
    }

    pub(super) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    pub(super) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        camera: &wgpu::Buffer,
        heights: &wgpu::TextureView,
        metrics: &wgpu::Buffer,
        viewport: [u32; 2],
        stages: Option<&std::sync::Arc<super::gpu_stages::Profiler>>,
    ) {
        let Some(pipeline) = &self.pipeline else {
            return;
        };
        let tiles = viewport.map(|n| n.div_ceil(8).max(1));
        let mut target = self.target.borrow_mut();
        if target.tiles != tiles {
            let view = uint_texture(device, "NAADF screen beam empty prefix", tiles, true);
            let sample = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("NAADF screen beam sampling"),
                layout: &self.layout,
                entries: &[entry(0, &view)],
            });
            let write = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("NAADF screen beam output"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    entry(1, heights),
                    entry(2, &view),
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: metrics.as_entire_binding(),
                    },
                ],
            });
            *target = BeamTarget {
                sample,
                write: Some(write),
                tiles,
            };
        }
        let timing = stages.and_then(|p| p.begin(super::gpu_stages::Stage::Beam));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("NAADF beam prepass"),
            timestamp_writes: timing.as_ref().map(|t| t.writes(true, true)),
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, target.write.as_ref().unwrap(), &[]);
        pass.dispatch_workgroups(tiles[0].div_ceil(8), tiles[1].div_ceil(8), 1);
        drop(pass);
        if let Some(t) = timing {
            t.resolve(encoder);
            t.defer();
        }
    }

    pub(super) fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(2, &self.target.borrow().sample, &[]);
    }

    pub(super) fn bytes(&self) -> usize {
        self.target.borrow().tiles.into_iter().product::<u32>() as usize * 4
    }
}

#[test]
fn shader_variants_validate() {
    for source in [HEIGHT_SHADER, HEIGHT_REDUCE, BEAM_SHADER] {
        let module = naga::front::wgsl::parse_str(source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
