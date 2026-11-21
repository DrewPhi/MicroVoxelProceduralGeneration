//! Bounded immutable cluster residency. Geometry and ownership are CPU contracts;
//! this path never adds a normal-derived coverage approximation or screen blur.
const ADAPTIVE_SHADER: &str = concat!(
    "const NAADF_GROUP:u32=2u;const NAADF_BASE:u32=3u;\n",
    include_str!("naadf/trace.wgsl"),
    include_str!("sunlight.wgsl"),
    include_str!("ground_light.wgsl"),
    include_str!("water.wgsl"),
    include_str!("edited_columns.wgsl"),
    include_str!("material_palette.wgsl"),
    include_str!("adaptive.wgsl")
);

use crate::{
    adaptive_geometry::{GeometryTile, NativeBin, TileKey},
    gpu_ordered_profiles::GpuOrderedProfiles,
    mesh, Camera,
};
use frontier_runtime::occlusion::{Aabb, OcclusionSet};
use glam::{DVec3, Mat4, Vec3};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    time::{Duration, Instant},
};
use wgpu::util::DeviceExt;
#[path = "adaptive_shadow.rs"]
pub(crate) mod adaptive_shadow;
#[path = "coarse_batch.rs"]
pub(crate) mod coarse_batch;
#[path = "edit_landscape.rs"]
pub(crate) mod edit_landscape;
#[path = "face_regions.rs"]
mod face_regions;
#[path = "landscape_detail.rs"]
pub(crate) mod landscape_detail;
#[path = "proxy_indexed.rs"]
mod proxy_indexed;
use adaptive_shadow::ShadowResources;

const PAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const WARM_CACHE_MAX_BYTES: u64 = 96 * 1024 * 1024;
const WARM_CACHE_MAX_CLUSTERS: usize = 1024;
const WARM_CACHE_TTL: Duration = Duration::from_secs(12);
pub(crate) const MAX_CLUSTERS: usize = 16384;
const UNIFORM_STRIDE: u64 = 256;
pub(crate) const CAMERA_UNIFORM_BYTES: u64 = UNIFORM_STRIDE * MAX_CLUSTERS as u64;
const CLUSTER_CAPACITY_ERROR: &str = "adaptive cluster count budget exhausted";
const POOL_CAPACITY_ERROR: &str = "adaptive GPU pool budget exhausted";
/// Only resource refusal may preserve the old coherent cover. Invalid geometry
/// and other upload failures must remain visible to the caller.
pub(crate) fn resource_capacity_error(error: &str) -> bool {
    matches!(error, CLUSTER_CAPACITY_ERROR | POOL_CAPACITY_ERROR)
}

/// A coalescing byte allocator. Released clusters reuse the existing GPU pages.
struct Arena {
    buffer: wgpu::Buffer,
    free: Vec<Range<u64>>,
    size: u64,
}
impl Arena {
    fn new(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("adaptive cluster pool"),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            free: vec![0..size],
            size,
        }
    }
    fn release(&mut self, range: Range<u64>) {
        release_range(&mut self.free, range)
    }
}
#[cfg(test)]
fn allocate_range(free: &mut Vec<Range<u64>>, bytes: u64) -> Option<Range<u64>> {
    allocate_aligned_range(free, bytes, 1)
}
fn allocate_aligned_range(
    free: &mut Vec<Range<u64>>,
    bytes: u64,
    alignment: u64,
) -> Option<Range<u64>> {
    let (i, start) = free.iter().enumerate().find_map(|(i, range)| {
        let start = range.start.checked_add(alignment - 1)? / alignment * alignment;
        (start.checked_add(bytes)? <= range.end).then_some((i, start))
    })?;
    let old = free.remove(i);
    let range = start..start + bytes;
    if old.start < start {
        free.insert(i, old.start..start);
    }
    let next = i + usize::from(old.start < start);
    if range.end < old.end {
        free.insert(next, range.end..old.end);
    }
    Some(range)
}
fn release_range(free: &mut Vec<Range<u64>>, range: Range<u64>) {
    free.push(range);
    free.sort_by_key(|r| r.start);
    let mut i = 0;
    while i + 1 < free.len() {
        if free[i].end == free[i + 1].start {
            let end = free.remove(i + 1).end;
            free[i].end = end;
        } else {
            i += 1;
        }
    }
}

struct Allocation {
    page: usize,
    range: Range<u64>,
}
/// Reserve padding as part of the allocation so release and pool accounting
/// include it. All vertex allocations share this alignment in the experiment.
fn vertex_reservation_bytes(bytes: u64, enabled: bool) -> Option<u64> {
    if enabled {
        bytes.checked_add(63).map(|n| n & !63)
    } else {
        Some(bytes)
    }
}
/// An entire-page instance binding addresses the same records as an allocation
/// slice. Generic per-vertex meshes deliberately retain their slice binding.
#[cfg(test)]
fn packed_first_instance(
    enabled: bool,
    native: bool,
    proxy: bool,
    allocation: &Range<u64>,
    page_bytes: u64,
    count: u32,
) -> Option<u32> {
    packed_first_instance_stride(
        enabled,
        native,
        proxy,
        allocation,
        page_bytes,
        count,
        if native { 32 } else { 64 },
    )
}
fn packed_first_instance_stride(
    enabled: bool,
    native: bool,
    proxy: bool,
    allocation: &Range<u64>,
    page_bytes: u64,
    count: u32,
    stride: u64,
) -> Option<u32> {
    if !enabled || native == proxy {
        return None;
    }
    if allocation.start % stride != 0
        || allocation.end > page_bytes
        || allocation.end < allocation.start
        || u64::from(count) * stride > allocation.end - allocation.start
    {
        return None;
    }
    let first = u32::try_from(allocation.start / stride).ok()?;
    first.checked_add(count)?;
    Some(first)
}
struct ProfileAllocation {
    indices: Option<Allocation>,
    table: Allocation,
}
struct Cluster {
    appearance_pending: bool,
    cover: bool,
    native_quads: bool,
    proxy_triangles: bool,
    indexed_proxy: bool,
    derived_proxy: bool,
    position_bytes: u64,
    position_reserved_bytes: u64,
    bins: Vec<NativeBin>,
    face_bounds: Vec<[Vec3; 2]>,
    exact: bool,
    profile: Option<ProfileAllocation>,
    vertices: Allocation,
    indices: Option<Allocation>,
    count: u32,
    origin: DVec3,
    bounds: [Vec3; 2],
    canonical_bounds: Aabb,
    guaranteed_solid: Option<Aabb>,
    proxy_bounds: [[f64; 3]; 2],
    best_error_m: Option<f64>,
    water_feature_error_m: f64,
    certificate_kind: u8,
    slot: u32,
}
impl Cluster {
    fn payload_bytes(&self) -> u64 {
        self.vertices.range.end - self.vertices.range.start
            + self
                .indices
                .as_ref()
                .map_or(0, |i| i.range.end - i.range.start)
            + self.profile.as_ref().map_or(0, |p| {
                p.table.range.end - p.table.range.start
                    + p.indices
                        .as_ref()
                        .map_or(0, |i| i.range.end - i.range.start)
            })
    }
}
/// Bounded current-draw diagnostic; None projected error means unbounded.
#[derive(Clone, Copy, Debug)]
pub struct VisibleOffender {
    pub key: TileKey,
    pub canonical_bounds: [[f64; 3]; 2],
    pub proxy_bounds: [[f64; 3]; 2],
    pub cached_vertical_error_m: Option<f64>,
    pub water_feature_error_m: f64,
    pub projected_error_px: Option<f64>,
}
#[derive(Default, Clone, Copy, Debug)]
pub struct GpuProfileStats {
    pub enabled: bool,
    pub source_cache_hits: u64,
    pub source_uploads: u64,
    pub source_upload_bytes: u64,
    pub job_upload_bytes: u64,
    pub result_copy_bytes: u64,
    pub cpu_profile_upload_bytes_avoided: u64,
    pub dispatches: u64,
    pub profiles: u64,
    pub submits: u64,
    /// CPU upload/encode/submit duration; never reported as GPU duration.
    pub enqueue_ms: f64,
    pub initialization_ms: f64,
    pub resident_gpu_bytes: u64,
    pub source_cache_cpu_bytes: u64,
}
#[derive(Default, Clone, Copy, Debug)]
pub struct DrawStats {
    pub warm_clusters: usize,
    pub warm_bytes: u64,
    pub warm_reuses: u64,
    pub warm_evictions: u64,
    pub gpu_profiles: GpuProfileStats,
    pub prepare_shadow_ms: f64,
    pub prepare_order_ms: f64,
    pub prepare_resident_ms: f64,
    pub prepare_upload_ms: f64,
    pub prepare_bundle_ms: f64,
    pub prepare_order_reused: bool,
    pub prepare_occlusion_ms: f64,
    pub prepare_visibility_uniform_ms: f64,
    pub prepare_reuse_enabled: bool,
    pub prepared_view_hits: u64,
    pub prepared_view_misses: u64,
    pub bundle_enabled: bool,
    pub bundle_active: bool,
    pub bundle_builds: u64,
    pub bundle_reuses: u64,
    pub bundle_build_ms: f64,
    pub vertex_pages_coalescing_enabled: bool,
    pub vertex_page_binds_avoided: u64,
    pub vertex_page_binds_eligible: u64,
    pub resident_clusters: usize,
    pub cluster_capacity: usize,
    pub available_cluster_slots: usize,
    pub visible_clusters: usize,
    pub triangles: u64,
    pub resident_bytes: u64,
    pub allocated_bytes: u64,
    pub resident_native_quad_bytes: u64,
    pub resident_proxy_triangle_bytes: u64,
    pub resident_profile_bytes: u64,
    pub indexed_proxy_enabled: bool,
    pub indexed_proxy_mode: u32,
    pub resident_derived_proxy_clusters: u64,
    pub resident_indexed_proxy_clusters: u64,
    pub resident_proxy_position_bytes: u64,
    pub resident_proxy_position_reserved_bytes: u64,
    pub resident_indexed_proxy_record_padding_bytes: u64,
    pub indexed_proxy_pack_ms: f64,
    pub indexed_proxy_pack_attempts: u64,
    pub indexed_proxy_pack_accepted: u64,
    pub indexed_proxy_peak_staging_estimate_bytes: u64,
    pub visible_native_quads: u64,
    pub occluded_clusters: usize,
    pub clipped_draw_clusters: usize,
    pub occluded_triangles: u64,
    pub occluder_boxes: usize,
    pub current_max_error_px: Option<f64>,
    pub current_max_finite_error_px: Option<f64>,
    pub current_unresolved_clusters: usize,
    pub current_uncertified_clusters: usize,
    pub current_quality_ready: bool,
    pub native_bin_tests: u64,
    pub native_bin_skipped_quads: u64,
    pub clipped_region_quads: u64,
    pub draw_calls: u64,
    pub no_voxel_shadow_clusters: usize,
    pub coarse_batch_groups: usize,
    pub coarse_batch_candidates: usize,
    pub current_local_certified_clusters: usize,
    pub current_affine_bound_clusters: usize,
    pub current_global_bound_clusters: usize,
    pub current_offenders: [Option<VisibleOffender>; 8],
}
fn record_offender(top: &mut [Option<VisibleOffender>; 8], offender: VisibleOffender) {
    let priority = offender.projected_error_px.unwrap_or(f64::INFINITY);
    if let Some(index) = top.iter().position(|old| {
        old.map_or(true, |old| {
            let previous = old.projected_error_px.unwrap_or(f64::INFINITY);
            priority > previous || (priority == previous && offender.key < old.key)
        })
    }) {
        for slot in (index + 1..top.len()).rev() {
            top[slot] = top[slot - 1];
        }
        top[index] = Some(offender);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrawItem {
    no_voxel_shadow: bool,
    key: TileKey,
    bin_mask: Option<u32>,
}
// Keep direct and cached command encoding identical. Bundles cache commands,
// never visibility or camera values; each prepare still evaluates current demand.
macro_rules! encode_cluster_draws {
    ($renderer:expr, $pass:expr) => {{
        let renderer = $renderer;
        let pass = $pass;
        encode_cluster_draws!(renderer, &mut *pass, false);
        if renderer.cover_pipelines.is_some() {
            encode_cluster_draws!(renderer, &mut *pass, true);
        }
    }};
    ($renderer:expr, $pass:expr, $shell:expr) => {{
        let renderer = $renderer;
        let pass = $pass;
        let mut bound_vertex_page = None;
        for (draw_index, item) in renderer.visible.iter().enumerate() {
            let far = if $shell {
                renderer.cover_pipelines.as_ref()
            } else if item.no_voxel_shadow {
                renderer.far_pipelines.as_ref()
            } else {
                None
            };
            let c = &renderer.clusters[&item.key];
            if renderer.coarse_batch.is_some() && coarse_batch::Batch::accepts(c) {
                continue;
            }
            if $shell && (c.exact || !c.cover) {
                continue;
            }
            if c.native_quads {
                pass.set_pipeline(far.map_or(&renderer.native_pipeline, |p| &p[2]));
            } else if c.proxy_triangles {
                pass.set_pipeline(if c.derived_proxy {
                    far.map_or(&renderer.derived_proxy_pipeline, |p| &p[5])
                } else if c.indexed_proxy {
                    far.map_or(&renderer.indexed_proxy_pipeline, |p| &p[4])
                } else {
                    far.map_or(&renderer.proxy_pipeline, |p| &p[3])
                });
            } else if let Some(p) = &c.profile {
                pass.set_pipeline(far.map_or(&renderer.profile_pipeline, |p| &p[1]));
                let indices = p.indices.as_ref().unwrap();
                pass.set_vertex_buffer(
                    1,
                    renderer.profiles[indices.page]
                        .buffer
                        .slice(indices.range.clone()),
                );
            } else {
                pass.set_pipeline(far.map_or(&renderer.pipeline, |p| &p[0]));
                pass.set_vertex_buffer(1, renderer.zero_profile.slice(..));
            }
            pass.set_bind_group(
                0,
                c.profile
                    .as_ref()
                    .map_or(&renderer.bind, |p| &renderer.profile_binds[p.table.page]),
                &[(draw_index as u64 * UNIFORM_STRIDE) as u32],
            );
            let page = &renderer.vertices[c.vertices.page];
            let packed_first = packed_first_instance_stride(
                renderer.coalesce_vertex_pages,
                c.native_quads,
                c.proxy_triangles,
                &c.vertices.range,
                page.size,
                c.count,
                if c.derived_proxy {
                    20
                } else if c.indexed_proxy {
                    40
                } else if c.native_quads {
                    32
                } else {
                    64
                },
            );
            let first_instance = if let Some(first) = packed_first {
                if bound_vertex_page != Some(c.vertices.page) {
                    pass.set_vertex_buffer(0, page.buffer.slice(..));
                    bound_vertex_page = Some(c.vertices.page);
                }
                first
            } else {
                pass.set_vertex_buffer(0, page.buffer.slice(c.vertices.range.clone()));
                bound_vertex_page = None;
                0
            };
            if c.native_quads {
                pass.set_index_buffer(renderer.quad_indices.slice(..), wgpu::IndexFormat::Uint32);
                if let Some(mask) = item.bin_mask {
                    for span in face_regions::visible_runs(&c.bins, mask) {
                        pass.draw_indexed(
                            0..6,
                            0,
                            first_instance + span.start..first_instance + span.end,
                        );
                    }
                } else {
                    pass.draw_indexed(0..6, 0, first_instance..first_instance + c.count);
                }
            } else if c.proxy_triangles {
                pass.draw(0..3, first_instance..first_instance + c.count);
            } else {
                let indices = c.indices.as_ref().unwrap();
                pass.set_index_buffer(
                    renderer.indices[indices.page]
                        .buffer
                        .slice(indices.range.clone()),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..c.count, 0, 0..1);
            }
        }
    }};
}

/// Same local f32 transform as `look_to_rh`, with the view basis prepared once.
/// Keep the eye subtraction and dot products local to each cluster: rebasing or
/// factoring the translation through a global f32 position loses precision.
struct LocalViewTransform {
    forward: Vec3,
    right: Vec3,
    up: Vec3,
}
impl LocalViewTransform {
    fn new(direction: Vec3, view_up: Vec3) -> Self {
        let forward = direction.normalize();
        let right = forward.cross(view_up).normalize();
        Self {
            forward,
            right,
            up: right.cross(forward),
        }
    }
    fn matrix(&self, eye: Vec3) -> Mat4 {
        let [r, u, f] = [self.right, self.up, self.forward];
        Mat4::from_cols(
            glam::Vec4::new(r.x, u.x, -f.x, 0.),
            glam::Vec4::new(r.y, u.y, -f.y, 0.),
            glam::Vec4::new(r.z, u.z, -f.z, 0.),
            glam::Vec4::new(-eye.dot(r), -eye.dot(u), eye.dot(f), 1.),
        )
    }
}

fn sort_draw_order(order: &mut [(f64, TileKey)]) {
    static IN_PLACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let in_place = *IN_PLACE
        .get_or_init(|| std::env::var("FRONTIER_TERRAIN_IN_PLACE_ORDER").as_deref() == Ok("1"));
    sort_draw_order_impl(order, in_place);
}

fn sort_draw_order_impl(order: &mut [(f64, TileKey)], in_place: bool) {
    // A TileKey occurs once in a resident draw domain. Depth plus key therefore
    // defines a total order: unstable sorting preserves the same visible order
    // without the stable sort's temporary allocation. Keep the control for A/B.
    let compare = |(da, ka): &(f64, TileKey), (db, kb): &(f64, TileKey)| {
        da.total_cmp(db).then_with(|| ka.cmp(kb))
    };
    if in_place {
        order.sort_unstable_by(compare);
    } else {
        order.sort_by(compare);
    }
}

// A previous sorted key sequence is only a starting order. All depth values
// are recomputed for the current camera before the same exact total sort.
struct DrawOrderDomain {
    residency_revision: u64,
    owners: HashSet<TileKey>,
}
impl DrawOrderDomain {
    fn matches(&self, residency_revision: u64, owners: &HashSet<TileKey>) -> bool {
        self.residency_revision == residency_revision && self.owners == *owners
    }
}

struct PreparedView {
    camera: Camera,
    width: u32,
    height: u32,
    pixel_error: f64,
    owners: HashSet<TileKey>,
    stats: DrawStats,
}
impl PreparedView {
    fn matches(
        &self,
        camera: &Camera,
        width: u32,
        height: u32,
        pixel_error: f64,
        owners: &HashSet<TileKey>,
    ) -> bool {
        self.camera == *camera
            && self.width == width
            && self.height == height
            && self.pixel_error == pixel_error
            && self.owners == *owners
    }
}
pub struct AdaptiveGpu {
    edits: edit_landscape::Surface,
    landscape_detail: landscape_detail::Detail,
    resource_budget_bytes: u64,
    gpu_profiles: Option<GpuOrderedProfiles>,
    gpu_profile_stats: GpuProfileStats,
    lighting: [f32; 12],
    near_detail: bool,
    visible_vertical_bound: bool,
    near_beauty: bool,
    shadows: ShadowResources,
    prepared_view: Option<PreparedView>,
    resident_stats: Option<DrawStats>,
    draw_order: Vec<(f64, TileKey)>,
    draw_order_domain: Option<DrawOrderDomain>,
    incremental_order_enabled: bool,
    prepare_reuse_enabled: bool,
    pub(super) residency_revision: u64,
    prepared_view_hits: u64,
    prepared_view_misses: u64,
    prepared_view_enabled: bool,
    bundles_enabled: bool,
    coalesce_vertex_pages: bool,
    format: wgpu::TextureFormat,
    sample_count: u32,
    bundle: Option<wgpu::RenderBundle>,
    coarse_batch: Option<coarse_batch::Batch>,
    bundle_signature: Vec<DrawItem>,
    bundle_stable_frames: u8,
    bundle_builds: u64,
    bundle_reuses: u64,
    bundle_build_ms: f64,
    native_bins_enabled: bool,
    face_regions_enabled: bool,
    voxel_filter: bool,
    far_pipelines: Option<[wgpu::RenderPipeline; 6]>,
    cover_pipelines: Option<[wgpu::RenderPipeline; 6]>,
    naadf_shadow_bounds: Option<[[f64; 3]; 2]>,
    pipeline: wgpu::RenderPipeline,
    profile_pipeline: wgpu::RenderPipeline,
    native_pipeline: wgpu::RenderPipeline,
    proxy_pipeline: wgpu::RenderPipeline,
    indexed_proxy_pipeline: wgpu::RenderPipeline,
    derived_proxy_pipeline: wgpu::RenderPipeline,
    indexed_proxy_mode: u32,
    indexed_proxy_enabled: bool,
    indexed_proxy_pack_ms: f64,
    indexed_proxy_pack_attempts: u64,
    indexed_proxy_pack_accepted: u64,
    indexed_proxy_peak_staging_estimate_bytes: u64,
    quad_indices: wgpu::Buffer,
    zero_profile: wgpu::Buffer,
    profiles: Vec<Arena>,
    profile_tables: Vec<Arena>,
    profile_binds: Vec<wgpu::BindGroup>,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    vertices: Vec<Arena>,
    indices: Vec<Arena>,
    clusters: HashMap<TileKey, Cluster>,
    warm_enabled: bool,
    warm: HashMap<TileKey, (Instant, u64)>,
    warm_bytes: u64,
    warm_reuses: u64,
    warm_evictions: u64,
    slots: Vec<u32>,
    visible: Vec<DrawItem>,
    camera_upload: Vec<u8>,
}
impl AdaptiveGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        assert!(
            CAMERA_UNIFORM_BYTES <= device.limits().max_buffer_size,
            "adaptive camera uniforms exceed device buffer limit"
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("adaptive cluster camera"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(208),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(80),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(4),
                    },
                    count: None,
                },
            ],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("adaptive pooled camera uniforms"),
            size: CAMERA_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let zero_table = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("zero canonical profile table"),
            contents: &[0u8; 80],
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bind = profile_bind(device, &layout, &uniform, &zero_table);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("adaptive canonical shading"),
            source: wgpu::ShaderSource::Wgsl(crate::voxel_shader(ADAPTIVE_SHADER)),
        });
        let shadows_layout = adaptive_shadow::sampling_layout(device);
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Alpine shadow caster pipeline"),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
        let mask_layout = adaptive_shadow::caster_mask_layout(device);
        let masked_shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain shadow with excavated columns"),
            bind_group_layouts: &[&layout, &mask_layout],
            push_constant_ranges: &[],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("adaptive cluster pipeline"),
            bind_group_layouts: &[
                &layout,
                &shadows_layout,
                &adaptive_shadow::near_sampling_layout(device),
            ],
            push_constant_ranges: &[],
        });
        let batch_layout = coarse_batch::supported(device).then(|| coarse_batch::layout(device));
        let batch_pipeline_layout = batch_layout.as_ref().map(|layout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("coarse voxel batch pipeline"),
                bind_group_layouts: &[
                    layout,
                    &shadows_layout,
                    &adaptive_shadow::near_sampling_layout(device),
                ],
                push_constant_ranges: &[],
            })
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3];
        let profile_attributes = wgpu::vertex_attr_array![3=>Uint32];
        let native_attributes =
            wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Uint32,3=>Uint32,4=>Uint32];
        let proxy_attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3,3=>Float32x3,4=>Float32x3,5=>Uint32];
        let indexed_proxy_attributes =
            wgpu::vertex_attr_array![0=>Uint32x3,1=>Float32x3,2=>Float32x3,3=>Uint32];
        let derived_proxy_attributes = wgpu::vertex_attr_array![0=>Uint32x3,1=>Uint32,2=>Uint32];
        let mut naadf_constants = crate::naadf::mesh_traversal_constants();
        naadf_constants.insert(
            "cover_directional".into(),
            if crate::ground_cover::directional_enabled() {
                1.
            } else {
                0.
            },
        );
        let mut far_constants = naadf_constants.clone();
        far_constants.insert("traversal_mesh_sun".into(), 0.);
        let mut shell_constants = naadf_constants.clone();
        shell_constants.insert("cover_shell".into(), 1.);
        type Caster<'a> = Option<(&'a wgpu::PipelineLayout, bool)>;
        let create_pipeline = |step_mode, kind, caster: Caster<'_>, far, shell| {
            let shadow = caster.is_some();
            let constants = if shell {
                &shell_constants
            } else if far {
                &far_constants
            } else {
                &naadf_constants
            };
            let batched = kind == 5;
            let native = kind == 1 || batched;
            let proxy = kind == 2;
            let indexed_proxy = kind == 3;
            let derived_proxy = kind == 4;
            let derived_proxy_buffers = [wgpu::VertexBufferLayout {
                array_stride: 20,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &derived_proxy_attributes,
            }];
            let indexed_proxy_buffers = [wgpu::VertexBufferLayout {
                array_stride: 40,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &indexed_proxy_attributes,
            }];
            let proxy_buffers = [wgpu::VertexBufferLayout {
                array_stride: 64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &proxy_attributes,
            }];
            let native_buffers = [wgpu::VertexBufferLayout {
                array_stride: 32,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &native_attributes,
            }];
            let mesh_buffers = [
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<mesh::Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                },
                wgpu::VertexBufferLayout {
                    array_stride: 4,
                    step_mode,
                    attributes: &profile_attributes,
                },
            ];

            let color_targets = [Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("adaptive clusters"),
                layout: Some(if let Some((caster_layout, _)) = caster {
                    caster_layout
                } else if batched {
                    batch_pipeline_layout.as_ref().unwrap()
                } else {
                    &pipeline_layout
                }),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(if batched {
                        "vs_native_batch"
                    } else if native {
                        "vs_native"
                    } else if derived_proxy {
                        "vs_proxy_derived"
                    } else if indexed_proxy {
                        "vs_proxy_indexed"
                    } else if proxy {
                        "vs_proxy"
                    } else {
                        "vs_main"
                    }),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        ..Default::default()
                    },
                    buffers: if derived_proxy {
                        &derived_proxy_buffers
                    } else if indexed_proxy {
                        &indexed_proxy_buffers
                    } else if proxy {
                        &proxy_buffers
                    } else if native {
                        &native_buffers
                    } else {
                        &mesh_buffers
                    },
                },
                fragment: if shadow {
                    caster
                        .filter(|(_, masked)| *masked)
                        .map(|_| wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some("fs_shadow_cut"),
                            compilation_options: Default::default(),
                            targets: &[],
                        })
                } else {
                    Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(if batched {
                            "fs_native_batch"
                        } else {
                            "fs_main"
                        }),
                        compilation_options: wgpu::PipelineCompilationOptions {
                            constants,
                            ..Default::default()
                        },
                        targets: &color_targets,
                    })
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: if shadow {
                        wgpu::CompareFunction::Less
                    } else {
                        wgpu::CompareFunction::Greater
                    },
                    stencil: Default::default(),
                    bias: if shadow {
                        wgpu::DepthBiasState {
                            constant: 2,
                            slope_scale: 2.,
                            clamp: 0.,
                        }
                    } else {
                        Default::default()
                    },
                }),
                multisample: wgpu::MultisampleState {
                    count: if shadow { 1 } else { sample_count },
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
        };
        let make_pipeline = |mode, kind, far, shell| create_pipeline(mode, kind, None, far, shell);
        let pipeline = make_pipeline(wgpu::VertexStepMode::Instance, 0, false, false);
        let profile_pipeline = make_pipeline(wgpu::VertexStepMode::Vertex, 0, false, false);
        let native_pipeline = make_pipeline(wgpu::VertexStepMode::Instance, 1, false, false);
        let proxy_pipeline = make_pipeline(wgpu::VertexStepMode::Instance, 2, false, false);
        let indexed_proxy_pipeline = make_pipeline(wgpu::VertexStepMode::Instance, 3, false, false);
        let derived_proxy_pipeline = make_pipeline(wgpu::VertexStepMode::Instance, 4, false, false);
        let far_pipelines = (std::env::var("FRONTIER_NAADF_SPLIT_SHADERS").as_deref() == Ok("1"))
            .then(|| {
                [
                    make_pipeline(wgpu::VertexStepMode::Instance, 0, true, false),
                    make_pipeline(wgpu::VertexStepMode::Vertex, 0, true, false),
                    make_pipeline(wgpu::VertexStepMode::Instance, 1, true, false),
                    make_pipeline(wgpu::VertexStepMode::Instance, 2, true, false),
                    make_pipeline(wgpu::VertexStepMode::Instance, 3, true, false),
                    make_pipeline(wgpu::VertexStepMode::Instance, 4, true, false),
                ]
            });
        let cover_pipelines = crate::ground_cover::shell_enabled().then(|| {
            [
                make_pipeline(wgpu::VertexStepMode::Instance, 0, false, true),
                make_pipeline(wgpu::VertexStepMode::Vertex, 0, false, true),
                make_pipeline(wgpu::VertexStepMode::Instance, 1, false, true),
                make_pipeline(wgpu::VertexStepMode::Instance, 2, false, true),
                make_pipeline(wgpu::VertexStepMode::Instance, 3, false, true),
                make_pipeline(wgpu::VertexStepMode::Instance, 4, false, true),
            ]
        });
        let caster_pipelines = |layout: &wgpu::PipelineLayout, masked| {
            [
                (wgpu::VertexStepMode::Instance, 0),
                (wgpu::VertexStepMode::Vertex, 0),
                (wgpu::VertexStepMode::Instance, 1),
                (wgpu::VertexStepMode::Instance, 2),
                (wgpu::VertexStepMode::Instance, 3),
                (wgpu::VertexStepMode::Instance, 4),
            ]
            .map(|(mode, kind)| create_pipeline(mode, kind, Some((layout, masked)), false, false))
        };
        let shadow_pipelines = caster_pipelines(&shadow_pipeline_layout, false);
        let masked_pipelines = caster_pipelines(&masked_shadow_layout, true);
        let batch_pipeline = batch_pipeline_layout
            .as_ref()
            .map(|_| make_pipeline(wgpu::VertexStepMode::Instance, 5, false, false));
        let coarse_batch = batch_layout
            .zip(batch_pipeline)
            .map(|(layout, raster)| coarse_batch::Batch::new(device, layout, raster));
        let shadows = ShadowResources::new(
            device,
            &shadows_layout,
            shadow_pipelines,
            masked_pipelines,
            mask_layout,
        );
        let indexed_proxy_mode = match std::env::var("FRONTIER_INDEXED_PROXY").as_deref() {
            Ok("1") => 1,
            Ok("2") => 2,
            _ => 0,
        };
        let quad_indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shared exact native quad topology"),
            contents: bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]),
            usage: wgpu::BufferUsages::INDEX,
        });
        let zero_profile = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("exact native zero appearance profile"),
            contents: &[0u8; 4],
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            resource_budget_bytes: MAX_BYTES,
            edits: edit_landscape::Surface::default(),
            landscape_detail: landscape_detail::Detail::default(),
            gpu_profiles: None,
            gpu_profile_stats: GpuProfileStats {
                enabled: std::env::var("FRONTIER_GPU_PROFILES").as_deref() == Ok("1"),
                ..Default::default()
            },
            lighting: [0.; 12],
            near_beauty: false,
            near_detail: false,
            visible_vertical_bound: crate::adaptive_stream::visible_vertical_bound_for_alpine(
                false,
            ),
            shadows,
            prepared_view: None,
            resident_stats: None,
            draw_order: vec![],
            draw_order_domain: None,
            incremental_order_enabled: std::env::var("FRONTIER_TERRAIN_INCREMENTAL_ORDER")
                .as_deref()
                == Ok("1"),
            prepare_reuse_enabled: std::env::var("FRONTIER_TERRAIN_PREPARE_REUSE").as_deref()
                != Ok("0"),
            residency_revision: 0,
            prepared_view_hits: 0,
            prepared_view_misses: 0,
            prepared_view_enabled: std::env::var_os("FRONTIER_PREPARED_VIEW_CACHE")
                .is_none_or(|v| v != "0"),
            bundles_enabled: std::env::var_os("FRONTIER_RENDER_BUNDLES").is_none_or(|v| v != "0"),
            coalesce_vertex_pages: std::env::var_os("FRONTIER_COALESCE_VERTEX_PAGES")
                .is_none_or(|v| v != "0"),
            format,
            sample_count,
            bundle: None,
            coarse_batch,
            bundle_signature: vec![],
            bundle_stable_frames: 0,
            bundle_builds: 0,
            bundle_reuses: 0,
            bundle_build_ms: 0.,
            native_bins_enabled: true,
            face_regions_enabled: std::env::var("FRONTIER_FACE_REGIONS").as_deref() != Ok("0")
                && crate::adaptive_geometry::distant_voxels::Mode::current()
                    == crate::adaptive_geometry::distant_voxels::Mode::Blocks,
            voxel_filter: true,
            far_pipelines,
            cover_pipelines,
            naadf_shadow_bounds: None,
            pipeline,
            profile_pipeline,
            native_pipeline,
            proxy_pipeline,
            indexed_proxy_pipeline,
            derived_proxy_pipeline,
            indexed_proxy_mode,
            indexed_proxy_enabled: indexed_proxy_mode > 0,
            indexed_proxy_pack_ms: 0.,
            indexed_proxy_pack_attempts: 0,
            indexed_proxy_pack_accepted: 0,
            indexed_proxy_peak_staging_estimate_bytes: 0,
            quad_indices,
            zero_profile,
            profiles: vec![],
            profile_tables: vec![],
            profile_binds: vec![],
            layout,
            uniform,
            bind,
            vertices: vec![],
            indices: vec![],
            clusters: HashMap::new(),
            warm_enabled: crate::adaptive_stream::turn_preload_enabled()
                && crate::adaptive_geometry::distant_voxels::enabled(),
            warm: HashMap::new(),
            warm_bytes: 0,
            warm_reuses: 0,
            warm_evictions: 0,
            slots: (0..MAX_CLUSTERS as u32).rev().collect(),
            visible: vec![],
            camera_upload: vec![],
        }
    }
    /// Rebuild only the color pipelines for a supported MSAA sample count.
    /// Existing geometry arenas, cluster owners, profiles and shadow resources
    /// retain their identities. Apply this while settings presentation is paused:
    /// the temporary constructor allocates fresh scratch/shadow resources too.
    pub fn set_sample_count_preserving_geometry(
        &mut self,
        device: &wgpu::Device,
        sample_count: u32,
    ) {
        if self.sample_count == sample_count {
            return;
        }
        let fresh = Self::new(device, self.format, sample_count);
        // Explicit bind-group layouts have identical structural contracts;
        // existing camera/profile/shadow bindings remain compatible.
        self.far_pipelines = fresh.far_pipelines;
        self.cover_pipelines = fresh.cover_pipelines;
        self.pipeline = fresh.pipeline;
        self.profile_pipeline = fresh.profile_pipeline;
        self.native_pipeline = fresh.native_pipeline;
        if let (Some(current), Some(updated)) = (&mut self.coarse_batch, fresh.coarse_batch) {
            current.raster = updated.raster;
        }
        self.proxy_pipeline = fresh.proxy_pipeline;
        self.indexed_proxy_pipeline = fresh.indexed_proxy_pipeline;
        self.derived_proxy_pipeline = fresh.derived_proxy_pipeline;
        self.sample_count = sample_count;
        self.prepared_view = None;
        self.resident_stats = None;
        self.invalidate_bundle();
    }
    pub fn set_visual_lighting(
        &mut self,
        settings: &crate::settings::VisualSettings,
        regional: bool,
        alpine: bool,
        terrain_shadow_appearance: bool,
        amplitude: f32,
    ) {
        // With no plant pass there is no depth-map clear/draw. Do not sample
        // a previous frame's plant shadows after vegetation is switched off.
        let near_beauty = settings.beauty && settings.vegetation;
        let near_detail = crate::adaptive_stream::near_detail_for_alpine(alpine);
        let visible_vertical_bound =
            crate::adaptive_stream::visible_vertical_bound_for_alpine(alpine);
        if self.near_beauty != near_beauty
            || self.near_detail != near_detail
            || self.visible_vertical_bound != visible_vertical_bound
        {
            self.prepared_view = None;
        }
        self.near_beauty = near_beauty;
        self.near_detail = near_detail;
        self.visible_vertical_bound = visible_vertical_bound;
        let sun = crate::atmosphere::sun_direction(settings.sun_elevation, settings.sun_azimuth);
        let value = [
            sun.x,
            sun.y,
            sun.z,
            if regional { 1. } else { 0. },
            settings.overcast,
            if terrain_shadow_appearance && settings.terrain_shadows && sun.y > 0. {
                1.
            } else {
                0.
            },
            crate::appearance_method::debug_view() as f32,
            crate::appearance_method::near_blend() as f32,
            crate::appearance_method::map_anisotropy() as f32,
            amplitude,
            crate::appearance_method::voxel_grain().unwrap_or(-1.),
            if settings.beauty { 1. } else { 0. },
        ];
        if value != self.lighting {
            self.lighting = value;
            self.prepared_view = None;
        }
    }
    /// Headless diagnostics redraw the same immutable published owner set.
    pub(crate) fn set_debug_view(&mut self, mode: u32) {
        self.lighting[6] = mode as f32;
        self.prepared_view = None;
    }
    pub(super) fn shadows_active(&self) -> bool {
        self.lighting[5] > 0.5
    }
    pub fn set_native_bins(&mut self, enabled: bool) {
        if self.native_bins_enabled != enabled {
            self.prepared_view = None;
            self.invalidate_bundle();
        }
        self.native_bins_enabled = enabled;
    }
    pub fn set_voxel_filter(&mut self, enabled: bool) {
        if self.voxel_filter != enabled {
            self.prepared_view = None;
        }
        self.voxel_filter = enabled;
    }
    /// Limit future page/scratch allocations. Existing coherent content remains
    /// resident when lowering the limit; reclamation follows normal ownership.
    pub fn set_resource_budget_mib(&mut self, mib: u64) {
        self.resource_budget_bytes = mib.saturating_mul(1024 * 1024);
    }
    pub fn resource_budget_mib(&self) -> u64 {
        self.resource_budget_bytes / (1024 * 1024)
    }
    pub(crate) fn naadf_residency(
        &self,
        owner: &crate::naadf::ownership::Ownership,
    ) -> (usize, usize) {
        let certified = self
            .clusters
            .keys()
            .filter(|k| !self.warm.contains_key(k) && owner.terrain(**k))
            .count();
        let overlap = self
            .clusters
            .iter()
            .filter(|(key, c)| {
                !self.warm.contains_key(key) && {
                    (0..3).all(|a| {
                        c.proxy_bounds[1][a] >= owner.bounds[0][a]
                            && c.proxy_bounds[0][a] <= owner.bounds[1][a]
                    })
                }
            })
            .count();
        (certified, overlap)
    }
    pub(crate) fn set_naadf_shadow_bounds(&mut self, bounds: Option<[[f64; 3]; 2]>) {
        if bounds != self.naadf_shadow_bounds {
            self.naadf_shadow_bounds = bounds;
            self.prepared_view = None;
            self.invalidate_bundle();
        }
    }
    pub fn contains(&self, key: &TileKey) -> bool {
        self.clusters.contains_key(key)
    }
    fn allocated_bytes(&self) -> u64 {
        (self.shadows.allocated_bytes()
            + CAMERA_UNIFORM_BYTES
            + self.edit_bytes()
            + self.shadows.near.detail.bytes())
            + self
                .coarse_batch
                .as_ref()
                .map_or(0, |_| coarse_batch::Batch::bytes())
            + self
                .gpu_profiles
                .as_ref()
                .map_or(0, |builder| builder.resident_bytes())
            + self
                .vertices
                .iter()
                .chain(self.indices.iter())
                .chain(self.profiles.iter())
                .chain(self.profile_tables.iter())
                .map(|p| p.size)
                .sum::<u64>()
    }
    fn allocate(
        &mut self,
        device: &wgpu::Device,
        bytes: u64,
        kind: u8,
    ) -> Result<Allocation, String> {
        // Indexed records are 40B, so use lcm(40,64)=320B only for this
        // allocation. Prefix gaps remain free for other allocations.
        let alignment = if kind == 4 && self.coalesce_vertex_pages {
            320
        } else {
            1
        };
        let bytes = if alignment == 320 {
            bytes.checked_add(319).map(|n| n / 320 * 320)
        } else {
            vertex_reservation_bytes(bytes, kind == 0 && self.coalesce_vertex_pages)
        }
        .ok_or(POOL_CAPACITY_ERROR)?;
        let total = self.allocated_bytes();
        let pages = match kind {
            0 | 4 => &mut self.vertices,
            1 => &mut self.indices,
            2 => &mut self.profiles,
            _ => &mut self.profile_tables,
        };
        for (page, arena) in pages.iter_mut().enumerate() {
            if let Some(range) = allocate_aligned_range(&mut arena.free, bytes, alignment) {
                return Ok(Allocation { page, range });
            }
        }
        let size = bytes.max(PAGE_BYTES).next_multiple_of(4);
        if total.saturating_add(size) > self.resource_budget_bytes
            || size > device.limits().max_buffer_size
        {
            return Err(POOL_CAPACITY_ERROR.into());
        }
        let mut arena = Arena::new(
            device,
            size,
            match kind {
                1 => wgpu::BufferUsages::INDEX,
                3 => wgpu::BufferUsages::STORAGE,
                _ => wgpu::BufferUsages::VERTEX,
            },
        );
        let range = allocate_aligned_range(&mut arena.free, bytes, alignment).unwrap();
        let page = pages.len();
        pages.push(arena);
        Ok(Allocation { page, range })
    }
    /// Upload immutable geometry once. Calling again with a resident key is free.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tile: &GeometryTile,
    ) -> Result<(), String> {
        if self.contains(&tile.key) {
            return Ok(());
        }
        self.prepared_view = None;
        self.resident_stats = None;
        self.invalidate_bundle();
        if self.slots.is_empty() {
            return Err(CLUSTER_CAPACITY_ERROR.into());
        }
        if tile.gpu_profiles.is_some()
            && (tile.proxy_triangles.is_empty() || !self.gpu_profile_stats.enabled)
        {
            return Err("deferred GPU profiles require enabled proxy consumer".into());
        }
        if !tile.native_quads.is_empty() {
            if !tile.mesh.indices.is_empty() || !tile.proxy_triangles.is_empty() {
                return Err("quad tile must have exclusive primitive storage".into());
            }
            return self.upload_native(device, queue, tile);
        }
        if !tile.proxy_triangles.is_empty() {
            if !tile.mesh.vertices.is_empty()
                || !tile.mesh.indices.is_empty()
                || !tile.face_profile_indices.is_empty()
            {
                return Err("packed proxy must have exclusive triangle ownership".into());
            }
            return self.upload_proxy(device, queue, tile);
        }
        let mesh = &tile.mesh;
        let vb: &[u8] = bytemuck::cast_slice(&mesh.vertices);
        let ib: &[u8] = bytemuck::cast_slice(&mesh.indices);
        if vb.is_empty() || ib.is_empty() {
            return Ok(());
        }
        if !tile.face_profile_indices.is_empty()
            && (tile.face_profile_indices.len() != mesh.vertices.len()
                || tile
                    .face_profile_indices
                    .iter()
                    .any(|&i| i as usize >= tile.face_profiles.len()))
        {
            return Err("adaptive face profile indices do not match vertices/table".into());
        }
        let vertices = self.allocate(device, vb.len() as u64, 0)?;
        let indices = match self.allocate(device, ib.len() as u64, 1) {
            Ok(a) => a,
            Err(e) => {
                self.vertices[vertices.page].release(vertices.range);
                return Err(e);
            }
        };
        let profile = if tile.face_profile_indices.is_empty() {
            None
        } else {
            let table_bytes = bytemuck::cast_slice(&tile.face_profiles);
            let table = match self.allocate(device, table_bytes.len() as u64, 3) {
                Ok(p) => p,
                Err(e) => {
                    self.vertices[vertices.page].release(vertices.range);
                    self.indices[indices.page].release(indices.range);
                    return Err(e);
                }
            };
            while self.profile_binds.len() < self.profile_tables.len() {
                let page = self.profile_binds.len();
                self.profile_binds.push(profile_bind(
                    device,
                    &self.layout,
                    &self.uniform,
                    &self.profile_tables[page].buffer,
                ));
            }
            let profile_indices: Vec<u32> = tile
                .face_profile_indices
                .iter()
                .map(|i| i + ((table.range.start / 80) as u32))
                .collect();
            let index_bytes = bytemuck::cast_slice(&profile_indices);
            let profile_indices = match self.allocate(device, index_bytes.len() as u64, 2) {
                Ok(p) => p,
                Err(e) => {
                    self.profile_tables[table.page].release(table.range);
                    self.vertices[vertices.page].release(vertices.range);
                    self.indices[indices.page].release(indices.range);
                    return Err(e);
                }
            };
            queue.write_buffer(
                &self.profile_tables[table.page].buffer,
                table.range.start,
                table_bytes,
            );
            queue.write_buffer(
                &self.profiles[profile_indices.page].buffer,
                profile_indices.range.start,
                index_bytes,
            );
            Some(ProfileAllocation {
                indices: Some(profile_indices),
                table,
            })
        };
        queue.write_buffer(
            &self.vertices[vertices.page].buffer,
            vertices.range.start,
            vb,
        );
        queue.write_buffer(&self.indices[indices.page].buffer, indices.range.start, ib);
        let mut bounds = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
        for vertex in &mesh.vertices {
            let p = Vec3::from_array(vertex.position);
            bounds[0] = bounds[0].min(p);
            bounds[1] = bounds[1].max(p);
        }
        if crate::ground_cover::shell_enabled() && tile.face_profiles.iter().any(|p| p[0][3] == -6.)
        {
            bounds[1].y += 1.;
        }
        self.residency_revision = self.residency_revision.wrapping_add(1);
        self.clusters.insert(
            tile.key,
            Cluster {
                appearance_pending: tile.cover_input.is_some(),
                cover: tile.face_profiles.iter().any(|p| p[0][3] == -6.),
                native_quads: false,
                proxy_triangles: false,
                indexed_proxy: false,
                derived_proxy: false,
                position_bytes: 0,
                position_reserved_bytes: 0,
                bins: vec![],
                face_bounds: vec![],
                exact: tile.exact,
                profile,
                vertices,
                indices: Some(indices),
                count: mesh.indices.len() as u32,
                origin: DVec3::new(
                    mesh.origin.x as f64,
                    mesh.origin.y as f64,
                    mesh.origin.z as f64,
                ) * frontier_world::voxel_size(),
                bounds,
                canonical_bounds: Aabb {
                    min: tile.conservative_surface_bounds[0],
                    max: tile.conservative_surface_bounds[1],
                },
                guaranteed_solid: tile.guaranteed_solid_box,
                proxy_bounds: tile.bounds,
                certificate_kind: certificate_kind(tile),
                water_feature_error_m: tile.water_feature_error_m,
                best_error_m: tile
                    .best_certified_vertical_error_m
                    .filter(|e| e.is_finite() && *e >= 0.),
                slot: self.slots.pop().unwrap(),
            },
        );
        Ok(())
    }
    pub(crate) fn appearance_pending(&self, key: &TileKey) -> bool {
        self.clusters
            .get(key)
            .is_some_and(|cluster| cluster.appearance_pending)
    }
    pub(crate) fn appearance_debt(&self, owners: &HashSet<TileKey>) -> usize {
        owners
            .iter()
            .filter(|key| self.appearance_pending(key))
            .count()
    }
    /// Replace complete backing resources while keeping the old cluster alive.
    /// Failed allocation restores the old drawable cluster and its uniform slot.
    pub(crate) fn promote_appearance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tile: &GeometryTile,
    ) -> Result<(), String> {
        let Some(old) = self.clusters.get(&tile.key) else {
            return Ok(());
        };
        if !old.appearance_pending {
            return Ok(());
        }
        if tile.cover_input.is_some()
            || tile.is_empty()
            || tile.exact
            || tile.bounds != old.proxy_bounds
            || tile
                .best_certified_vertical_error_m
                .filter(|e| e.is_finite() && *e >= 0.)
                != old.best_error_m
            || tile.water_feature_error_m != old.water_feature_error_m
            || certificate_kind(tile) != old.certificate_kind
        {
            return Err("appearance promotion changed geometry certificate".into());
        }
        let old = self.clusters.remove(&tile.key).unwrap();
        self.slots.push(old.slot);
        if let Err(error) = self.upload(device, queue, tile) {
            let restored_slot = self.slots.pop();
            debug_assert_eq!(restored_slot, Some(old.slot));
            self.clusters.insert(tile.key, old);
            return Err(error);
        }
        if !self.clusters.contains_key(&tile.key) {
            let restored_slot = self.slots.pop();
            debug_assert_eq!(restored_slot, Some(old.slot));
            self.clusters.insert(tile.key, old);
            return Err("appearance promotion omitted drawable geometry".into());
        }
        if let Some(profile) = old.profile {
            if let Some(indices) = profile.indices {
                self.profiles[indices.page].release(indices.range);
            }
            self.profile_tables[profile.table.page].release(profile.table.range);
        }
        self.vertices[old.vertices.page].release(old.vertices.range);
        if let Some(indices) = old.indices {
            self.indices[indices.page].release(indices.range);
        }
        Ok(())
    }
    /// Only published owners can draw. Keep inactive, immutable coarse
    /// payloads in existing arena pages for a quick turn back.
    pub fn retain(&mut self, keys: &HashSet<TileKey>, allow_warm: bool) {
        let now = Instant::now();
        let budget = if self.warm_enabled && allow_warm {
            (self.resource_budget_bytes / 4).min(WARM_CACHE_MAX_BYTES)
        } else {
            0
        };
        self.warm.retain(|key, &mut (_, bytes)| {
            let active = keys.contains(key);
            if active {
                self.warm_bytes -= bytes;
                self.warm_reuses += 1;
            }
            !active
        });
        let mut removed = Vec::new();
        for (&key, cluster) in &self.clusters {
            if keys.contains(&key) || self.warm.contains_key(&key) {
                continue;
            }
            let bytes = cluster.payload_bytes();
            if budget > 0 && bytes <= budget && key.step_cells > 1 && !cluster.appearance_pending {
                self.warm.insert(key, (now, bytes));
                self.warm_bytes += bytes;
            } else {
                removed.push(key);
            }
        }
        if self.warm_bytes > budget
            || self.warm.len() > WARM_CACHE_MAX_CLUSTERS
            || self
                .warm
                .values()
                .any(|(since, _)| now.duration_since(*since) >= WARM_CACHE_TTL)
        {
            let mut oldest: Vec<_> = self
                .warm
                .iter()
                .map(|(&key, &(since, bytes))| (since, key, bytes))
                .collect();
            oldest.sort_unstable();
            for (since, key, bytes) in oldest {
                if self.warm_bytes <= budget
                    && self.warm.len() <= WARM_CACHE_MAX_CLUSTERS
                    && now.duration_since(since) < WARM_CACHE_TTL
                {
                    break;
                }
                self.warm.remove(&key);
                self.warm_bytes -= bytes;
                self.warm_evictions += 1;
                removed.push(key);
            }
        }
        self.release_clusters(removed);
        self.visible.retain(|item| keys.contains(&item.key));
    }
    /// Published and pending keys are never warm and may use the whole pool.
    pub(super) fn evict_warm(&mut self) -> bool {
        if self.warm.is_empty() {
            return false;
        }
        let keys: Vec<_> = self.warm.drain().map(|(key, _)| key).collect();
        self.warm_evictions += keys.len() as u64;
        self.warm_bytes = 0;
        self.release_clusters(keys);
        true
    }
    fn release_clusters(&mut self, removed: Vec<TileKey>) {
        if !removed.is_empty() {
            self.residency_revision = self.residency_revision.wrapping_add(1);
            self.resident_stats = None;
            self.prepared_view = None;
            self.invalidate_bundle();
        }
        for key in removed {
            let c = self.clusters.remove(&key).unwrap();
            if let Some(p) = c.profile {
                if let Some(indices) = p.indices {
                    self.profiles[indices.page].release(indices.range);
                }
                self.profile_tables[p.table.page].release(p.table.range);
            }
            self.vertices[c.vertices.page].release(c.vertices.range);
            if let Some(indices) = c.indices {
                self.indices[indices.page].release(indices.range);
            }
            self.slots.push(c.slot);
        }
    }
    /// Select only published owners. Resident preparation must never become visible.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &Camera,
        width: u32,
        height: u32,
        owners: &HashSet<TileKey>,
        pixel_error: f64,
    ) -> DrawStats {
        // Reuse only identical current demand and immutable resident content.
        // Upload/removal/filter changes invalidate this snapshot immediately.
        if self.prepared_view_enabled
            && self
                .prepared_view
                .as_ref()
                .is_some_and(|old| old.matches(camera, width, height, pixel_error, owners))
        {
            self.prepared_view_hits += 1;
            let bundle_started = std::time::Instant::now();
            self.prepare_bundle(device);
            let mut stats = self.prepared_view.as_ref().unwrap().stats;
            stats.prepare_bundle_ms = bundle_started.elapsed().as_secs_f64() * 1000.;
            stats.prepare_resident_ms = 0.;
            stats.prepare_upload_ms = 0.;
            stats.prepare_order_reused = false;
            stats.prepare_shadow_ms = 0.;
            stats.prepare_order_ms = 0.;
            stats.prepare_occlusion_ms = 0.;
            stats.prepare_visibility_uniform_ms = 0.;
            self.update_command_stats(&mut stats);
            return stats;
        }
        let shadow_started = std::time::Instant::now();
        self.prepare_shadows(device, queue, camera, owners);
        let shadow_ms = shadow_started.elapsed().as_secs_f64() * 1000.;
        let order_started = std::time::Instant::now();
        self.prepared_view_misses += 1;
        self.visible.clear();
        self.camera_upload.clear();
        let light_frame = adaptive_shadow::LightFrame::new(camera, self.lighting);
        let projection = camera.projection(width as f32 / height as f32);
        let direction = camera.direction();
        let view_transform = LocalViewTransform::new(direction, camera.view_up());
        let queries = crate::adaptive_stream::CameraQueries::new(
            camera,
            width,
            height,
            self.visible_vertical_bound,
        );
        let resident_started = std::time::Instant::now();
        if self.resident_stats.is_none() || !self.prepare_reuse_enabled {
            let mut stats = DrawStats {
                resident_clusters: self.clusters.len(),
                cluster_capacity: MAX_CLUSTERS,
                available_cluster_slots: self.slots.len(),
                allocated_bytes: self.allocated_bytes(),
                ..Default::default()
            };
            for c in self.clusters.values() {
                let profile_bytes = c.profile.as_ref().map_or(0, |p| {
                    p.indices
                        .as_ref()
                        .map_or(0, |i| i.range.end - i.range.start)
                        + p.table.range.end
                        - p.table.range.start
                });
                stats.resident_profile_bytes += profile_bytes - c.position_reserved_bytes;
                stats.resident_proxy_position_bytes += c.position_bytes;
                stats.resident_proxy_position_reserved_bytes += c.position_reserved_bytes;
                stats.resident_indexed_proxy_clusters += u64::from(c.indexed_proxy);
                stats.resident_derived_proxy_clusters += u64::from(c.derived_proxy);
                if c.indexed_proxy {
                    stats.resident_indexed_proxy_record_padding_bytes += c.vertices.range.end
                        - c.vertices.range.start
                        - c.count as u64 * if c.derived_proxy { 20 } else { 40 };
                }
                if c.proxy_triangles {
                    stats.resident_proxy_triangle_bytes +=
                        c.vertices.range.end - c.vertices.range.start;
                }
                if c.native_quads {
                    stats.resident_native_quad_bytes +=
                        c.vertices.range.end - c.vertices.range.start;
                }
                stats.resident_bytes += c.payload_bytes();
            }
            self.resident_stats = Some(stats);
        }
        let mut stats = self.resident_stats.unwrap();
        stats.prepare_resident_ms = resident_started.elapsed().as_secs_f64() * 1000.;
        stats.current_max_error_px = Some(0.);
        stats.current_quality_ready =
            width > 0 && height > 0 && pixel_error.is_finite() && pixel_error >= 0.;
        let forward = camera.direction().as_dvec3();
        let mut bound_vertex_page = None;
        // Rebuild from CURRENT pose and published resident geometry every frame.
        // Pending CPU visibility and old-camera hidden keys never suppress draws.
        let mut occlusion = OcclusionSet::new(
            camera.position.to_array(),
            forward.to_array(),
            0.025,
            frontier_world::voxel_size() * 2.,
        );
        let mut draw_order = if self.prepare_reuse_enabled {
            std::mem::take(&mut self.draw_order)
        } else {
            Vec::new()
        };
        let reuse_order = self.incremental_order_enabled
            && self.prepare_reuse_enabled
            && self
                .draw_order_domain
                .as_ref()
                .is_some_and(|domain| domain.matches(self.residency_revision, owners));
        if reuse_order {
            for (depth, key) in &mut draw_order {
                let c = &self.clusters[key];
                *depth = nearest_depth(c.origin, c.bounds, camera.position, forward);
            }
        } else {
            draw_order.clear();
            draw_order.extend(self.clusters.iter().filter_map(|(&key, c)| {
                (!self.prepare_reuse_enabled || owners.contains(&key)).then(|| {
                    (
                        nearest_depth(c.origin, c.bounds, camera.position, forward),
                        key,
                    )
                })
            }));
            self.draw_order_domain = self.incremental_order_enabled.then(|| DrawOrderDomain {
                residency_revision: self.residency_revision,
                owners: owners.clone(),
            });
        }
        stats.prepare_order_reused = reuse_order;
        sort_draw_order(&mut draw_order);
        stats.prepare_shadow_ms = shadow_ms;
        stats.prepare_order_ms = order_started.elapsed().as_secs_f64() * 1000.;
        let occlusion_started = std::time::Instant::now();
        // Every box belongs to the complete currently published heightfield.
        // Build the same proof set as CPU selection, independent of draw order.
        if let Some(set) = &mut occlusion {
            for (_, key) in &draw_order {
                if owners.contains(key) {
                    if let Some(solid) = self.clusters[key].guaranteed_solid {
                        set.insert_solid(solid);
                    }
                }
            }
        }
        stats.prepare_occlusion_ms = occlusion_started.elapsed().as_secs_f64() * 1000.;
        let visible_started = std::time::Instant::now();
        // Uniform offsets and draw indices use this same stable depth order.
        for &(_, key) in &draw_order {
            let c = &self.clusters[&key];
            if !owners.contains(&key) {
                continue;
            }
            if !queries.visible([c.canonical_bounds.min, c.canonical_bounds.max]) {
                continue;
            }
            if occlusion
                .as_ref()
                .is_some_and(|set| set.occludes(c.canonical_bounds))
            {
                stats.occluded_clusters += 1;
                stats.occluded_triangles += if c.native_quads {
                    c.count as u64 * 2
                } else if c.proxy_triangles {
                    c.count as u64
                } else {
                    c.count as u64 / 3
                };
                continue;
            }
            match c.certificate_kind {
                1 => stats.current_local_certified_clusters += 1,
                2 => stats.current_affine_bound_clusters += 1,
                3 => stats.current_global_bound_clusters += 1,
                _ => {}
            }
            let error = c
                .best_error_m
                .map_or(f64::INFINITY, |e| {
                    queries.vertical_error(e, c.proxy_bounds, c.origin)
                })
                .max(queries.feature_error(c.water_feature_error_m, c.proxy_bounds));
            let error = crate::adaptive_stream::weighted_near_detail_error(
                error,
                [c.canonical_bounds.min, c.canonical_bounds.max],
                camera.position,
                self.near_detail,
            );
            if error.is_finite() {
                stats.current_max_finite_error_px = Some(
                    stats
                        .current_max_finite_error_px
                        .map_or(error, |max| max.max(error)),
                );
            }
            if !error.is_finite() {
                stats.current_max_error_px = None;
            } else if let Some(max) = &mut stats.current_max_error_px {
                *max = max.max(error);
            }
            if c.best_error_m.is_none() {
                stats.current_uncertified_clusters += 1;
            }
            if !error.is_finite() || error > pixel_error {
                stats.current_unresolved_clusters += 1;
                stats.current_quality_ready = false;
                record_offender(
                    &mut stats.current_offenders,
                    VisibleOffender {
                        key,
                        canonical_bounds: [c.canonical_bounds.min, c.canonical_bounds.max],
                        proxy_bounds: c.proxy_bounds,
                        cached_vertical_error_m: c.best_error_m,
                        water_feature_error_m: c.water_feature_error_m,
                        projected_error_px: error.is_finite().then_some(error),
                    },
                );
            }
            let local_eye = (camera.position - c.origin).as_vec3();
            let matrix = projection
                * if self.prepare_reuse_enabled {
                    view_transform.matrix(local_eye)
                } else {
                    Mat4::look_to_rh(local_eye, direction, camera.view_up())
                };
            // Demand uses bounds on the procedural source. Rendering can use
            // the tighter bounds of the published faces after recording that
            // demand, including when the enclosing source interval crosses a
            // ridge outside the actual camera frustum.
            let raster_clip = RasterClip::new(matrix, c.bounds);
            if !raster_clip.intersects(c.bounds) {
                stats.clipped_draw_clusters += 1;
                continue;
            }
            let (bin_mask, native_count) = if c.native_quads
                && (c.exact || self.coarse_batch.is_none())
                && self.native_bins_enabled
                && !c.bins.is_empty()
            {
                let eye = (camera.position - c.origin).to_array();
                let mut mask = 0u32;
                let mut count = 0u32;
                for (index, bin) in c.bins.iter().enumerate() {
                    stats.native_bin_tests += 1;
                    if bin.backfacing(eye, frontier_world::voxel_size() / 1024.) {
                        stats.native_bin_skipped_quads += u64::from(bin.quad_count);
                        continue;
                    }
                    if c.face_bounds
                        .get(index)
                        .is_some_and(|bounds| !raster_clip.intersects(*bounds))
                    {
                        stats.clipped_region_quads += u64::from(bin.quad_count);
                        continue;
                    }
                    mask |= 1u32 << index;
                    count += bin.quad_count;
                }
                if count == 0 {
                    continue;
                }
                (Some(mask), count)
            } else {
                (None, c.count)
            };
            let mut bytes = [0u8; 208];
            bytes[..64].copy_from_slice(bytemuck::cast_slice(&matrix.to_cols_array()));
            bytes[64..80].copy_from_slice(bytemuck::cast_slice(&local_eye.extend(1.).to_array()));
            let paint = [
                (c.origin.x / frontier_world::voxel_size()).round() as i64 as u32,
                (c.origin.y / frontier_world::voxel_size()).round() as i64 as u32,
                (c.origin.z / frontier_world::voxel_size()).round() as i64 as u32,
                (self.voxel_filter as u32)
                    | ((c.exact as u32) << 1)
                    | (((self.lighting[11] > 0.5) as u32) << 2)
                    | (((c.native_quads && !c.exact) as u32) << 3)
                    | (u32::from(
                        c.native_quads
                            && !c.exact
                            && crate::adaptive_geometry::distant_voxels::Mode::current()
                                == crate::adaptive_geometry::distant_voxels::Mode::Blocks,
                    ) << 5),
            ];
            bytes[80..96].copy_from_slice(bytemuck::cast_slice(&paint));
            let mut lighting = self.lighting;
            lighting[11] = ((key.width_cells.ilog2() << 5) | key.step_cells.ilog2()) as f32;
            bytes[96..144].copy_from_slice(bytemuck::cast_slice(&lighting));
            bytes[144..208].copy_from_slice(bytemuck::cast_slice(
                &light_frame.matrix(c.origin).to_cols_array(),
            ));
            let offset = self.camera_upload.len();
            self.camera_upload
                .resize(offset + UNIFORM_STRIDE as usize, 0);
            self.camera_upload[offset..offset + 208].copy_from_slice(&bytes);
            // A receiver box swept towards the sun can be rejected only if
            // every possible ray misses the full exact volume. Use actual
            // resident vertex bounds, not a sampled terrain height estimate.
            let receiver = [
                (c.bounds[0].as_dvec3() + c.origin).to_array(),
                (c.bounds[1].as_dvec3() + c.origin).to_array(),
            ];
            let no_voxel_shadow = self.far_pipelines.is_some()
                && self.naadf_shadow_bounds.is_some_and(|volume| {
                    !sun_sweep_intersects(receiver, volume, self.lighting[..3].try_into().unwrap())
                });
            stats.no_voxel_shadow_clusters += usize::from(no_voxel_shadow);
            self.visible.push(DrawItem {
                key,
                bin_mask,
                no_voxel_shadow,
            });
            if packed_first_instance_stride(
                self.coalesce_vertex_pages,
                c.native_quads,
                c.proxy_triangles,
                &c.vertices.range,
                self.vertices[c.vertices.page].size,
                c.count,
                if c.derived_proxy {
                    20
                } else if c.indexed_proxy {
                    40
                } else if c.native_quads {
                    32
                } else {
                    64
                },
            )
            .is_some()
            {
                stats.vertex_page_binds_eligible += 1;
                if bound_vertex_page == Some(c.vertices.page) {
                    stats.vertex_page_binds_avoided += 1;
                }
                bound_vertex_page = Some(c.vertices.page);
            } else {
                bound_vertex_page = None;
            }
            stats.visible_clusters += 1;
            stats.draw_calls += bin_mask.map_or(1, |m| u64::from((m & !(m << 1)).count_ones()));
            if c.native_quads {
                stats.visible_native_quads += native_count as u64;
            }
            stats.triangles += if c.native_quads {
                native_count as u64 * 2
            } else if c.proxy_triangles {
                c.count as u64
            } else {
                c.count as u64 / 3
            };
        }
        self.draw_order = draw_order;
        stats.occluder_boxes = occlusion.as_ref().map_or(0, |set| set.len());
        stats.prepare_visibility_uniform_ms = visible_started.elapsed().as_secs_f64() * 1000.;
        let upload_started = std::time::Instant::now();
        if !self.camera_upload.is_empty() {
            queue.write_buffer(&self.uniform, 0, &self.camera_upload);
        }
        coarse_batch::Batch::prepare(self, queue);
        stats.prepare_upload_ms = upload_started.elapsed().as_secs_f64() * 1000.;
        let bundle_started = std::time::Instant::now();
        self.prepare_bundle(device);
        stats.prepare_bundle_ms = bundle_started.elapsed().as_secs_f64() * 1000.;
        self.update_command_stats(&mut stats);
        if self.prepared_view_enabled {
            self.prepared_view = Some(PreparedView {
                camera: *camera,
                width,
                height,
                pixel_error,
                owners: owners.clone(),
                stats,
            });
        }
        stats
    }
    fn update_command_stats(&self, stats: &mut DrawStats) {
        stats.warm_clusters = self.warm.len();
        stats.warm_bytes = self.warm_bytes;
        stats.warm_reuses = self.warm_reuses;
        stats.warm_evictions = self.warm_evictions;
        stats.prepare_reuse_enabled = self.prepare_reuse_enabled;
        stats.gpu_profiles = self.gpu_profile_stats;
        stats.indexed_proxy_enabled = self.indexed_proxy_enabled;
        stats.indexed_proxy_mode = self.indexed_proxy_mode;
        stats.indexed_proxy_pack_ms = self.indexed_proxy_pack_ms;
        stats.indexed_proxy_pack_attempts = self.indexed_proxy_pack_attempts;
        stats.indexed_proxy_pack_accepted = self.indexed_proxy_pack_accepted;
        stats.indexed_proxy_peak_staging_estimate_bytes =
            self.indexed_proxy_peak_staging_estimate_bytes;
        if let Some(batch) = &self.coarse_batch {
            (stats.coarse_batch_groups, stats.coarse_batch_candidates) = batch.stats();
        }
        stats.bundle_enabled = self.bundles_enabled;
        stats.vertex_pages_coalescing_enabled = self.coalesce_vertex_pages;
        stats.bundle_active = self.bundle.is_some();
        stats.bundle_builds = self.bundle_builds;
        stats.bundle_reuses = self.bundle_reuses;
        stats.bundle_build_ms = self.bundle_build_ms;
        stats.prepared_view_hits = self.prepared_view_hits;
        stats.prepared_view_misses = self.prepared_view_misses;
    }
    pub(crate) fn encode_coarse_cull(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(batch) = &self.coarse_batch {
            batch.encode_cull(encoder);
        }
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_bind_group(1, &self.shadows.sampling_bind, &[]);
        pass.set_bind_group(2, &self.shadows.near.bind, &[]);
        self.draw_edits(pass);
        if let Some(bundle) = &self.bundle {
            pass.execute_bundles(std::iter::once(bundle));
        } else {
            encode_cluster_draws!(self, &mut *pass);
        }
        if let Some(batch) = &self.coarse_batch {
            batch.draw(self, pass);
        }
    }
    fn invalidate_bundle(&mut self) {
        self.bundle = None;
        self.bundle_signature.clear();
        self.bundle_stable_frames = 0;
    }
    fn prepare_bundle(&mut self, device: &wgpu::Device) {
        if !self.bundles_enabled || self.visible.is_empty() {
            self.invalidate_bundle();
            return;
        }
        if self.visible != self.bundle_signature {
            self.bundle = None;
            self.bundle_signature.clone_from(&self.visible);
            self.bundle_stable_frames = 0;
            return;
        }
        if self.bundle.is_some() {
            self.bundle_reuses += 1;
            return;
        }
        self.bundle_stable_frames += 1;
        // Avoid encoding a second command stream for transient view selections.
        if self.bundle_stable_frames < 2 {
            return;
        }
        let started = std::time::Instant::now();
        let mut encoder =
            device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                label: Some("adaptive stable draw commands"),
                color_formats: &[Some(self.format)],
                depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_read_only: false,
                    stencil_read_only: true,
                }),
                sample_count: self.sample_count,
                multiview: None,
            });
        encoder.set_bind_group(1, &self.shadows.sampling_bind, &[]);
        encoder.set_bind_group(2, &self.shadows.near.bind, &[]);
        encode_cluster_draws!(&*self, &mut encoder);
        let bundle = encoder.finish(&wgpu::RenderBundleDescriptor {
            label: Some("adaptive stable draw commands"),
        });
        self.bundle = Some(bundle);
        self.bundle_builds += 1;
        self.bundle_build_ms += started.elapsed().as_secs_f64() * 1000.;
    }
    fn upload_native(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tile: &GeometryTile,
    ) -> Result<(), String> {
        if tile
            .native_quads
            .iter()
            .any(|q| q.profile_index as usize >= tile.face_profiles.len())
        {
            return Err("native quad profile index outside table".into());
        }
        let vertices = self.allocate(device, (tile.native_quads.len() * 32) as u64, 0)?;
        let table_bytes = bytemuck::cast_slice(&tile.face_profiles);
        let table = match self.allocate(device, table_bytes.len() as u64, 3) {
            Ok(table) => table,
            Err(e) => {
                self.vertices[vertices.page].release(vertices.range);
                return Err(e);
            }
        };
        while self.profile_binds.len() < self.profile_tables.len() {
            let page = self.profile_binds.len();
            self.profile_binds.push(profile_bind(
                device,
                &self.layout,
                &self.uniform,
                &self.profile_tables[page].buffer,
            ));
        }
        let slot = self.slots.last().copied().unwrap();
        if let Some(batch) = &mut self.coarse_batch {
            while batch.profile_binds.len() < self.profile_tables.len() {
                let page = batch.profile_binds.len();
                batch.add_profile_page(device, &self.profile_tables[page].buffer);
            }
        }
        let origin = DVec3::new(
            tile.mesh.origin.x as f64,
            tile.mesh.origin.y as f64,
            tile.mesh.origin.z as f64,
        ) * frontier_world::voxel_size();
        let mut bounds = tile
            .bounds
            .map(|b| (DVec3::from_array(b) - origin).as_vec3());
        let cover = tile.face_profiles.iter().any(|p| p[0][3] == -6.);
        let raised_cover = crate::ground_cover::shell_enabled() && cover;
        if raised_cover {
            bounds[1].y += 1.;
        }
        let mut quads = tile.native_quads.clone();
        let mut bins = validated_bins(&tile.native_bins, quads.len() as u32);
        let mut face_bounds = Vec::new();
        if !tile.exact
            && quads.len() >= 512
            && !bins.is_empty()
            && self.coarse_batch.is_none()
            && self.face_regions_enabled
        {
            (bins, face_bounds) = face_regions::partition(
                &tile.native_quads,
                &mut quads,
                &bins,
                (bounds[0] + bounds[1]) * 0.5,
            );
            if raised_cover {
                for region in &mut face_bounds {
                    region[1].y += 1.;
                }
            }
        }
        for quad in &mut quads {
            quad.profile_index += (table.range.start / 80) as u32;
            quad.axis_sign |= slot << 3;
        }
        queue.write_buffer(
            &self.vertices[vertices.page].buffer,
            vertices.range.start,
            bytemuck::cast_slice(&quads),
        );
        queue.write_buffer(
            &self.profile_tables[table.page].buffer,
            table.range.start,
            table_bytes,
        );
        self.residency_revision = self.residency_revision.wrapping_add(1);
        self.clusters.insert(
            tile.key,
            Cluster {
                appearance_pending: tile.cover_input.is_some(),
                cover,
                native_quads: true,
                proxy_triangles: false,
                indexed_proxy: false,
                derived_proxy: false,
                position_bytes: 0,
                position_reserved_bytes: 0,
                bins,
                face_bounds,
                exact: tile.exact,
                profile: Some(ProfileAllocation {
                    indices: None,
                    table,
                }),
                vertices,
                indices: None,
                count: quads.len() as u32,
                origin,
                bounds,
                canonical_bounds: Aabb {
                    min: tile.conservative_surface_bounds[0],
                    max: tile.conservative_surface_bounds[1],
                },
                guaranteed_solid: tile.guaranteed_solid_box,
                proxy_bounds: tile.bounds,
                certificate_kind: certificate_kind(tile),
                water_feature_error_m: tile.water_feature_error_m,
                best_error_m: tile
                    .best_certified_vertical_error_m
                    .filter(|e| e.is_finite() && *e >= 0.),
                slot: self.slots.pop().unwrap(),
            },
        );
        Ok(())
    }
    fn upload_proxy(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tile: &GeometryTile,
    ) -> Result<(), String> {
        if !crate::spatial_appearance::valid_table(&tile.face_profiles) {
            return Err("invalid spatial appearance table".into());
        }
        if tile
            .proxy_triangles
            .iter()
            .any(|q| q.profile_index as usize >= tile.face_profiles.len())
        {
            return Err("proxy triangle profile index outside table".into());
        }
        if let Some(deferred) = &tile.gpu_profiles {
            if tile.key.width_cells != 128
                || tile.key.step_cells < 8
                || deferred.source_revision != tile.key.source_revision
                || !deferred
                    .page
                    .matches_tile(tile.key.min, tile.key.source_revision)
                || tile.face_profiles.len() != deferred.jobs.len() + 1
                || tile
                    .face_profiles
                    .iter()
                    .flatten()
                    .flatten()
                    .any(|v| *v != 0.)
            {
                return Err("deferred GPU profile source/table contract mismatch".into());
            }
            deferred.page.validate_jobs(&deferred.jobs)?;
            // Reserve fixed scratch under the same hard GPU budget as arenas.
            if self.gpu_profiles.is_none() {
                const SCRATCH_BYTES: u64 = crate::gpu_ordered_profiles::RESIDENT_BYTES;
                if self.allocated_bytes().saturating_add(SCRATCH_BYTES) > self.resource_budget_bytes
                {
                    return Err(POOL_CAPACITY_ERROR.into());
                }
                let started = std::time::Instant::now();
                self.gpu_profiles = Some(GpuOrderedProfiles::new_resident(device));
                self.gpu_profile_stats.initialization_ms += started.elapsed().as_secs_f64() * 1000.;
            }
        }
        let started = std::time::Instant::now();
        let mut indexed = if self.indexed_proxy_enabled {
            self.indexed_proxy_pack_attempts += 1;
            let mut staging = 0;
            let mut packed = proxy_indexed::pack(&tile.proxy_triangles, &mut staging);
            if self.indexed_proxy_mode == 2 {
                if let Some(packed) = &mut packed {
                    packed.derive_attributes(&mut staging);
                }
            }
            self.indexed_proxy_peak_staging_estimate_bytes = self
                .indexed_proxy_peak_staging_estimate_bytes
                .max(staging as u64);
            packed
        } else {
            None
        };
        if self.indexed_proxy_enabled {
            self.indexed_proxy_pack_ms += started.elapsed().as_secs_f64() * 1000.;
        }
        if indexed.is_some() {
            self.indexed_proxy_pack_accepted += 1;
        }
        let derived = indexed.as_ref().is_some_and(|p| p.derived.is_some());
        let stride = if derived {
            20
        } else if indexed.is_some() {
            40
        } else {
            64
        };
        let vertices = self.allocate(
            device,
            (tile.proxy_triangles.len() * stride) as u64,
            if indexed.is_some() { 4 } else { 0 },
        )?;
        let table_bytes = bytemuck::cast_slice(&tile.face_profiles);
        let position_bytes = indexed
            .as_ref()
            .map_or(0, |p| p.positions.len() as u64 * 12);
        // Every profile allocation must start at a multiple of its 80B stride.
        let table_size = (table_bytes.len() as u64 + position_bytes).next_multiple_of(80);
        let table_extra = table_size - table_bytes.len() as u64;
        let table = match self.allocate(device, table_size, 3) {
            Ok(table) => table,
            Err(e) => {
                self.vertices[vertices.page].release(vertices.range);
                return Err(e);
            }
        };
        while self.profile_binds.len() < self.profile_tables.len() {
            let page = self.profile_binds.len();
            self.profile_binds.push(profile_bind(
                device,
                &self.layout,
                &self.uniform,
                &self.profile_tables[page].buffer,
            ));
        }
        if let Some(packed) = &mut indexed {
            let base = ((table.range.start + table_bytes.len() as u64) / 4) as u32;
            packed.relocate(base, (table.range.start / 80) as u32);
            queue.write_buffer(
                &self.vertices[vertices.page].buffer,
                vertices.range.start,
                if let Some(derived) = &packed.derived {
                    bytemuck::cast_slice(derived)
                } else {
                    bytemuck::cast_slice(&packed.triangles)
                },
            );
            queue.write_buffer(
                &self.profile_tables[table.page].buffer,
                table.range.start + table_bytes.len() as u64,
                bytemuck::cast_slice(&packed.positions),
            );
        } else {
            let mut triangles = tile.proxy_triangles.clone();
            for triangle in &mut triangles {
                triangle.profile_index += (table.range.start / 80) as u32;
            }
            queue.write_buffer(
                &self.vertices[vertices.page].buffer,
                vertices.range.start,
                bytemuck::cast_slice(&triangles),
            );
        }
        if let Some(deferred) = &tile.gpu_profiles {
            let started = std::time::Instant::now();
            let builder = self.gpu_profiles.as_mut().unwrap();
            // All fallible descriptor checks ran before allocations. Submit each
            // scratch batch immediately: queue writes precede this submission,
            // and later tiles cannot overwrite inputs before this dispatch.
            let reused = builder
                .upload_cached_page(device, queue, &deferred.page, &deferred.jobs)
                .expect("validated deferred profile descriptors");
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("adaptive deferred appearance publication"),
            });
            builder
                .encode_resident(&mut encoder)
                .expect("uploaded profile batch");
            // Copy only profiles, preserving indexed position data after table.
            encoder.copy_buffer_to_buffer(
                builder.profile_buffer(),
                0,
                &self.profile_tables[table.page].buffer,
                table.range.start,
                table_bytes.len() as u64,
            );
            queue.submit(Some(encoder.finish()));
            let stats = &mut self.gpu_profile_stats;
            stats.source_cache_hits += u64::from(reused);
            stats.source_uploads += u64::from(!reused);
            stats.source_upload_bytes += if reused {
                0
            } else {
                deferred.page.payload_bytes() as u64
            };
            stats.job_upload_bytes += (deferred.jobs.len() * 32 + 80) as u64;
            stats.result_copy_bytes += table_bytes.len() as u64;
            stats.cpu_profile_upload_bytes_avoided += table_bytes.len() as u64;
            stats.dispatches += 1;
            stats.profiles += deferred.jobs.len() as u64;
            stats.submits += 1;
            stats.enqueue_ms += started.elapsed().as_secs_f64() * 1000.;
            stats.resident_gpu_bytes = builder.resident_bytes();
            stats.source_cache_cpu_bytes = builder.source_cache_cpu_bytes();
        } else {
            queue.write_buffer(
                &self.profile_tables[table.page].buffer,
                table.range.start,
                table_bytes,
            );
        }
        let origin = DVec3::new(
            tile.mesh.origin.x as f64,
            tile.mesh.origin.y as f64,
            tile.mesh.origin.z as f64,
        ) * frontier_world::voxel_size();
        let mut bounds = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
        for triangle in &tile.proxy_triangles {
            for p in triangle.positions {
                let p = Vec3::from_array(p);
                bounds[0] = bounds[0].min(p);
                bounds[1] = bounds[1].max(p);
            }
        }
        if crate::ground_cover::shell_enabled() && tile.face_profiles.iter().any(|p| p[0][3] == -6.)
        {
            bounds[1].y += 1.;
        }
        self.residency_revision = self.residency_revision.wrapping_add(1);
        self.clusters.insert(
            tile.key,
            Cluster {
                appearance_pending: tile.cover_input.is_some(),
                cover: tile.face_profiles.iter().any(|p| p[0][3] == -6.),
                native_quads: false,
                proxy_triangles: true,
                indexed_proxy: indexed.is_some(),
                derived_proxy: derived,
                position_bytes: indexed
                    .as_ref()
                    .map_or(0, |p| p.positions.len() as u64 * 12),
                position_reserved_bytes: table_extra,
                bins: vec![],
                face_bounds: vec![],
                exact: tile.exact,
                profile: Some(ProfileAllocation {
                    indices: None,
                    table,
                }),
                vertices,
                indices: None,
                count: tile.proxy_triangles.len() as u32,
                origin,
                bounds,
                canonical_bounds: Aabb {
                    min: tile.conservative_surface_bounds[0],
                    max: tile.conservative_surface_bounds[1],
                },
                guaranteed_solid: tile.guaranteed_solid_box,
                proxy_bounds: tile.bounds,
                certificate_kind: certificate_kind(tile),
                water_feature_error_m: tile.water_feature_error_m,
                best_error_m: tile
                    .best_certified_vertical_error_m
                    .filter(|e| e.is_finite() && *e >= 0.),
                slot: self.slots.pop().unwrap(),
            },
        );
        Ok(())
    }
}

fn validated_bins(bins: &[NativeBin], quad_count: u32) -> Vec<NativeBin> {
    let mut end = 0u32;
    if bins.is_empty() || bins.len() > 8 {
        return vec![];
    }
    for bin in bins {
        if bin.first_quad != end || bin.quad_count == 0 || bin.axis_sign & 3 > 2 {
            return vec![];
        }
        let Some(next) = end.checked_add(bin.quad_count) else {
            return vec![];
        };
        end = next;
    }
    if end == quad_count {
        bins.to_vec()
    } else {
        vec![]
    }
}

fn certificate_kind(tile: &GeometryTile) -> u8 {
    if tile.exact {
        return 0;
    }
    let Some(best) = tile.best_certified_vertical_error_m else {
        return 0;
    };
    if tile.certified_error_m == Some(best) {
        1
    } else if tile.affine_error_m == Some(best) {
        2
    } else if tile.global_error_m == Some(best) {
        3
    } else {
        0
    }
}

struct RasterClip {
    planes: [glam::Vec4; 6],
}
impl RasterClip {
    fn new(matrix: Mat4, [lo, hi]: [Vec3; 2]) -> Self {
        let r = matrix.transpose();
        let abs = lo.abs().max(hi.abs()).extend(1.);
        // Each hardware test uses only its particular clip coordinates. In
        // reversed depth the far plane uses z alone, whose coefficients are
        // orders of magnitude smaller than w. A tolerance based on w would
        // admit entire terrain tiles past the far plane.
        let [x, y, z, w_err] = [r.x_axis, r.y_axis, r.z_axis, r.w_axis]
            .map(|row| 16. * f32::EPSILON * row.abs().dot(abs));
        let w = r.w_axis;
        let mut planes = [
            w + r.x_axis,
            w - r.x_axis,
            w + r.y_axis,
            w - r.y_axis,
            r.z_axis,
            w - r.z_axis,
        ];
        for (plane, margin) in
            planes
                .iter_mut()
                .zip([w_err + x, w_err + x, w_err + y, w_err + y, z, w_err + z])
        {
            plane.w += margin;
        }
        Self { planes }
    }
    fn intersects(&self, [lo, hi]: [Vec3; 2]) -> bool {
        self.planes.into_iter().all(|plane| {
            let n = plane.truncate();
            let support = Vec3::select(n.cmpge(Vec3::ZERO), hi, lo);
            n.dot(support) + plane.w >= 0.
        })
    }
}
#[cfg(test)]
fn intersects_raster_clip(matrix: Mat4, bounds: [Vec3; 2]) -> bool {
    RasterClip::new(matrix, bounds).intersects(bounds)
}
fn nearest_depth(origin: DVec3, bounds: [Vec3; 2], eye: DVec3, forward: DVec3) -> f64 {
    let local = Vec3::new(
        if forward.x >= 0. {
            bounds[0].x
        } else {
            bounds[1].x
        },
        if forward.y >= 0. {
            bounds[0].y
        } else {
            bounds[1].y
        },
        if forward.z >= 0. {
            bounds[0].z
        } else {
            bounds[1].z
        },
    );
    ((origin - eye) + local.as_dvec3()).dot(forward)
}

fn profile_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    table: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pooled canonical profile page"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: uniform,
                    offset: 0,
                    size: std::num::NonZeroU64::new(208),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: table.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: table.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::ADAPTIVE_SHADER;
    #[test]
    fn experimental_vertex_padding_preserves_default_and_releases_full_reservations() {
        use super::*;
        let payloads = [108, 7 * 32, 3 * 64, 36, 32];
        let mut free = vec![0..4096];
        let mut ranges = vec![];
        let mut reserved = 0;
        for bytes in payloads {
            assert_eq!(vertex_reservation_bytes(bytes, false), Some(bytes));
            let padded = vertex_reservation_bytes(bytes, true).unwrap();
            assert!(padded >= bytes && padded - bytes <= 63);
            let range = allocate_range(&mut free, padded).unwrap();
            assert_eq!(range.start % 64, 0);
            assert_eq!(range.end - range.start, padded);
            reserved += padded;
            ranges.push(range);
        }
        assert_eq!(
            free.iter().map(|r| r.end - r.start).sum::<u64>() + reserved,
            4096
        );
        for range in ranges.into_iter().rev() {
            release_range(&mut free, range);
        }
        assert_eq!(free, vec![0..4096]);
        assert!(vertex_reservation_bytes(u64::MAX, true).is_none());
        assert_eq!(vertex_reservation_bytes(PAGE_BYTES, true), Some(PAGE_BYTES));
        assert!(63 * (MAX_CLUSTERS as u64) < 1024 * 1024);
    }

    #[test]
    fn packed_page_instance_addresses_match_slices_and_native_bin_ranges() {
        use super::*;
        for (native, proxy, stride, allocation, count) in [
            (true, false, 32u64, 320..576, 7),
            (false, true, 64u64, 128..320, 3),
        ] {
            let base =
                packed_first_instance(true, native, proxy, &allocation, 4096, count).unwrap();
            // Fetch every byte of every record, not just the first vertex.
            for instance in 0..count {
                for byte in 0..stride {
                    assert_eq!(
                        u64::from(base + instance) * stride + byte,
                        allocation.start + u64::from(instance) * stride + byte,
                    );
                }
            }
            if native {
                for bin in [0..2, 2..5, 5..7] {
                    let old: Vec<_> = bin
                        .clone()
                        .map(|i| allocation.start + u64::from(i) * stride)
                        .collect();
                    let new: Vec<_> = (base + bin.start..base + bin.end)
                        .map(|i| u64::from(i) * stride)
                        .collect();
                    assert_eq!(old, new);
                }
            }
            assert!(
                packed_first_instance(false, native, proxy, &allocation, 4096, count).is_none()
            );
        }
        // Per-vertex generic geometry keeps its local index/base-vertex meaning.
        assert!(packed_first_instance(true, false, false, &(128..256), 4096, 3).is_none());
        assert!(packed_first_instance(true, true, true, &(128..256), 4096, 3).is_none());
    }

    #[test]
    fn packed_page_addressing_falls_back_on_misalignment_bounds_or_integer_overflow() {
        use super::*;
        for (range, page, count) in [
            (4..68, 4096, 2),   // native record alignment
            (64..96, 4096, 2),  // reservation shorter than payload
            (64..128, 127, 2),  // allocation leaves its page
            (128..64, 4096, 1), // malformed reservation
            (
                (u64::from(u32::MAX) * 32)..(u64::from(u32::MAX) * 32 + 32),
                u64::MAX,
                1,
            ),
        ] {
            assert!(packed_first_instance(true, true, false, &range, page, count).is_none());
        }
        assert!(packed_first_instance(true, false, true, &(32..160), 4096, 2).is_none());
        assert_eq!(
            packed_first_instance(true, true, false, &(64..128), 128, 2),
            Some(2)
        );
        assert_eq!(
            packed_first_instance(true, false, true, &(64..192), 192, 2),
            Some(1)
        );
    }
    #[test]
    fn only_explicit_capacity_refusals_allow_coherent_cover_fallback() {
        assert!(super::resource_capacity_error(
            super::CLUSTER_CAPACITY_ERROR
        ));
        assert!(super::resource_capacity_error(super::POOL_CAPACITY_ERROR));
        for message in [
            "proxy triangle profile index outside table",
            "quad tile must have exclusive primitive storage",
            "adaptive GPU pool budget exhausted: invalid record",
            "cancelled",
            "",
        ] {
            assert!(!super::resource_capacity_error(message));
        }
    }
    #[test]
    fn proxy_instance_layout_and_shader_inputs_match_exact_fields() {
        use crate::adaptive_geometry::ProxyTriangle;
        assert_eq!(std::mem::size_of::<ProxyTriangle>(), 64);
        assert_eq!(std::mem::offset_of!(ProxyTriangle, positions), 0);
        assert_eq!(std::mem::offset_of!(ProxyTriangle, normal), 36);
        assert_eq!(std::mem::offset_of!(ProxyTriangle, color), 48);
        assert_eq!(std::mem::offset_of!(ProxyTriangle, profile_index), 60);
        let module = naga::front::wgsl::parse_str(&crate::voxel_shader(ADAPTIVE_SHADER)).unwrap();
        let entry = module
            .entry_points
            .iter()
            .find(|e| e.name == "vs_proxy")
            .unwrap();
        let locations: Vec<_> = entry
            .function
            .arguments
            .iter()
            .filter_map(|a| match a.binding {
                Some(naga::Binding::Location { location, .. }) => {
                    Some((a.name.as_deref().unwrap(), location))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            locations,
            vec![
                ("p0", 0),
                ("p1", 1),
                ("p2", 2),
                ("normal", 3),
                ("color", 4),
                ("profile_index", 5)
            ]
        );
    }
    #[test]
    fn fixed_cluster_uniform_slots_stay_in_bounds_without_changing_arena_budget() {
        assert_eq!(super::MAX_CLUSTERS, 16384);
        assert_eq!(super::CAMERA_UNIFORM_BYTES, 4 * 1024 * 1024);
        assert!(super::CAMERA_UNIFORM_BYTES <= wgpu::Limits::downlevel_defaults().max_buffer_size);
        for i in [0, super::MAX_CLUSTERS - 1] {
            let offset = i as u64 * super::UNIFORM_STRIDE;
            assert_eq!(offset % 256, 0);
            assert!(offset + 208 <= super::CAMERA_UNIFORM_BYTES);
            assert!(offset <= u32::MAX as u64);
        }
        assert_eq!(super::MAX_BYTES, 512 * 1024 * 1024);
    }
    #[test]
    fn offender_diagnostics_are_bounded_deterministic_and_prioritize_unbounded() {
        let make = |x, error| super::VisibleOffender {
            key: super::TileKey {
                min: [x, 0],
                width_cells: 128,
                step_cells: 8,
                edge_steps: [8; 4],
                source_revision: 0,
            },
            canonical_bounds: [[0.; 3]; 2],
            proxy_bounds: [[0.; 3]; 2],
            cached_vertical_error_m: Some(1.),
            water_feature_error_m: 0.,
            projected_error_px: error,
        };
        let mut forward = [None; 8];
        let mut reverse = [None; 8];
        for x in 0..20 {
            super::record_offender(&mut forward, make(x, Some(x as f64)));
        }
        for x in (0..20).rev() {
            super::record_offender(&mut reverse, make(x, Some(x as f64)));
        }
        for top in [&mut forward, &mut reverse] {
            super::record_offender(top, make(40, None));
            super::record_offender(top, make(30, None));
        }
        let keys = |top: [Option<super::VisibleOffender>; 8]| top.map(|v| v.unwrap().key.min[0]);
        assert_eq!(keys(forward), [30, 40, 19, 18, 17, 16, 15, 14]);
        assert_eq!(keys(forward), keys(reverse));
    }
    #[test]
    fn retiring_clusters_coalesces_both_neighbors_and_reuses_the_same_page() {
        let mut free = vec![0..1024];
        let a = super::allocate_range(&mut free, 256).unwrap();
        let b = super::allocate_range(&mut free, 256).unwrap();
        let c = super::allocate_range(&mut free, 256).unwrap();
        super::release_range(&mut free, a);
        super::release_range(&mut free, c);
        assert_eq!(free, vec![0..256, 512..1024]);
        assert!(super::allocate_range(&mut free, 768).is_none());
        super::release_range(&mut free, b);
        assert_eq!(super::allocate_range(&mut free, 1024), Some(0..1024));
        assert!(free.is_empty());
    }
    #[test]
    fn packed_native_material_palette_matches_canonical_world_exactly() {
        use frontier_world::Material;
        let module = naga::front::wgsl::parse_str(&crate::voxel_shader(ADAPTIVE_SHADER)).unwrap();
        let (_, palette) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some("MATERIAL_COLORS"))
            .unwrap();
        let naga::Expression::Compose {
            components: colors, ..
        } = &module.global_expressions[palette.init]
        else {
            panic!("material palette must be constant")
        };
        for (index, material) in [
            Material::Air,
            Material::Grass,
            Material::Soil,
            Material::Stone,
            Material::Wood,
            Material::Leaf,
            Material::Flower,
            Material::Water,
            Material::Sand,
            Material::Snow,
        ]
        .into_iter()
        .enumerate()
        {
            let naga::Expression::Compose {
                components: channels,
                ..
            } = &module.global_expressions[colors[index]]
            else {
                panic!("palette color must be a vector")
            };
            for (channel, expected) in material.color().into_iter().enumerate() {
                let naga::Expression::Literal(naga::Literal::F32(actual)) =
                    module.global_expressions[channels[channel]]
                else {
                    panic!("palette channel must be f32")
                };
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }
        assert_eq!(
            std::mem::size_of::<crate::adaptive_geometry::NativeQuad>(),
            32
        );
    }
    #[test]
    fn front_to_back_sort_uses_conservative_depth_not_center_distance() {
        use glam::{DVec3, Vec3};
        let origin = DVec3::new(100000000., -20., 100000000.);
        let eye = origin + DVec3::new(4., 2., -3.);
        let bounds = [Vec3::new(-2., -4., 1.), Vec3::new(6., 3., 12.)];
        for forward in [
            DVec3::Z,
            -DVec3::Z,
            DVec3::X,
            DVec3::new(-1., 2., -3.).normalize(),
        ] {
            let expected = (0..8)
                .map(|i| {
                    let corner = Vec3::new(
                        bounds[i & 1].x,
                        bounds[(i >> 1) & 1].y,
                        bounds[(i >> 2) & 1].z,
                    )
                    .as_dvec3();
                    (origin + corner - eye).dot(forward)
                })
                .fold(f64::INFINITY, f64::min);
            assert!((super::nearest_depth(origin, bounds, eye, forward) - expected).abs() < 1e-8);
        }
    }
    #[test]
    fn invalid_bin_ranges_fall_back_to_the_complete_native_draw() {
        let a = crate::adaptive_geometry::NativeBin {
            axis_sign: 1,
            first_quad: 0,
            quad_count: 10,
            min_plane: 0.,
            max_plane: 1.,
        };
        let b = crate::adaptive_geometry::NativeBin {
            axis_sign: 0,
            first_quad: 10,
            quad_count: 5,
            min_plane: 0.,
            max_plane: 1.,
        };
        assert_eq!(super::validated_bins(&[a, b], 15).len(), 2);
        assert!(super::validated_bins(
            &[
                a,
                crate::adaptive_geometry::NativeBin {
                    first_quad: 11,
                    ..b
                }
            ],
            15
        )
        .is_empty());
        assert!(super::validated_bins(&[a, b], 16).is_empty());
        assert!(super::validated_bins(&[], 15).is_empty());
    }
    #[test]
    fn indexed_proxy_alignment_reuses_prefix_and_releases_complete_reservation() {
        use super::*;
        let mut free = vec![64..2048];
        let indexed = allocate_aligned_range(&mut free, 640, 320).unwrap();
        assert_eq!(indexed, 320..960);
        assert_eq!(allocate_range(&mut free, 64), Some(64..128));
        assert_eq!(
            packed_first_instance_stride(true, false, true, &indexed, 2048, 16, 40),
            Some(8)
        );
        assert_eq!(
            packed_first_instance_stride(true, false, true, &(64..704), 2048, 16, 40),
            None
        );
        release_range(&mut free, 64..128);
        release_range(&mut free, indexed);
        assert_eq!(free, vec![64..2048]);
    }
    #[test]
    fn indexed_proxy_instance_attributes_match_shader_entry() {
        use super::proxy_indexed::Triangle;
        assert_eq!(std::mem::size_of::<Triangle>(), 40);
        assert_eq!(std::mem::offset_of!(Triangle, positions), 0);
        assert_eq!(std::mem::offset_of!(Triangle, normal), 12);
        assert_eq!(std::mem::offset_of!(Triangle, color), 24);
        assert_eq!(std::mem::offset_of!(Triangle, profile_index), 36);
        let module = naga::front::wgsl::parse_str(&crate::voxel_shader(ADAPTIVE_SHADER)).unwrap();
        let entry = module
            .entry_points
            .iter()
            .find(|e| e.name == "vs_proxy_indexed")
            .unwrap();
        let locations: Vec<_> = entry
            .function
            .arguments
            .iter()
            .filter_map(|a| match a.binding {
                Some(naga::Binding::Location { location, .. }) => {
                    Some((a.name.as_deref().unwrap(), location))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            locations,
            vec![
                ("addresses", 0),
                ("normal", 1),
                ("color", 2),
                ("profile_index", 3)
            ]
        );
    }
    #[test]
    fn derived_proxy_instance_attributes_match_shader_entry() {
        use super::proxy_indexed::DerivedTriangle;
        assert_eq!(std::mem::size_of::<DerivedTriangle>(), 20);
        assert_eq!(std::mem::offset_of!(DerivedTriangle, positions), 0);
        assert_eq!(std::mem::offset_of!(DerivedTriangle, material), 12);
        assert_eq!(std::mem::offset_of!(DerivedTriangle, profile_index), 16);
        let module = naga::front::wgsl::parse_str(&crate::voxel_shader(ADAPTIVE_SHADER)).unwrap();
        let entry = module
            .entry_points
            .iter()
            .find(|e| e.name == "vs_proxy_derived")
            .unwrap();
        let locations: Vec<_> = entry
            .function
            .arguments
            .iter()
            .filter_map(|a| match a.binding {
                Some(naga::Binding::Location { location, .. }) => {
                    Some((a.name.as_deref().unwrap(), location))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            locations,
            vec![("addresses", 0), ("material", 1), ("profile_index", 2)]
        );
        assert_eq!(
            super::packed_first_instance_stride(true, false, true, &(320..960), 2048, 32, 20),
            Some(16)
        );
    }
    #[test]
    fn canonical_shader_validates() {
        let module = naga::front::wgsl::parse_str(&crate::voxel_shader(ADAPTIVE_SHADER)).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn canonical_lighting_and_tint_match_native_source() {
        let shader = include_str!("adaptive.wgsl");
        let native = include_str!("voxel.wgsl");
        let legacy = shader
            .split("fn terrain_lighting(")
            .next()
            .unwrap()
            .replace("fn legacy_terrain_lighting(", "fn terrain_lighting(");
        assert!(native.starts_with(&legacy));
        let paint = shader.split("// Coordinate-only paint").nth(1).unwrap();
        assert!(native.contains(paint.split("struct Camera").next().unwrap().trim()));
    }
    #[test]
    fn frustum_culling_keeps_near_plane_intersections() {
        let mut camera = crate::Camera::fixture();
        camera.position = glam::DVec3::ZERO;
        camera.yaw = 0.;
        camera.pitch = 0.;
        camera.far = 10.;
        assert!(crate::adaptive_stream::cached_bounds_visible(
            [[-0.1, -0.1, -0.5], [0.1, 0.1, 0.5]],
            &camera,
            100,
            100
        ));
        assert!(!crate::adaptive_stream::cached_bounds_visible(
            [[-0.1, -0.1, -1.], [0.1, 0.1, -0.5]],
            &camera,
            100,
            100
        ));
    }
    #[test]
    fn raster_clip_retains_touching_faces_and_matches_projected_corners() {
        use super::{intersects_raster_clip, Mat4, Vec3};
        // WebGPU clips z to the interval from zero to w in both depth conventions.
        let inside = [Vec3::new(-1., -1., 0.), Vec3::ONE];
        assert!(intersects_raster_clip(Mat4::IDENTITY, inside));
        for axis in 0..3 {
            for upper in [false, true] {
                let limit = inside[usize::from(upper)][axis];
                let mut p = Vec3::new(0., 0., 0.5);
                p[axis] = limit;
                assert!(intersects_raster_clip(Mat4::IDENTITY, [p, p]));
                p[axis] += if upper { 0.01 } else { -0.01 };
                assert!(!intersects_raster_clip(Mat4::IDENTITY, [p, p]));
            }
        }
        let reversed = Mat4::perspective_rh(1., 1.7, 6000., 0.025);
        assert!(intersects_raster_clip(
            reversed,
            [Vec3::new(0., 0., -6000.); 2]
        ));
        assert!(!intersects_raster_clip(
            reversed,
            [Vec3::new(-10., 0., -6900.), Vec3::new(10., 5., -6800.)]
        ));
        let eye = Vec3::new(-3762., 440., 1198.);
        let matrix = Mat4::perspective_rh(1., 1.7, 0.12, 6000.)
            * Mat4::look_to_rh(eye, Vec3::new(0.3, -0.15, 0.9).normalize(), Vec3::Y);
        for x in -4..=4 {
            for y in -3..=3 {
                for z in -3..=6 {
                    let lo = eye + 180. * Vec3::new(x as f32, y as f32, z as f32);
                    let hi = lo + Vec3::new(72., 103., 51.);
                    let mut outside = [true; 6];
                    for i in 0..8 {
                        let p = matrix
                            * Vec3::new(
                                if i & 1 == 0 { lo.x } else { hi.x },
                                if i & 2 == 0 { lo.y } else { hi.y },
                                if i & 4 == 0 { lo.z } else { hi.z },
                            )
                            .extend(1.);
                        for (flag, distance) in outside.iter_mut().zip([
                            p.w + p.x,
                            p.w - p.x,
                            p.w + p.y,
                            p.w - p.y,
                            p.z,
                            p.w - p.z,
                        ]) {
                            *flag &= distance < 0.;
                        }
                    }
                    if outside.iter().all(|flag| !flag) {
                        assert!(intersects_raster_clip(matrix, [lo, hi]));
                    }
                }
            }
        }
    }
    #[test]
    fn owner_filter_before_sort_preserves_current_draw_and_occluder_order() {
        use super::{sort_draw_order, HashSet, TileKey};
        let make = |i| TileKey {
            min: [i, -i],
            width_cells: 128,
            step_cells: 8,
            edge_steps: [8; 4],
            source_revision: 3,
        };
        let residents: Vec<_> = (0..97)
            .map(|i| (((i * 17) % 13) as f64 - 6., make(i)))
            .collect();
        for epoch in 0..8 {
            let owners: HashSet<_> = (0..97).filter(|i| (i + epoch) % 3 != 0).map(make).collect();
            let mut original = residents.clone();
            sort_draw_order(&mut original);
            original.retain(|(_, key)| owners.contains(key));
            let mut optimized: Vec<_> = residents
                .iter()
                .copied()
                .filter(|(_, key)| owners.contains(key))
                .collect();
            sort_draw_order(&mut optimized);
            assert_eq!(optimized, original);
            assert!(optimized.iter().all(|(_, key)| owners.contains(key)));
        }
    }

    #[test]
    fn incremental_order_requires_exact_owner_and_residency_identity() {
        use super::{HashSet, TileKey};
        let key = TileKey {
            min: [0, 0],
            width_cells: 128,
            step_cells: 8,
            edge_steps: [8; 4],
            source_revision: 3,
        };
        let owners = HashSet::from([key]);
        let domain = super::DrawOrderDomain {
            residency_revision: 5,
            owners: owners.clone(),
        };
        assert!(domain.matches(5, &owners));
        assert!(!domain.matches(6, &owners));
        let mut replacement = key;
        replacement.source_revision += 1;
        assert!(!domain.matches(5, &HashSet::from([replacement])));
        assert!(!domain.matches(5, &HashSet::new()));
    }

    #[test]
    fn in_place_sort_matches_control_through_ties_and_nonfinite_diagnostics() {
        let mut keys: Vec<_> = (0..4096)
            .map(|i| {
                let key = super::TileKey {
                    min: [i - 2048, -i],
                    width_cells: 64,
                    step_cells: 4,
                    edge_steps: [4; 4],
                    source_revision: 0,
                };
                ((i % 23) as f64, key)
            })
            .collect();
        // Include deterministic signed-zero/nonfinite ordering for diagnostic
        // inputs; production visibility independently rejects invalid bounds.
        keys[0].0 = f64::NAN;
        keys[1].0 = f64::NEG_INFINITY;
        keys[2].0 = -0.;
        keys.reverse();
        let mut control = keys.clone();
        super::sort_draw_order_impl(&mut control, false);
        super::sort_draw_order_impl(&mut keys, true);
        assert_eq!(keys.len(), control.len());
        for (a, b) in keys.iter().zip(&control) {
            assert_eq!(a.0.to_bits(), b.0.to_bits());
            assert_eq!(a.1, b.1);
        }
    }

    #[test]
    fn incremental_order_matches_fresh_sort_through_motion_turn_and_teleport() {
        use super::{nearest_depth, sort_draw_order, TileKey};
        let residents: Vec<_> = (0..97)
            .map(|i| {
                let key = TileKey {
                    min: [i, -i],
                    width_cells: 128,
                    step_cells: 8,
                    edge_steps: [8; 4],
                    source_revision: 3,
                };
                (
                    key,
                    glam::DVec3::new((i % 13) as f64 * 32., 0., (i / 13) as f64 * 32.),
                )
            })
            .collect();
        let bounds = [glam::Vec3::splat(-1.), glam::Vec3::splat(1.)];
        let mut previous: Vec<_> = residents.iter().map(|(k, _)| (0., *k)).rev().collect();
        for (eye, forward) in [
            (glam::DVec3::ZERO, glam::DVec3::Z),
            (glam::DVec3::new(0.1, 0., 0.2), glam::DVec3::Z),
            (glam::DVec3::new(0.1, 0., 0.2), -glam::DVec3::Z),
            (glam::DVec3::new(50000., -300., -42000.), glam::DVec3::X),
        ] {
            let mut fresh: Vec<_> = residents
                .iter()
                .map(|(key, origin)| (nearest_depth(*origin, bounds, eye, forward), *key))
                .collect();
            for (depth, key) in &mut previous {
                let origin = residents.iter().find(|(k, _)| k == key).unwrap().1;
                *depth = nearest_depth(origin, bounds, eye, forward);
            }
            sort_draw_order(&mut previous);
            sort_draw_order(&mut fresh);
            assert_eq!(previous, fresh);
        }
    }

    #[test]
    fn reusable_local_view_transform_matches_per_cluster_matrix_bits() {
        use super::{Camera, Mat4, Vec3};
        for yaw in [-3.1_f32, -1.7, 0., 0.3, 1.9, 3.1] {
            for pitch in [-1.4_f32, -0.3, 0., 0.8, 1.4] {
                let mut camera = Camera::fixture();
                camera.yaw = yaw;
                camera.pitch = pitch;
                let direction = camera.direction();
                let reusable = super::LocalViewTransform::new(direction, camera.view_up());
                for eye in [
                    Vec3::ZERO,
                    Vec3::new(-0.0625, 2., 55.),
                    Vec3::new(99999.25, -312.0625, -12345.5),
                ] {
                    assert_eq!(
                        reusable.matrix(eye).to_cols_array().map(f32::to_bits),
                        Mat4::look_to_rh(eye, direction, Vec3::Y)
                            .to_cols_array()
                            .map(f32::to_bits)
                    );
                }
            }
        }
    }

    #[test]
    fn prepared_view_requires_identical_projection_pose_and_ownership() {
        use super::{Camera, DrawStats, HashSet, PreparedView, TileKey};
        let camera = Camera::fixture();
        let key = TileKey {
            min: [0, 0],
            width_cells: 128,
            step_cells: 8,
            edge_steps: [8; 4],
            source_revision: 0,
        };
        let owners = HashSet::from([key]);
        let cached = PreparedView {
            camera,
            width: 1920,
            height: 1080,
            pixel_error: 5.,
            owners: owners.clone(),
            stats: DrawStats::default(),
        };
        assert!(cached.matches(&camera, 1920, 1080, 5., &owners));
        for field in 0..5 {
            let mut moved = camera;
            match field {
                0 => moved.position.x += 0.0625,
                1 => moved.yaw += 0.01,
                2 => moved.pitch += 0.01,
                3 => moved.far += 100.,
                _ => moved.fov_y += 1.,
            }
            assert!(!cached.matches(&moved, 1920, 1080, 5., &owners));
        }
        assert!(!cached.matches(&camera, 1280, 1080, 5., &owners));
        assert!(!cached.matches(&camera, 1920, 720, 5., &owners));
        assert!(!cached.matches(&camera, 1920, 1080, 4., &owners));
        assert!(!cached.matches(&camera, 1920, 1080, 5., &HashSet::new()));
        let edited = HashSet::from([TileKey {
            source_revision: 1,
            ..key
        }]);
        assert!(!cached.matches(&camera, 1920, 1080, 5., &edited));
        let mut invalid = camera;
        invalid.position.x = f64::NAN;
        assert!(!cached.matches(&invalid, 1920, 1080, 5., &owners));
    }
}

/// Conservative ray/AABB test for a whole AABB of receiver origins.
/// Interval intersection may admit rays from different origins (false positives),
/// but cannot reject a real ray. Touches, invalid inputs and uncertain numerics
/// retain the ordinary shader. Padding covers normal offsets and GPU rounding.
fn sun_sweep_intersects(receiver: [[f64; 3]; 2], volume: [[f64; 3]; 2], sun: [f32; 3]) -> bool {
    let mut enter = 0.0_f64;
    let mut leave = f64::INFINITY;
    for axis in 0..3 {
        let a = receiver[0][axis];
        let b = receiver[1][axis];
        let lo = volume[0][axis];
        let hi = volume[1][axis];
        let d = f64::from(sun[axis]);
        if ![a, b, lo, hi, d].iter().all(|v| v.is_finite()) || a > b || lo > hi {
            return true;
        }
        let pad = 0.125 + 1.0e-6 * a.abs().max(b.abs()).max(lo.abs()).max(hi.abs());
        if d.abs() < 1.0e-7 {
            // Ignore this axis: tiny directions can reach the volume on long rays.
            if d == 0. && (b + pad < lo || a - pad > hi) {
                return false;
            }
        } else {
            let t0 = (lo - b - pad) / d;
            let t1 = (hi - a + pad) / d;
            enter = enter.max(t0.min(t1));
            leave = leave.min(t0.max(t1));
        }
    }
    leave >= enter
}
#[cfg(test)]
mod shadow_specialization_tests {
    use super::sun_sweep_intersects as hit;
    #[test]
    fn sweep_retains_possible_hits_and_rejects_only_separated_rays() {
        let b = [[0.; 3], [1.; 3]];
        assert!(hit([[-2., 0., 0.], [-1., 1., 1.]], b, [1., 0., 0.]));
        assert!(!hit([[2., 0., 0.], [3., 1., 1.]], b, [1., 0., 0.]));
        assert!(!hit([[-2., 2., 0.], [-1., 3., 1.]], b, [1., 0., 0.]));
        assert!(hit(b, b, [1., 1., 1.]));
        assert!(hit([[f64::NAN; 3], [1.; 3]], b, [1.; 3]));
        // Opposite direction and grazing contact must keep the full shader.
        assert!(hit([[2., 1., 0.], [3., 2., 1.]], b, [-1., 0., 0.]));
    }
}
