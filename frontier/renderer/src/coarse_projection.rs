//! Alternative color passes for published, seed-sampled coarse columns.
use super::Allocation;
use std::{collections::HashMap, ops::Range, sync::OnceLock};

pub(super) const COLUMN_BYTES: u64 = 52;
pub(super) const SEAM_BYTES: u64 = 32;
const REGION_BYTES: u64 = 24;

pub(super) fn visibility_probe_requested() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("FRONTIER_COARSE_VISIBILITY_PROBE").is_some_and(|v| v == "1")
    })
}

#[derive(Clone, Copy)]
pub(super) enum Mode {
    Quads,
    Boxes,
}

impl Mode {
    pub(super) fn requested() -> Option<Self> {
        match std::env::var("FRONTIER_COARSE_PROJECTION").as_deref() {
            Ok("quads") => Some(Self::Quads),
            Ok("boxes") => Some(Self::Boxes),
            _ => None,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Quads => "quads",
            Self::Boxes => "boxes",
        }
    }

    fn vertices_per_column(self) -> u32 {
        match self {
            Self::Quads => 54,
            Self::Boxes => 4,
        }
    }

    pub(super) fn triangles_per_column(self) -> u64 {
        match self {
            Self::Quads => 18,
            Self::Boxes => 2,
        }
    }
}

pub(super) struct Resident {
    pub allocation: Allocation,
    pub columns: u32,
    pub seams: u32,
    pub regions: u32,
}

impl Resident {
    pub(super) fn bytes(&self) -> u64 {
        self.allocation.range.end - self.allocation.range.start
    }

    pub(super) fn column_range(&self) -> Range<u64> {
        let start = self.allocation.range.start;
        start..start + u64::from(self.columns) * COLUMN_BYTES
    }

    pub(super) fn seam_range(&self) -> Range<u64> {
        let start = self.column_range().end;
        start..start + u64::from(self.seams) * SEAM_BYTES
    }

    pub(super) fn region_range(&self) -> Range<u64> {
        let start = self.seam_range().end;
        start..start + u64::from(self.regions) * REGION_BYTES
    }
}

pub(super) struct Pass {
    pub mode: Mode,
    pub normal: wgpu::RenderPipeline,
    pub far: Option<wgpu::RenderPipeline>,
}

impl Pass {
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        mode: Mode,
        constants: &HashMap<String, f64>,
        far_constants: Option<&HashMap<String, f64>>,
    ) -> Self {
        let (vertex, fragment, topology, cull_mode) = match mode {
            Mode::Quads => (
                "vs_coarse_generated",
                "fs_main",
                wgpu::PrimitiveTopology::TriangleList,
                Some(wgpu::Face::Back),
            ),
            Mode::Boxes => (
                "vs_coarse_splat",
                "fs_coarse_splat",
                wgpu::PrimitiveTopology::TriangleStrip,
                None,
            ),
        };
        let attributes =
            wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x2,2=>Float32x4,3=>Uint32x3];
        let pipeline = |constants| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("coarse column projection"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vertex),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        ..Default::default()
                    },
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: COLUMN_BYTES,
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
        Self {
            mode,
            normal: pipeline(constants),
            far: far_constants.map(pipeline),
        }
    }

    pub(super) fn vertices(&self) -> Range<u32> {
        0..self.mode.vertices_per_column()
    }
}
