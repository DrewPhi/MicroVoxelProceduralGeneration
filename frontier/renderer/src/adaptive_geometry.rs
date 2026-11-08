//! Spatial, source-revision keyed terrain clusters. Camera demand never enters a key.
//!
//! Coarse interiors are triangles, never enlarged voxels. Matching canonical
//! boundary strides close neighboring clusters; native joins retain full column
//! stair profiles and canonical exposed faces without a skirt or crop cap.
//! Edited 3D content remains under the separate explicit volume owner.
use crate::{
    column_mesh,
    integrated_profiles::{self, DeferredProfiles, GpuProfilePreparationStats},
    mesh::{Mesh, Vertex},
    source_page_cache::PageCache,
};
use frontier_runtime::occlusion::Aabb;
use frontier_world::{
    affine_bounds::{bound_native_height, BoundBudget, ColumnBox, HeightBound},
    Cell, Material, World, VOXEL_SIZE,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

/// Integer source addresses, independent of camera/view epoch. A cache belongs to
/// one immutable source snapshot; revision is checked before construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct TileKey {
    pub min: [i64; 2],
    pub width_cells: u32,
    pub step_cells: u32,
    /// Edge sample stride: -X,+Z,+X,-Z. Neighboring edges must agree.
    pub edge_steps: [u32; 4],
    pub source_revision: u64,
}

/// Exact canonical quad, with shared topology supplied by the GPU.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NativeQuad {
    pub anchor: [f32; 3],
    pub extent: [f32; 2],
    /// Axis in low two bits, bit 2 set for the negative normal.
    pub axis_sign: u32,
    /// Air/Grass/Soil/Stone/Wood = 0/1/2/3/4.
    pub material: u32,
    pub profile_index: u32,
}
pub(crate) const MAX_NATIVE_QUADS: usize = 4_000_000;
/// Lossless proxy primitive. The three positions retain the indexed triangle's
/// original order; identical per-corner normal/color/profile data is stored once.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ProxyTriangle {
    pub positions: [[f32; 3]; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub profile_index: u32,
}
/// Contiguous exact-axis faces. This is source metadata, not a saved visibility
/// selection; current-camera rejection is a single conservative plane-range test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeBin {
    pub axis_sign: u32,
    pub first_quad: u32,
    pub quad_count: u32,
    pub min_plane: f32,
    pub max_plane: f32,
}
impl NativeBin {
    pub fn backfacing(self, eye_local: [f64; 3], tolerance_m: f64) -> bool {
        let axis = (self.axis_sign & 3) as usize;
        if axis > 2
            || !tolerance_m.is_finite()
            || tolerance_m < 0.
            || eye_local.iter().any(|v| !v.is_finite())
            || !self.min_plane.is_finite()
            || !self.max_plane.is_finite()
            || self.min_plane > self.max_plane
        {
            return false;
        }
        // Metadata planes are exact f32 vertex values. Charge a deliberately
        // outward f32 arithmetic allowance for camera-relative conversion and
        // matrix evaluation; grazing/equality cases remain eligible to draw.
        let scale = eye_local
            .into_iter()
            .fold(1.0_f64, |a, v| a.max(v.abs()))
            .max(self.min_plane.abs() as f64)
            .max(self.max_plane.abs() as f64);
        let margin = tolerance_m + 64. * f32::EPSILON as f64 * scale;
        if self.axis_sign & 4 != 0 {
            eye_local[axis] > self.max_plane as f64 + margin
        } else {
            eye_local[axis] < self.min_plane as f64 - margin
        }
    }
}
/// Counts an admitted allocation once, until its final Arc owner disappears.
/// This avoids retaining/scanning Weak records on every cache admission.
pub(crate) struct PayloadLifetime {
    total: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for PayloadLifetime {
    fn drop(&mut self) {
        self.total.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
pub struct GeometryTile {
    pub(crate) payload_lifetime: Option<PayloadLifetime>,
    pub key: TileKey,
    pub mesh: Mesh,
    pub native_quads: Vec<NativeQuad>,
    pub proxy_triangles: Vec<ProxyTriangle>,
    pub native_bins: Vec<NativeBin>,
    /// World meter bounds of actual generated vertices (not unsampled terrain).
    pub bounds: [[f64; 3]; 2],
    /// All canonical surface geometry, including vertical certificate/global fallback.
    pub conservative_surface_bounds: [[f64; 3]; 2],
    /// Source-proven solid interior; never inferred from rendered occupancy.
    pub guaranteed_solid_box: Option<Aabb>,
    /// Unique local columns touched by this build. Memoized appearance may
    /// avoid sampling columns that another build already integrated exactly.
    pub sampled_columns: usize,
    /// Sampled center residual only. NEVER use this as a certified rejection bound.
    pub empirical_error_m: f64,
    /// Exhaustive native top/side vertical envelope for tiles <=128 columns.
    pub certified_error_m: Option<f64>,
    /// Minimum eligible source certificate, cached once rather than per camera.
    pub best_certified_vertical_error_m: Option<f64>,
    pub global_error_m: Option<f64>,
    pub affine_error_m: Option<f64>,
    pub affine_height_range_m: Option<[f64; 2]>,
    pub exact_height_range_m: Option<[f64; 2]>,
    pub affine_stats: AffineStats,
    pub appearance: FaceAppearance,
    /// Unique patch profiles: +Y,+X,-X,+Z,-Z; entry zero is the exact-face fallback.
    pub face_profiles: Vec<[[f32; 4]; 5]>,
    /// When present, face_profiles contains addresses only, not computed colors.
    pub gpu_profiles: Option<DeferredProfiles>,
    /// Per-vertex profile table address; index zero preserves exact face shading.
    pub face_profile_indices: Vec<u32>,
    pub exact: bool,
    pub bytes: usize,
}

impl GeometryTile {
    pub fn triangle_count(&self) -> u64 {
        if !self.proxy_triangles.is_empty() {
            self.proxy_triangles.len() as u64
        } else if self.native_quads.is_empty() {
            self.mesh.indices.len() as u64 / 3
        } else {
            self.native_quads.len() as u64 * 2
        }
    }
    pub fn is_empty(&self) -> bool {
        self.native_quads.is_empty()
            && self.proxy_triangles.is_empty()
            && self.mesh.indices.is_empty()
    }
}
#[derive(Clone, Copy, Default, Debug)]
pub struct AffineStats {
    pub quad_bounds: u64,
    pub unavailable_quads: u64,
    pub noise_cells: u64,
    pub full_range_fallbacks: u64,
    pub cpu_ms: f64,
}
/// Small jobs are a cancellation boundary even on the exact mesher path. The
/// legacy whole-square extraction is deliberately not used here.
/// Canonical exposed area integrals: +Y, -X, +X, -Z, +Z. These do not
/// establish visibility or an appearance error bound under occlusion.
#[derive(Clone, Copy, Debug, Default)]
pub struct FaceAppearance {
    /// False when integration is GPU-only; area/color totals are unavailable.
    pub available: bool,
    pub area_m2: [f64; 5],
    pub color_area: [[f64; 3]; 5],
    pub exhaustive: bool,
}
const MAX_WIDTH: u32 = 1 << 20;
const MAX_NATIVE_WIDTH: u32 = 128;
const MAX_CERTIFICATE_WIDTH: u32 = 128;
const MAX_AFFINE_QUADS: u64 = 256;
const MAX_SAMPLES: usize = 300_000;
const MAX_VERTICES: usize = 1_000_000;

/// Heightfield tiles outside the explicit volume footprint remain valid even
/// when that separate owner has edits. The owner renders its true boundary
/// faces; the surface path must never restore an excavated wall or cave cap.
pub fn build(
    world: &World,
    key: TileKey,
    cancelled: impl FnMut() -> bool,
) -> Result<GeometryTile, String> {
    build_cached(world, key, cancelled, &mut SourceSamples::new(0))
}
fn build_cached(
    world: &World,
    key: TileKey,
    cancelled: impl FnMut() -> bool,
    samples: &mut SourceSamples,
) -> Result<GeometryTile, String> {
    samples.source_stage = SourceStage::Proxy;
    samples.begin_job();
    samples.gpu_job_allowed = integrated_profiles::eligible(world, key);
    if world.topology_base_cells().is_none() {
        let mut tile = build_surface(world, key, cancelled, samples)?;
        let finalize_started = std::time::Instant::now();
        if tile.exact {
            pack_native_quads(&mut tile)?;
        } else {
            compact_vertices(&mut tile);
        }
        configure_occlusion_bounds(world, &mut tile);
        samples.stages.finalize_ms += finalize_started.elapsed().as_secs_f64() * 1000.;
        return Ok(tile);
    }
    if key.source_revision != world.revision() {
        return Err("obsolete adaptive geometry source revision".into());
    }
    if world.edit_cells().any(|c| !world.volume_owned(c)) {
        return Err("adaptive surface edit lies outside explicit volume ownership".into());
    }
    let max = [
        key.min[0] as i128 + key.width_cells as i128,
        key.min[1] as i128 + key.width_cells as i128,
    ];
    if key.min[0] < 128 && max[0] > -128 && key.min[1] < 128 && max[1] > -128 {
        return Err(
            "adaptive surface tile intersects explicit volume footprint; split or exclude it"
                .into(),
        );
    }
    let overlap_z = key.min[1] < 128 && max[1] > -128;
    let overlap_x = key.min[0] < 128 && max[0] > -128;
    let touching = [
        key.min[0] == 128 && overlap_z,
        max[1] == -128 && overlap_x,
        max[0] == -128 && overlap_z,
        key.min[1] == 128 && overlap_x,
    ];
    if touching
        .into_iter()
        .enumerate()
        .any(|(edge, touches)| touches && key.edge_steps[edge] != 1)
    {
        return Err("explicit volume owner boundary requires native edge stride 1".into());
    }
    let terrain = world
        .fbm_terrain()
        .ok_or("adaptive topology requires FBM base")?;
    let base = World::fbm(world.seed(), terrain.params())
        .map_err(|e| format!("invalid adaptive source: {e:?}"))?;
    let mut tile = build_surface(
        &base,
        TileKey {
            source_revision: 0,
            ..key
        },
        cancelled,
        samples,
    )?;
    tile.key = key;
    let finalize_started = std::time::Instant::now();
    remove_volume_boundary_faces(&mut tile);
    disable_owner_boundary_profiles(&mut tile);
    if tile.exact {
        pack_native_quads(&mut tile)?;
    } else {
        compact_vertices(&mut tile);
    }
    configure_occlusion_bounds(world, &mut tile);
    samples.stages.finalize_ms += finalize_started.elapsed().as_secs_f64() * 1000.;
    Ok(tile)
}

/// Validate disk eligibility using the same ownership and bounded-job contracts
/// as generation. Persisted payloads cannot bypass unsupported source edits.
pub(crate) fn disk_cache_key_supported(world: &World, key: TileKey) -> bool {
    if world.fbm_terrain().is_none()
        || key.source_revision != world.revision()
        || key.width_cells == 0
        || key.width_cells > MAX_WIDTH
        || key.step_cells == 0
        || key.step_cells > key.width_cells
        || key.width_cells % key.step_cells != 0
        || key
            .edge_steps
            .iter()
            .any(|&e| e == 0 || e > key.step_cells || key.step_cells % e != 0)
        || (key.width_cells / key.step_cells) as u64 * (key.width_cells / key.step_cells) as u64
            > 65_536
        || key
            .edge_steps
            .iter()
            .map(|&e| key.width_cells as u64 / e as u64)
            .sum::<u64>()
            > 65_536
        || (key.step_cells == 1 && key.width_cells > MAX_NATIVE_WIDTH)
        || key
            .min
            .iter()
            .any(|&p| (p as i128).abs() + key.width_cells as i128 + 1 >= (1_i128 << 53))
    {
        return false;
    }
    if world.topology_base_cells().is_none() {
        return world.revision() == 0;
    }
    if world.edit_cells().any(|c| !world.volume_owned(c)) {
        return false;
    }
    let max = [
        key.min[0] as i128 + key.width_cells as i128,
        key.min[1] as i128 + key.width_cells as i128,
    ];
    let overlap_x = key.min[0] < 128 && max[0] > -128;
    let overlap_z = key.min[1] < 128 && max[1] > -128;
    if overlap_x && overlap_z {
        return false;
    }
    let touching = [
        key.min[0] == 128 && overlap_z,
        max[1] == -128 && overlap_x,
        max[0] == -128 && overlap_z,
        key.min[1] == 128 && overlap_x,
    ];
    !touching
        .into_iter()
        .enumerate()
        .any(|(edge, touches)| touches && key.edge_steps[edge] != 1)
}

fn configure_occlusion_bounds(world: &World, tile: &mut GeometryTile) {
    let Some(terrain) = world.fbm_terrain() else {
        return;
    };
    let envelope = frontier_world::summary::fbm_height_envelope(terrain);
    tile.global_error_m = if tile.exact {
        None
    } else {
        frontier_world::summary::conservative_grid_error_at(
            terrain,
            tile.key.min,
            tile.key.width_cells,
            tile.key.step_cells,
        )
        .meters()
    };
    tile.best_certified_vertical_error_m = [
        tile.certified_error_m,
        tile.affine_error_m,
        tile.global_error_m,
    ]
    .into_iter()
    .flatten()
    .filter(|e| e.is_finite() && *e >= 0.)
    .reduce(f64::min);
    let mut canonical = tile.bounds;
    if let Some(e) = tile.best_certified_vertical_error_m {
        canonical[0][1] -= e;
        canonical[1][1] += e;
    } else {
        canonical[0][1] = envelope[0];
        canonical[1][1] = envelope[1];
    }
    // Intersect independently proven height intervals. Native/AA intervals also
    // include the neighbor-center halo, so exposed side endpoints stay enclosed.
    for range in [
        Some(envelope),
        tile.affine_height_range_m,
        tile.exact_height_range_m,
    ]
    .into_iter()
    .flatten()
    {
        canonical[0][1] = canonical[0][1].max(range[0]);
        canonical[1][1] = canonical[1][1].min(range[1]);
    }
    if canonical[0][1] > canonical[1][1] {
        canonical[0][1] = envelope[0];
        canonical[1][1] = envelope[1];
    }
    tile.conservative_surface_bounds = canonical;
    if tile.best_certified_vertical_error_m.is_none()
        && tile.affine_height_range_m.is_none()
        && tile.exact_height_range_m.is_none()
    {
        return;
    }
    let min = tile.key.min.map(|v| v as f64 * VOXEL_SIZE);
    let max = [
        (tile.key.min[0] + tile.key.width_cells as i64) as f64 * VOXEL_SIZE,
        (tile.key.min[1] + tile.key.width_cells as i64) as f64 * VOXEL_SIZE,
    ];
    if world.topology_base_cells().is_some()
        && min[0] < 8.
        && max[0] > -8.
        && min[1] < 8.
        && max[1] > -8.
    {
        return;
    }
    // Canonical lower bounds may be tighter than the rendered proxy. Cap at
    // its actual minimum, ensuring the occluder also remains below the draw.
    let mut top = envelope[0];
    if let Some(e) = tile.best_certified_vertical_error_m {
        top = top.max(tile.bounds[0][1] - e);
    }
    for range in [tile.affine_height_range_m, tile.exact_height_range_m]
        .into_iter()
        .flatten()
    {
        top = top.max(range[0]);
    }
    top = top.min(tile.bounds[0][1]);
    let solid = Aabb {
        min: [min[0], envelope[0] - VOXEL_SIZE, min[1]],
        max: [max[0], top, max[1]],
    };
    if solid.valid() && (0..3).all(|a| solid.min[a] < solid.max[a]) {
        tile.guaranteed_solid_box = Some(solid);
    }
}

/// Preserve positions, flat normals, colors and profile ownership exactly;
/// only identical vertices share an address. No smooth-normal substitution.
fn compact_vertices(tile: &mut GeometryTile) {
    if tile.mesh.vertices.len() < 1024 || tile.mesh.vertices.len() > 500_000 {
        return;
    }
    let mut table = HashMap::<[u32; 10], u32>::with_capacity(tile.mesh.vertices.len());
    let mut vertices = Vec::new();
    let mut profiles = Vec::new();
    let mut remap = Vec::with_capacity(tile.mesh.vertices.len());
    for (i, &vertex) in tile.mesh.vertices.iter().enumerate() {
        let mut key = [0; 10];
        key[..3].copy_from_slice(&vertex.position.map(f32::to_bits));
        key[3..6].copy_from_slice(&vertex.normal.map(f32::to_bits));
        key[6..9].copy_from_slice(&vertex.color.map(f32::to_bits));
        key[9] = tile.face_profile_indices.get(i).copied().unwrap_or(0);
        let index = *table.entry(key).or_insert_with(|| {
            let index = vertices.len() as u32;
            vertices.push(vertex);
            if !tile.face_profile_indices.is_empty() {
                profiles.push(key[9]);
            }
            index
        });
        remap.push(index);
    }
    if vertices.len() < tile.mesh.vertices.len() {
        vertices.shrink_to_fit();
        profiles.shrink_to_fit();
        tile.mesh.vertices = vertices;
        tile.face_profile_indices = profiles;
        for index in &mut tile.mesh.indices {
            *index = remap[*index as usize];
        }
    }
    tile.bytes = tile.mesh.vertices.capacity() * std::mem::size_of::<Vertex>()
        + tile.mesh.indices.capacity() * 4
        + tile.face_profiles.capacity() * std::mem::size_of::<[[f32; 4]; 5]>()
        + tile.face_profile_indices.capacity() * 4
        + tile
            .gpu_profiles
            .as_ref()
            .map_or(0, DeferredProfiles::payload_bytes);
}

/// Pack only beneficial flat primitives after every source certificate and
/// topology operation has completed. Direct `build` remains an expanded oracle.
/// Existing packed products (including disk hits) need no further conversion.
pub(crate) fn pack_proxy_triangles(tile: &mut GeometryTile) -> Result<(), String> {
    if tile.exact || !tile.proxy_triangles.is_empty() || tile.mesh.indices.is_empty() {
        return Ok(());
    }
    if !tile.native_quads.is_empty() || tile.mesh.indices.len() % 3 != 0 {
        return Err("invalid adaptive proxy primitive ownership".into());
    }
    let count = tile.mesh.indices.len() / 3;
    let expanded_bytes = tile.mesh.vertices.capacity() * std::mem::size_of::<Vertex>()
        + tile.mesh.indices.capacity() * 4
        + tile.face_profile_indices.capacity() * 4;
    if count.saturating_mul(std::mem::size_of::<ProxyTriangle>()) >= expanded_bytes {
        return Ok(());
    }
    // Validate before allocation. Non-flat custom geometry keeps its expanded
    // interpolation path; sharing flat values must never change its appearance.
    for indices in tile.mesh.indices.chunks_exact(3) {
        let vertices: [Vertex; 3] = [
            *tile
                .mesh
                .vertices
                .get(indices[0] as usize)
                .ok_or("proxy vertex index out of range")?,
            *tile
                .mesh
                .vertices
                .get(indices[1] as usize)
                .ok_or("proxy vertex index out of range")?,
            *tile
                .mesh
                .vertices
                .get(indices[2] as usize)
                .ok_or("proxy vertex index out of range")?,
        ];
        let profile = |i: u32| {
            tile.face_profile_indices
                .get(i as usize)
                .copied()
                .unwrap_or(0)
        };
        if vertices[1..].iter().any(|v| {
            v.normal.map(f32::to_bits) != vertices[0].normal.map(f32::to_bits)
                || v.color.map(f32::to_bits) != vertices[0].color.map(f32::to_bits)
        }) || indices[1..]
            .iter()
            .any(|&i| profile(i) != profile(indices[0]))
        {
            return Ok(());
        }
        if profile(indices[0]) as usize >= tile.face_profiles.len() {
            return Err("proxy profile index out of range".into());
        }
    }
    let mut triangles = Vec::with_capacity(count);
    for indices in tile.mesh.indices.chunks_exact(3) {
        let vertices: [Vertex; 3] =
            std::array::from_fn(|i| tile.mesh.vertices[indices[i] as usize]);
        triangles.push(ProxyTriangle {
            positions: vertices.map(|v| v.position),
            normal: vertices[0].normal,
            color: vertices[0].color,
            profile_index: tile
                .face_profile_indices
                .get(indices[0] as usize)
                .copied()
                .unwrap_or(0),
        });
    }
    tile.proxy_triangles = triangles;
    tile.mesh.vertices = vec![];
    tile.mesh.indices = vec![];
    tile.face_profile_indices = vec![];
    tile.bytes = tile.proxy_triangles.capacity() * std::mem::size_of::<ProxyTriangle>()
        + tile.face_profiles.capacity() * std::mem::size_of::<[[f32; 4]; 5]>()
        + tile
            .gpu_profiles
            .as_ref()
            .map_or(0, DeferredProfiles::payload_bytes);
    Ok(())
}

fn pack_native_quads(tile: &mut GeometryTile) -> Result<(), String> {
    if tile.mesh.vertices.len() % 4 != 0 {
        return Err("native quad packing requires canonical rectangles".into());
    }
    let palette = [
        Material::Air,
        Material::Grass,
        Material::Soil,
        Material::Stone,
        Material::Wood,
    ];
    // Bound tessellation before allocating: steep native cliffs can expose
    // many more unit faces than their greedy rectangle count suggests.
    let quad_count = tile
        .mesh
        .vertices
        .chunks_exact(4)
        .try_fold(0usize, |sum, q| {
            let axis = q[0]
                .normal
                .iter()
                .position(|&v| v != 0.)
                .ok_or("native quad has no normal")?;
            let area = (0..3).filter(|&a| a != axis).try_fold(1usize, |area, a| {
                let cells = ((q[2].position[a] - q[0].position[a]) as f64 / VOXEL_SIZE).round();
                if !cells.is_finite() || cells <= 0. || cells > MAX_NATIVE_QUADS as f64 {
                    return Err("invalid native quad extent");
                }
                area.checked_mul(cells as usize)
                    .ok_or("native tessellation size overflow")
            })?;
            sum.checked_add(area)
                .filter(|&n| n <= MAX_NATIVE_QUADS)
                .ok_or("native tessellation exceeds output memory budget")
        })?;
    let mut quads = Vec::new();
    quads
        .try_reserve_exact(quad_count)
        .map_err(|_| "native tessellation allocation failed")?;
    for (i, q) in tile.mesh.vertices.chunks_exact(4).enumerate() {
        let axis = q[0]
            .normal
            .iter()
            .position(|&v| v != 0.)
            .ok_or("native quad has no normal")?;
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let material = palette
            .iter()
            .position(|m| m.color() == q[0].color)
            .ok_or("native quad color is outside canonical material palette")?
            as u32;
        let profile_index = tile.face_profile_indices.get(i * 4).copied().unwrap_or(0);
        // Greedy rectangles describe the right surface but leave T junctions
        // where a short face meets a longer edge. After perspective projection
        // and rasterizer subpixel rounding those edges can disagree, exposing
        // background samples even though the canonical surface is closed.
        // Exact near tiles use matching unit-cell edges on every face. This
        // only tessellates the same planar surfaces; materials and coverage in
        // world space remain canonical. Distant proxy geometry stays compact.
        let extent = [
            ((q[2].position[u] - q[0].position[u]) as f64 / VOXEL_SIZE).round() as usize,
            ((q[2].position[v] - q[0].position[v]) as f64 / VOXEL_SIZE).round() as usize,
        ];
        for dv in 0..extent[1] {
            for du in 0..extent[0] {
                let mut anchor = q[0].position;
                anchor[u] += (du as f64 * VOXEL_SIZE) as f32;
                anchor[v] += (dv as f64 * VOXEL_SIZE) as f32;
                quads.push(NativeQuad {
                    anchor,
                    extent: [VOXEL_SIZE as f32; 2],
                    axis_sign: axis as u32 | if q[0].normal[axis] < 0. { 4 } else { 0 },
                    material,
                    profile_index,
                });
            }
        }
    }
    let mut bins: Vec<NativeBin> = vec![];
    for (i, q) in quads.iter().enumerate() {
        let plane = q.anchor[(q.axis_sign & 3) as usize];
        if let Some(bin) = bins.last_mut().filter(|bin| bin.axis_sign == q.axis_sign) {
            bin.quad_count += 1;
            bin.min_plane = bin.min_plane.min(plane);
            bin.max_plane = bin.max_plane.max(plane);
        } else {
            bins.push(NativeBin {
                axis_sign: q.axis_sign,
                first_quad: i as u32,
                quad_count: 1,
                min_plane: plane,
                max_plane: plane,
            });
        }
    }
    tile.native_bins = bins;
    tile.native_quads = quads;
    tile.mesh.vertices = vec![];
    tile.mesh.indices = vec![];
    tile.face_profile_indices = vec![];
    tile.bytes = tile.native_quads.capacity() * std::mem::size_of::<NativeQuad>()
        + tile.native_bins.capacity() * std::mem::size_of::<NativeBin>()
        + tile.face_profiles.capacity() * std::mem::size_of::<[[f32; 4]; 5]>()
        + tile
            .gpu_profiles
            .as_ref()
            .map_or(0, DeferredProfiles::payload_bytes);
    Ok(())
}
/// The base-heightfield halo does not describe carved/edited volume faces.
/// Keep the explicit owner's adjacent surface band on unfiltered mesh shading
/// (exact for native faces, approximate for proxies)
/// instead of averaging a stale source-side profile across that ownership join.
fn disable_owner_boundary_profiles(tile: &mut GeometryTile) {
    let mut ids = HashSet::new();
    for (i, v) in tile.mesh.vertices.iter().enumerate() {
        let x = v.position[0] as f64 / VOXEL_SIZE + tile.mesh.origin.x as f64;
        let z = v.position[2] as f64 / VOXEL_SIZE + tile.mesh.origin.z as f64;
        if ((x == -128. || x == 128.) && z > -128. && z < 128.)
            || ((z == -128. || z == 128.) && x > -128. && x < 128.)
        {
            if let Some(&id) = tile.face_profile_indices.get(i) {
                ids.insert(id as usize);
            }
        }
    }
    if !ids.is_empty() {
        tile.appearance.exhaustive = false;
    }
    for id in ids {
        tile.face_profiles[id] = [[0.; 4]; 5];
    }
}

fn remove_native_volume_boundary_faces(tile: &mut GeometryTile) {
    let mut vertices = vec![];
    let mut indices = vec![];
    let mut profiles = vec![];
    let origin = [tile.mesh.origin.x, tile.mesh.origin.y, tile.mesh.origin.z];
    let mut face_count = 0usize;
    for (i, q) in tile.mesh.vertices.chunks_exact(4).enumerate() {
        let axis = q[0].normal.iter().position(|&v| v != 0.).unwrap();
        let plane = q[0].position[axis] as f64 / VOXEL_SIZE + origin[axis] as f64;
        let remove = if axis != 1 && (plane == -128. || plane == 128.) {
            let tangent = if axis == 0 { 2 } else { 0 };
            // Native quads were split on global 16-cell profile blocks. Since
            // owner endpoints are multiples of 16, no quad straddles a cut.
            let center = (q[0].position[tangent] as f64 + q[2].position[tangent] as f64)
                / (2. * VOXEL_SIZE)
                + origin[tangent] as f64;
            (-128.0..128.0).contains(&center)
        } else {
            false
        };
        if remove {
            continue;
        }
        let base = vertices.len() as u32;
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        face_count += (((q[2].position[u] - q[0].position[u]) as f64
            * (q[2].position[v] - q[0].position[v]) as f64)
            / (VOXEL_SIZE * VOXEL_SIZE))
            .round() as usize;
        vertices.extend_from_slice(q);
        indices.extend(
            if q[0].normal[axis] > 0. {
                [0, 1, 2, 0, 2, 3]
            } else {
                [0, 2, 1, 0, 3, 2]
            }
            .map(|n| base + n),
        );
        profiles.extend_from_slice(&tile.face_profile_indices[i * 4..i * 4 + 4]);
    }
    tile.mesh.vertices = vertices;
    tile.mesh.indices = indices;
    tile.mesh.exposed_native_faces = face_count;
    tile.face_profile_indices = profiles;
}

fn remove_volume_boundary_faces(tile: &mut GeometryTile) {
    if tile.exact {
        remove_native_volume_boundary_faces(tile);
        return;
    }
    let origin = [tile.mesh.origin.x, tile.mesh.origin.y, tile.mesh.origin.z];
    let mut vertices = vec![];
    let mut indices = vec![];
    let mut profiles = vec![];
    for tri in tile.mesh.indices.chunks_exact(3) {
        let v: [Vertex; 3] = std::array::from_fn(|i| tile.mesh.vertices[tri[i] as usize]);
        let mut owner_axis = None;
        for axis in [0, 2] {
            let global = v[0].position[axis] as f64 / VOXEL_SIZE + origin[axis] as f64;
            if (global == -128. || global == 128.)
                && v.iter().all(|p| p.position[axis] == v[0].position[axis])
            {
                owner_axis = Some(axis);
                break;
            }
        }
        let polys = if let Some(axis) = owner_axis {
            let tangent = if axis == 0 { 2 } else { 0 };
            let mut polys = vec![];
            for (plane, sign) in [(-128., -1.), (128., 1.)] {
                let plane = ((plane - origin[tangent] as f64) * VOXEL_SIZE) as f32;
                let mut out = vec![];
                let mut last = v[2];
                let mut ld = (last.position[tangent] - plane) * sign;
                for current in v {
                    let d = (current.position[tangent] - plane) * sign;
                    if (d >= 0.) != (ld >= 0.) {
                        let t = ld / (ld - d);
                        let mut vertex = last;
                        for a in 0..3 {
                            vertex.position[a] =
                                last.position[a] + (current.position[a] - last.position[a]) * t
                        }
                        out.push(vertex);
                    }
                    if d >= 0. {
                        out.push(current)
                    }
                    last = current;
                    ld = d;
                }
                if out.len() >= 3 {
                    polys.push(out)
                }
            }
            polys
        } else {
            vec![v.to_vec()]
        };
        for poly in polys {
            for i in 1..poly.len() - 1 {
                let base = vertices.len() as u32;
                vertices.extend([poly[0], poly[i], poly[i + 1]]);
                indices.extend([base, base + 1, base + 2]);
                if !tile.face_profiles.is_empty() {
                    profiles.extend([tile.face_profile_indices[tri[0] as usize]; 3]);
                }
            }
        }
    }
    tile.mesh.vertices = vertices;
    tile.mesh.indices = indices;
    tile.face_profile_indices = profiles;
    tile.bytes = tile.mesh.vertices.capacity() * std::mem::size_of::<Vertex>()
        + tile.mesh.indices.capacity() * 4
        + tile.face_profiles.capacity() * std::mem::size_of::<[[f32; 4]; 5]>()
        + tile.face_profile_indices.capacity() * 4
        + tile
            .gpu_profiles
            .as_ref()
            .map_or(0, DeferredProfiles::payload_bytes);
}

fn build_surface(
    world: &World,
    key: TileKey,
    mut cancelled: impl FnMut() -> bool,
    samples: &mut SourceSamples,
) -> Result<GeometryTile, String> {
    if world.fbm_terrain().is_none()
        || world.topology_base_cells().is_some()
        || world.revision() != 0
    {
        return Err("adaptive surface requires unedited heightfield ownership; use volume geometry for topology/edits".into());
    }
    if key.source_revision != world.revision() {
        return Err("obsolete adaptive geometry source revision".into());
    }
    if key.width_cells == 0
        || key.width_cells > MAX_WIDTH
        || key.step_cells == 0
        || key.step_cells > key.width_cells
        || key.width_cells % key.step_cells != 0
    {
        return Err("adaptive tile width/step exceeds bounded divisible job limits".into());
    }
    if key
        .edge_steps
        .iter()
        .any(|&e| e == 0 || e > key.step_cells || key.step_cells % e != 0)
        || (key.width_cells / key.step_cells) as u64 * (key.width_cells / key.step_cells) as u64
            > 65_536
    {
        return Err("adaptive edge/grid work exceeds bounded job limits".into());
    }
    if key
        .edge_steps
        .iter()
        .map(|&e| key.width_cells as u64 / e as u64)
        .sum::<u64>()
        > 65_536
    {
        return Err("adaptive perimeter work exceeds bounded job limits".into());
    }
    for p in key.min {
        if (p as i128).abs() + key.width_cells as i128 + 1 >= (1_i128 << 53) {
            return Err("adaptive source coordinates exceed canonical integer precision".into());
        }
    }
    if cancelled() {
        return Err("adaptive geometry cancelled".into());
    }
    if key.step_cells == 1 {
        if key.width_cells > MAX_NATIVE_WIDTH {
            return Err("native adaptive job exceeds 128-column cancellation quantum".into());
        }
        samples.source_stage = SourceStage::NativeMesh;
        let native_started = std::time::Instant::now();
        let out = column_mesh::build_sampled(
            world,
            Cell {
                x: key.min[0],
                y: 0,
                z: key.min[1],
            },
            [key.width_cells as usize; 2],
            |x, z| samples.column(world, [x, z]),
        )?;
        samples.stages.native_mesh_ms += native_started.elapsed().as_secs_f64() * 1000.;
        samples.source_stage = SourceStage::Proxy;
        if cancelled() {
            return Err("adaptive geometry cancelled".into());
        }
        let mut height_range = [f64::INFINITY, f64::NEG_INFINITY];
        for z in key.min[1] - 1..=key.min[1] + key.width_cells as i64 {
            for x in key.min[0] - 1..=key.min[0] + key.width_cells as i64 {
                let h = out.heights.height(x, z).unwrap() as f64 * VOXEL_SIZE;
                height_range[0] = height_range[0].min(h);
                height_range[1] = height_range[1].max(h);
            }
        }
        let appearance_started = std::time::Instant::now();
        let appearance = native_appearance(&out.mesh);
        let (mesh, profiles, profile_indices) = native_profiles(
            world,
            key,
            out.mesh,
            &out.heights,
            &mut cancelled,
            &samples.pages,
        )?;
        samples.stages.appearance_ms += appearance_started.elapsed().as_secs_f64() * 1000.;
        let mut tile = finish(key, mesh, out.heights.sampled_columns(), 0., true);
        tile.face_profiles = profiles;
        tile.face_profile_indices = profile_indices;
        tile.bytes += tile.face_profiles.capacity() * std::mem::size_of::<[[f32; 4]; 5]>()
            + tile.face_profile_indices.capacity() * 4
            + tile
                .gpu_profiles
                .as_ref()
                .map_or(0, DeferredProfiles::payload_bytes);
        tile.certified_error_m = Some(0.);
        tile.exact_height_range_m = Some(height_range);
        tile.appearance = appearance;
        return Ok(tile);
    }
    let defer_profiles = samples.gpu_profiles.enabled
        && !near_profile_shared()
        && samples.gpu_job_allowed
        && appearance_mode(world).probe_cells == 1
        && !appearance_mode(world).spatial_profiles
        && crate::appearance_method::selected() == crate::appearance_method::Method::Baseline;
    if defer_profiles {
        samples.gpu_profiles.eligible_tiles += 1;
    }
    let mut generator = Generator {
        defer_profiles,
        #[cfg(test)]
        experiment: None,
        world,
        key,
        samples,
        heights: LocalColumns::new(key.width_cells),
        mesh: Mesh {
            vertices: vec![],
            indices: vec![],
            origin: Cell {
                x: key.min[0],
                y: 0,
                z: key.min[1],
            },
            exposed_native_faces: 0,
        },
        empirical_error_m: 0.,
        affine_error: 0.,
        affine_range: [f64::INFINITY, f64::NEG_INFINITY],
        affine_complete: true,
        affine_stats: AffineStats::default(),
        face_profiles: vec![[[0.; 4]; 5]],
        spatial_nodes: HashMap::new(),
        face_profile_indices: vec![],
        appearance: FaceAppearance {
            available: true,
            exhaustive: key.width_cells <= MAX_NATIVE_WIDTH,
            ..Default::default()
        },
    };
    let width = key.width_cells as i64;
    let step = key.step_cells as i64;
    // A quad has a canonical central anchor and four edges. Shared interior
    // edges use identical anchors. Exterior edges use the full native staircase.
    for z in (0..width).step_by(step as usize) {
        if cancelled() {
            return Err("adaptive geometry cancelled".into());
        }
        for x in (0..width).step_by(step as usize) {
            generator.patch(x, z, step)?;
        }
    }
    let proxy_triangles = generator.mesh.indices.len() / 3;
    // Only the solid side owns a boundary face. This is identical to native
    // extraction and connects the different column heights on either side.
    for axis in [0, 2] {
        for positive in [false, true] {
            let edge = match (axis, positive) {
                (0, false) => 0,
                (2, true) => 1,
                (0, true) => 2,
                _ => 3,
            };
            if key.edge_steps[edge] != 1 {
                continue;
            }
            for t in 0..width {
                if t % 64 == 0 && cancelled() {
                    return Err("adaptive geometry cancelled".into());
                }
                generator.boundary_face(axis, positive, t)?;
            }
        }
    }
    generator
        .face_profile_indices
        .resize(generator.mesh.vertices.len(), 0);
    let certificate = if key.width_cells <= MAX_CERTIFICATE_WIDTH {
        let started = std::time::Instant::now();
        generator.samples.source_stage = SourceStage::Certificate;
        let result = certify(&mut generator, proxy_triangles, &mut cancelled)?;
        generator.samples.source_stage = SourceStage::Proxy;
        generator.samples.stages.certificate_ms += started.elapsed().as_secs_f64() * 1000.;
        Some(result)
    } else {
        None
    };
    let exact_height_range = if certificate.is_some() {
        let mut r = [f64::INFINITY, f64::NEG_INFINITY];
        for c in generator.heights.values() {
            let h = c.height as f64 * VOXEL_SIZE;
            r[0] = r[0].min(h);
            r[1] = r[1].max(h);
        }
        Some(r)
    } else {
        None
    };
    let affine_error = if generator.affine_complete && generator.affine_stats.quad_bounds > 0 {
        Some(generator.affine_error)
    } else {
        None
    };
    let affine_range = if generator.affine_complete && generator.affine_stats.quad_bounds > 0 {
        Some(generator.affine_range)
    } else {
        None
    };
    let gpu_profiles = if defer_profiles {
        let started = std::time::Instant::now();
        let generated_before = generator.samples.generated;
        // Geometry/certification already populated the local canonical cache.
        // Only missing halo/corner inputs query the shared revision-owned cache;
        // any misses are canonical source work, counted independently below.
        let mut snapshot_cancelled = false;
        let mut visited = 0;
        let page = crate::gpu_ordered_profiles::CachedPage::from_cache(world, key.min, |x, z| {
            visited += 1;
            if visited % 130 == 0 && cancelled() {
                snapshot_cancelled = true;
                return None;
            }
            let local = [x - key.min[0], z - key.min[1]];
            Some(
                generator
                    .heights
                    .get(&local)
                    .copied()
                    .unwrap_or_else(|| generator.samples.column(world, [x, z])),
            )
        });
        generator.samples.gpu_profiles.snapshot_generated_columns +=
            generator.samples.generated - generated_before;
        generator.samples.gpu_profiles.snapshot_ms += started.elapsed().as_secs_f64() * 1000.;
        if snapshot_cancelled {
            return Err("adaptive geometry cancelled".into());
        }
        match page {
            Ok(page) => {
                let mut jobs = Vec::with_capacity((width / step * (width / step)) as usize);
                for z in (0..width).step_by(step as usize) {
                    for x in (0..width).step_by(step as usize) {
                        jobs.push(crate::gpu_ordered_profiles::ProfileJob::new(
                            0,
                            [x as u32, z as u32],
                            [step as u32; 2],
                            1,
                        )?);
                    }
                }
                generator.samples.gpu_profiles.deferred_tiles += 1;
                generator.samples.gpu_profiles.deferred_jobs += jobs.len() as u64;
                generator.appearance = FaceAppearance::default();
                Some(DeferredProfiles {
                    page,
                    jobs,
                    source_revision: key.source_revision,
                })
            }
            Err(_) => {
                // Unsupported relief keeps the baseline painter and arithmetic.
                generator.samples.gpu_profiles.fallback_tiles += 1;
                generator.defer_profiles = false;
                let started = std::time::Instant::now();
                generator.samples.source_stage = SourceStage::Appearance;
                let mut id = 1;
                for z in (0..width).step_by(step as usize) {
                    if cancelled() {
                        return Err("adaptive geometry cancelled".into());
                    }
                    for x in (0..width).step_by(step as usize) {
                        generator.face_profiles[id] = generator.patch_profile(x, z, step)?;
                        id += 1;
                    }
                }
                generator.samples.source_stage = SourceStage::Proxy;
                generator.samples.stages.appearance_ms += started.elapsed().as_secs_f64() * 1000.;
                None
            }
        }
    } else {
        None
    };
    let affine_stats = generator.affine_stats;
    let appearance = generator.appearance;
    let mut tile = finish(
        key,
        generator.mesh,
        generator.heights.len(),
        generator.empirical_error_m,
        false,
    );
    tile.face_profiles = generator.face_profiles;
    tile.gpu_profiles = gpu_profiles;
    tile.face_profile_indices = generator.face_profile_indices;
    tile.bytes += tile.face_profiles.capacity() * std::mem::size_of::<[[f32; 4]; 5]>()
        + tile.face_profile_indices.capacity() * 4
        + tile
            .gpu_profiles
            .as_ref()
            .map_or(0, DeferredProfiles::payload_bytes);
    tile.certified_error_m = certificate;
    tile.affine_error_m = affine_error;
    tile.affine_height_range_m = affine_range;
    tile.affine_stats = affine_stats;
    tile.exact_height_range_m = exact_height_range;
    tile.appearance = appearance;
    Ok(tile)
}

fn finish(
    key: TileKey,
    mesh: Mesh,
    sampled_columns: usize,
    empirical_error_m: f64,
    exact: bool,
) -> GeometryTile {
    let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    let origin = [mesh.origin.x, mesh.origin.y, mesh.origin.z];
    for v in &mesh.vertices {
        for a in 0..3 {
            let p = origin[a] as f64 * VOXEL_SIZE + v.position[a] as f64;
            bounds[0][a] = bounds[0][a].min(p);
            bounds[1][a] = bounds[1][a].max(p);
        }
    }
    let bytes =
        mesh.vertices.capacity() * std::mem::size_of::<Vertex>() + mesh.indices.capacity() * 4;
    GeometryTile {
        payload_lifetime: None,
        key,
        mesh,
        native_quads: vec![],
        proxy_triangles: vec![],
        native_bins: vec![],
        conservative_surface_bounds: bounds,
        guaranteed_solid_box: None,
        bounds,
        sampled_columns,
        empirical_error_m,
        certified_error_m: None,
        best_certified_vertical_error_m: None,
        global_error_m: None,
        affine_error_m: None,
        affine_height_range_m: None,
        exact_height_range_m: None,
        affine_stats: AffineStats::default(),
        appearance: FaceAppearance::default(),
        face_profiles: vec![],
        gpu_profiles: None,
        face_profile_indices: vec![],
        exact,
        bytes,
    }
}

/// Preserve canonical quads while splitting only at one-meter X/Z profile
/// boundaries. Every resulting face belongs to exactly one canonical block;
/// no integer profile address is interpolated across a long greedy rectangle.
pub(super) fn native_profile_reuse_enabled() -> bool {
    static REUSE_TOP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // Reuse sampled canonical materials by default. Zero retains the direct
    // painter for same-binary A/B; neither path changes float accumulation.
    *REUSE_TOP
        .get_or_init(|| std::env::var_os("FRONTIER_NATIVE_PROFILE_REUSE").is_none_or(|v| v != "0"))
}
pub(super) fn native_summary_reuse_enabled() -> bool {
    static REUSE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REUSE
        .get_or_init(|| std::env::var_os("FRONTIER_NATIVE_SUMMARY_REUSE").is_some_and(|v| v == "1"))
}
/// Opt-in appearance-only experiment: share native one-metre profile support.
pub(crate) fn near_profile_shared() -> bool {
    static SHARED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SHARED.get_or_init(|| std::env::var("FRONTIER_NEAR_APPEARANCE").as_deref() == Ok("shared"))
}
fn native_profiles(
    world: &World,
    key: TileKey,
    mesh: Mesh,
    heights: &column_mesh::ColumnHeights,
    cancelled: &mut impl FnMut() -> bool,
    pages: &SharedSourcePages,
) -> Result<(Mesh, Vec<[[f32; 4]; 5]>, Vec<u32>), String> {
    native_profiles_with_summary_reuse(
        world,
        key,
        mesh,
        heights,
        cancelled,
        native_profile_reuse_enabled(),
        native_summary_reuse_enabled().then_some(pages),
    )
}
#[cfg(test)]
fn native_profiles_with_top_reuse(
    world: &World,
    key: TileKey,
    mesh: Mesh,
    heights: &column_mesh::ColumnHeights,
    cancelled: &mut impl FnMut() -> bool,
    reuse_top: bool,
) -> Result<(Mesh, Vec<[[f32; 4]; 5]>, Vec<u32>), String> {
    native_profiles_with_summary_reuse(world, key, mesh, heights, cancelled, reuse_top, None)
}
fn native_profiles_with_summary_reuse(
    world: &World,
    key: TileKey,
    mesh: Mesh,
    heights: &column_mesh::ColumnHeights,
    cancelled: &mut impl FnMut() -> bool,
    reuse_top: bool,
    pages: Option<&SharedSourcePages>,
) -> Result<(Mesh, Vec<[[f32; 4]; 5]>, Vec<u32>), String> {
    const BLOCK: i64 = 16;
    let end = [
        key.min[0] + key.width_cells as i64,
        key.min[1] + key.width_cells as i64,
    ];
    let terrain = world
        .fbm_terrain()
        .ok_or("native profiles require FBM terrain")?;
    let mut profiles = vec![[[0.; 4]; 5]];
    let mut ids = HashMap::new();
    let mut bz = key.min[1].div_euclid(BLOCK) * BLOCK;
    while bz < end[1] {
        if cancelled() {
            return Err("adaptive geometry cancelled".into());
        }
        let mut bx = key.min[0].div_euclid(BLOCK) * BLOCK;
        while bx < end[0] {
            // Only a complete canonical block matches the proxy footprint.
            // Never merge child float sums or cache clipped native boundaries.
            let min = [bx.max(key.min[0]), bz.max(key.min[1])];
            let max = [(bx + BLOCK).min(end[0]), (bz + BLOCK).min(end[1])];
            let profile_key = ProfileKey {
                min,
                size: BLOCK,
                stride: 1,
            };
            let shared = pages.filter(|_| {
                min == [bx, bz]
                    && max == [bx + BLOCK, bz + BLOCK]
                    && world.revision() == 0
                    && key.source_revision == 0
                    && world.topology_base_cells().is_none()
            });
            let profile = if let Some(entry) = shared.and_then(|p| p.profile(profile_key)) {
                entry.value
            } else {
                let entry = if shared.is_some() {
                    native_profile_entry::<true>(terrain, heights, profile_key, max, reuse_top)?
                } else {
                    native_profile_entry::<false>(terrain, heights, profile_key, max, reuse_top)?
                };
                if let Some(pages) = shared {
                    pages.store_profile(entry);
                }
                entry.value
            };
            ids.insert([bx, bz], profiles.len() as u32);
            profiles.push(profile);
            bx += BLOCK;
        }
        bz += BLOCK;
    }
    let mut out = Mesh {
        origin: mesh.origin,
        vertices: vec![],
        indices: vec![],
        exposed_native_faces: mesh.exposed_native_faces,
    };
    let origin = [mesh.origin.x, mesh.origin.y, mesh.origin.z];
    let mut addresses = vec![];
    for (index, quad) in mesh.vertices.chunks_exact(4).enumerate() {
        if index % 256 == 0 && cancelled() {
            return Err("adaptive geometry cancelled".into());
        }
        let axis = quad[0]
            .normal
            .iter()
            .position(|&v| v != 0.)
            .ok_or("native face has no axis")?;
        let positive = quad[0].normal[axis] > 0.;
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let p = quad[0]
            .position
            .map(|n| (n as f64 / VOXEL_SIZE).round() as i64);
        let hi = quad[2]
            .position
            .map(|n| (n as f64 / VOXEL_SIZE).round() as i64);
        let cuts = |a: usize| {
            let mut points = vec![p[a]];
            if a != 1 {
                let mut n = (origin[a] + p[a]).div_euclid(BLOCK) * BLOCK + BLOCK;
                while n < origin[a] + hi[a] {
                    points.push(n - origin[a]);
                    n += BLOCK;
                }
            }
            points.push(hi[a]);
            points
        };
        let uc = cuts(u);
        let vc = cuts(v);
        for uu in uc.windows(2) {
            for vv in vc.windows(2) {
                let mut owned = p;
                owned[u] = uu[0];
                owned[v] = vv[0];
                owned[axis] -= i64::from(positive);
                let block = [
                    (origin[0] + owned[0]).div_euclid(BLOCK) * BLOCK,
                    (origin[2] + owned[2]).div_euclid(BLOCK) * BLOCK,
                ];
                let id = *ids
                    .get(&block)
                    .ok_or("native face profile ownership outside tile")?;
                let base = out.vertices.len() as u32;
                for (du, dv) in [
                    (uu[0], vv[0]),
                    (uu[1], vv[0]),
                    (uu[1], vv[1]),
                    (uu[0], vv[1]),
                ] {
                    let mut point = p;
                    point[u] = du;
                    point[v] = dv;
                    let mut vertex = quad[0];
                    vertex.position = point.map(|n| (n as f64 * VOXEL_SIZE) as f32);
                    out.vertices.push(vertex);
                    addresses.push(id);
                }
                out.indices.extend(
                    if positive {
                        [0, 1, 2, 0, 2, 3]
                    } else {
                        [0, 2, 1, 0, 3, 2]
                    }
                    .map(|i| base + i),
                );
            }
        }
    }
    Ok((out, profiles, addresses))
}

/// Accumulate in the same z/x/axis order as the exact proxy painter, including
/// first-attaining extrema from centers and the four axial halo samples.
fn native_profile_entry<const TRACK_EXTREMA: bool>(
    terrain: &frontier_world::FbmTerrain,
    heights: &column_mesh::ColumnHeights,
    key: ProfileKey,
    max: [i64; 2],
    reuse_top: bool,
) -> Result<ProfileEntry, String> {
    let mut extreme_heights = [i64::MAX, i64::MIN];
    let mut extrema = [key.min; 2];
    let mut note = |point: [i64; 2], height: i64| {
        if height < extreme_heights[0] {
            extreme_heights[0] = height;
            extrema[0] = point;
        }
        if height > extreme_heights[1] {
            extreme_heights[1] = height;
            extrema[1] = point;
        }
    };
    let mut profile = [[0.; 4]; 5];
    for z in key.min[1]..max[1] {
        for x in key.min[0]..max[0] {
            let h = heights
                .height(x, z)
                .ok_or("missing native profile column")?;
            if TRACK_EXTREMA {
                note([x, z], h);
            }
            let top = if reuse_top {
                heights
                    .top_material(x, z)
                    .ok_or("missing native profile material")?
            } else {
                terrain.surface_material(
                    (x as f64 + 0.5) * VOXEL_SIZE,
                    (z as f64 + 0.5) * VOXEL_SIZE,
                    h as f64 * VOXEL_SIZE,
                )
            };
            let add = |profile: &mut [[f32; 4]; 5], a: usize, n: i64, m: Material| {
                for (c, v) in m.color().into_iter().enumerate() {
                    profile[a][c] += n as f32 * v;
                }
                profile[a][3] += n as f32;
            };
            add(&mut profile, 0, 1, top);
            for (a, dx, dz) in [(1, 1, 0), (2, -1, 0), (3, 0, 1), (4, 0, -1)] {
                let mut lo = heights
                    .height(x + dx, z + dz)
                    .ok_or("missing native profile halo")?;
                if TRACK_EXTREMA {
                    note([x + dx, z + dz], lo);
                }
                for hi in [h - 12, h - 1, h] {
                    if hi > lo {
                        let material = if lo == h - 1 {
                            top
                        } else if lo >= h - 12 {
                            Material::Soil
                        } else {
                            Material::Stone
                        };
                        add(&mut profile, a, hi - lo, material);
                        lo = hi;
                    }
                }
            }
        }
    }
    Ok(ProfileEntry {
        key,
        value: profile,
        extrema,
    })
}

/// Source-derived display settings. Only Alpine opts into spatial filtering by
/// default; explicit process overrides retain the comparison/experiment modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AppearanceMode {
    pub(crate) probe_cells: i64,
    pub(crate) spatial_profiles: bool,
    pub(crate) half_footprint: bool,
}
impl AppearanceMode {
    fn defaults(world: &World) -> Self {
        let alpine = world
            .fbm_terrain()
            .is_some_and(|terrain| terrain.params().recipe == frontier_world::FbmRecipe::Alpine);
        Self {
            probe_cells: if alpine { 4 } else { 1 },
            spatial_profiles: alpine,
            half_footprint: alpine,
        }
    }
}
#[derive(Clone, Copy, Default)]
struct AppearanceOverrides {
    probe_cells: Option<i64>,
    spatial_profiles: Option<bool>,
    half_footprint: Option<bool>,
}
impl AppearanceOverrides {
    fn apply(self, mut mode: AppearanceMode) -> AppearanceMode {
        mode.probe_cells = self.probe_cells.unwrap_or(mode.probe_cells);
        mode.spatial_profiles = self.spatial_profiles.unwrap_or(mode.spatial_profiles);
        mode.half_footprint = self.half_footprint.unwrap_or(mode.half_footprint);
        mode
    }
}
pub(crate) fn appearance_mode(world: &World) -> AppearanceMode {
    static OVERRIDES: std::sync::OnceLock<AppearanceOverrides> = std::sync::OnceLock::new();
    let overrides = *OVERRIDES.get_or_init(|| AppearanceOverrides {
        probe_cells: match std::env::var("FRONTIER_PROFILE_PROBE_CELLS").as_deref() {
            Ok("1") => Some(1),
            Ok("2") => Some(2),
            Ok("4") => Some(4),
            Ok("8") => Some(8),
            _ => None,
        },
        spatial_profiles: match std::env::var("FRONTIER_SPATIAL_PROFILES").as_deref() {
            Ok("1") => Some(true),
            Ok("0") => Some(false),
            _ => None,
        },
        half_footprint: match std::env::var("FRONTIER_SPATIAL_FOOTPRINT").as_deref() {
            Ok("half") => Some(true),
            Ok("full") => Some(false),
            _ => None,
        },
    });
    overrides.apply(AppearanceMode::defaults(world))
}

/// Approximate exposed area per native column, measured in native face units.
/// This suppresses quantization phase variance but is not an exact source-face
/// integral: material strata are applied to the estimated one-cell drop.
fn profile_probe_bands(height: i64, neighbor: i64, probe: i64) -> [f32; 3] {
    let drop = ((height - neighbor) as f32 / probe as f32).max(0.);
    [
        drop.min(1.),
        (drop - 1.).clamp(0., 11.),
        (drop - 12.).max(0.),
    ]
}

/// Exact immutable source payload shared with native extraction.
type NativeColumn = column_mesh::SampledColumn;
const SOURCE_SHARDS: usize = 32;

/// Spatial page sharding shares exact samples between concurrent builders. The
/// miss stays under its shard lock, so overlapping jobs never generate the same
/// admitted column twice. Each shard has a fixed share of the source budget.
/// Like canonical columns and bounds, profile entries belong to the cache's
/// single immutable World; they never cross source snapshots or cache clears.
#[derive(Clone)]
struct SharedSourcePages {
    shards: [Arc<Mutex<SourceShard>>; SOURCE_SHARDS],
}
#[derive(Clone, Copy, PartialEq)]
struct BoundKey {
    min: [i64; 2],
    size: [u32; 2],
    budget: [u32; 2],
}
#[derive(Clone, Copy)]
struct BoundEntry {
    key: BoundKey,
    value: Option<HeightBound>,
}
#[derive(Clone, Copy, PartialEq)]
struct ProfileKey {
    min: [i64; 2],
    size: i64,
    stride: i64,
}
#[derive(Clone, Copy)]
struct ProfileEntry {
    key: ProfileKey,
    value: [[f32; 4]; 5],
    /// Native samples attaining the queried height extrema. Replaying these on
    /// a hit preserves the local envelope used by small-tile certificates.
    extrema: [[i64; 2]; 2],
}
struct SourceShard {
    pages: PageCache<NativeColumn>,
    bounds: Vec<Option<BoundEntry>>,
    bound_bytes: usize,
    bound_hits: u64,
    bound_misses: u64,
    profiles: Vec<Option<ProfileEntry>>,
    profile_bytes: usize,
    profile_hits: u64,
    profile_misses: u64,
}
impl SharedSourcePages {
    fn new(bytes: usize) -> Self {
        Self {
            shards: std::array::from_fn(|i| {
                let budget = bytes / SOURCE_SHARDS + usize::from(i < bytes % SOURCE_SHARDS);
                // Direct-mapped immutable source bounds reserve at most 1/8 of
                // source memory (4 MiB globally); there is no growing index.
                let reserve = (budget / 8).min(4 * 1024 * 1024 / SOURCE_SHARDS);
                let slots = reserve.saturating_sub(64) / std::mem::size_of::<Option<BoundEntry>>();
                let bounds = vec![None; slots];
                let bound_bytes = if slots == 0 {
                    0
                } else {
                    bounds.capacity() * std::mem::size_of::<Option<BoundEntry>>() + 64
                };
                // Share the existing source reservation; no additional cache
                // budget or growing key index accompanies memoized appearance.
                let profile_slots =
                    reserve.saturating_sub(64) / std::mem::size_of::<Option<ProfileEntry>>();
                let profiles = vec![None; profile_slots];
                let profile_bytes = if profile_slots == 0 {
                    0
                } else {
                    profiles.capacity() * std::mem::size_of::<Option<ProfileEntry>>() + 64
                };
                Arc::new(Mutex::new(SourceShard {
                    pages: {
                        let bytes = budget - bound_bytes - profile_bytes;
                        if std::env::var("FRONTIER_SOURCE_COLD_CODEC").as_deref() == Ok("lz4") {
                            PageCache::with_cold(
                                bytes,
                                crate::source_page_cache::ValueCodec {
                                    bytes: 11,
                                    encode: |v: NativeColumn, b| {
                                        b[..8].copy_from_slice(&v.height.to_le_bytes());
                                        for i in 0..3 {
                                            b[8 + i] = v.bands[i] as u8;
                                        }
                                    },
                                    decode: |b| {
                                        let material = |v| match v {
                                            0 => Some(Material::Air),
                                            1 => Some(Material::Grass),
                                            2 => Some(Material::Soil),
                                            3 => Some(Material::Stone),
                                            4 => Some(Material::Wood),
                                            _ => None,
                                        };
                                        Some(NativeColumn {
                                            height: i64::from_le_bytes(b[..8].try_into().ok()?),
                                            bands: [
                                                material(b[8])?,
                                                material(b[9])?,
                                                material(b[10])?,
                                            ],
                                        })
                                    },
                                },
                            )
                        } else {
                            PageCache::new(bytes)
                        }
                    },
                    bounds,
                    bound_bytes,
                    bound_hits: 0,
                    bound_misses: 0,
                    profiles,
                    profile_bytes,
                    profile_hits: 0,
                    profile_misses: 0,
                }))
            }),
        }
    }
    fn shard(&self, p: [i64; 2]) -> usize {
        let x = p[0].div_euclid(32) as u64;
        let z = p[1].div_euclid(32) as u64;
        let mut h = x.wrapping_mul(0x9e3779b97f4a7c15) ^ z.wrapping_mul(0xbf58476d1ce4e5b9);
        h ^= h >> 32;
        h as usize % SOURCE_SHARDS
    }
    fn begin_job(&self) {
        for shard in &self.shards {
            shard.lock().unwrap().pages.begin_job();
        }
    }
    /// A single pass locks each shard once; callers need a coherent per-shard
    /// snapshot, not five independent passes through the same locks.
    fn snapshot(&self) -> CacheStats {
        let mut out = CacheStats::default();
        for shard in &self.shards {
            let shard = shard.lock().unwrap();
            let stats = shard.pages.stats();
            let cold = shard.pages.cold_stats();
            out.source_cold_enabled_shards += cold.enabled_shards;
            out.source_cold_budget_bytes += cold.budget_bytes;
            out.source_cold_resident_bytes += cold.resident_bytes;
            out.source_cold_entries += cold.entries;
            out.source_cold_retained_raw_bytes += cold.retained_raw_bytes;
            out.source_cold_hits += cold.hits;
            out.source_cold_encoded_bytes += cold.encoded_bytes;
            out.source_cold_raw_bytes += cold.raw_bytes;
            out.source_cold_encode_ms += cold.encode_ns as f64 / 1e6;
            out.source_cold_decode_ms += cold.decode_ns as f64 / 1e6;
            out.source_cache_bytes +=
                shard.pages.allocated_bytes() + shard.bound_bytes + shard.profile_bytes;
            out.source_cache_peak_bytes +=
                stats.peak_allocated_bytes + shard.bound_bytes + shard.profile_bytes;
            out.source_cache_pages += shard.pages.page_count();
            out.source_cache_columns += shard.pages.len();
            out.source_cache_promotions += stats.promotions;
            out.source_cache_page_evictions += stats.page_evictions;
            out.affine_cache_hits += shard.bound_hits;
            out.affine_cache_misses += shard.bound_misses;
            out.affine_cache_bytes += shard.bound_bytes;
            out.appearance_cache_hits += shard.profile_hits;
            out.appearance_cache_misses += shard.profile_misses;
            out.appearance_cache_bytes += shard.profile_bytes;
        }
        out
    }
    #[cfg(test)]
    fn stats(&self) -> crate::source_page_cache::Stats {
        crate::source_page_cache::Stats {
            peak_allocated_bytes: self.snapshot().source_cache_peak_bytes,
            ..Default::default()
        }
    }
    fn profile_index(key: ProfileKey, slots: usize) -> Option<usize> {
        if slots == 0 {
            return None;
        }
        let hash = (key.min[0] as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (key.min[1] as u64).rotate_left(27)
            ^ (key.size as u64).wrapping_mul(0xbf58476d1ce4e5b9)
            ^ (key.stride as u64).rotate_left(41);
        Some((hash ^ (hash >> 32)) as usize % slots)
    }
    fn profile(&self, key: ProfileKey) -> Option<ProfileEntry> {
        let mut shard = self.shards[self.shard(key.min)].lock().unwrap();
        let entry = Self::profile_index(key, shard.profiles.len())
            .and_then(|i| shard.profiles[i])
            .filter(|entry| entry.key == key);
        if entry.is_some() {
            shard.profile_hits += 1;
        } else {
            shard.profile_misses += 1;
        }
        entry
    }
    fn store_profile(&self, entry: ProfileEntry) {
        let mut shard = self.shards[self.shard(entry.key.min)].lock().unwrap();
        if let Some(i) = Self::profile_index(entry.key, shard.profiles.len()) {
            shard.profiles[i] = Some(entry);
        }
    }
    fn bound(&self, world: &World, region: ColumnBox, budget: BoundBudget) -> Option<HeightBound> {
        let key = BoundKey {
            min: region.min,
            size: region.size_cells,
            budget: [budget.max_noise_cells, budget.max_cells_per_noise],
        };
        let mut shard = self.shards[self.shard(region.min)].lock().unwrap();
        let hash = (key.min[0] as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (key.min[1] as u64).rotate_left(27)
            ^ (key.size[0] as u64).wrapping_mul(0xbf58476d1ce4e5b9)
            ^ (key.size[1] as u64).rotate_left(41)
            ^ (key.budget[0] as u64).rotate_left(17)
            ^ key.budget[1] as u64;
        let index = if shard.bounds.is_empty() {
            None
        } else {
            Some((hash ^ (hash >> 32)) as usize % shard.bounds.len())
        };
        if let Some(entry) = index
            .and_then(|i| shard.bounds[i])
            .filter(|entry| entry.key == key)
        {
            shard.bound_hits += 1;
            return entry.value;
        }
        shard.bound_misses += 1;
        // Keep the miss under only its spatial lock: overlapping builders share
        // the exact analysis, while independent shards progress concurrently.
        let value = bound_native_height(world.fbm_terrain().unwrap(), region, budget);
        if let Some(i) = index {
            shard.bounds[i] = Some(BoundEntry { key, value });
        }
        value
    }
    fn clear(&self) {
        for shard in &self.shards {
            let mut shard = shard.lock().unwrap();
            shard.pages.clear();
            shard.bounds.fill(None);
            shard.profiles.fill(None);
        }
    }
}
#[derive(Default, Clone, Copy, Debug)]
struct BuildStages {
    appearance_ms: f64,
    certificate_ms: f64,
    native_mesh_ms: f64,
    finalize_ms: f64,
}
#[derive(Clone, Copy, Default)]
enum SourceStage {
    #[default]
    Proxy,
    Appearance,
    Certificate,
    NativeMesh,
}
#[derive(Clone, Copy, Default)]
struct SourceTimings {
    lock_ms: f64,
    lookup_ms: f64,
    generate_ms: f64,
    appearance_ms: f64,
    certificate_ms: f64,
    native_mesh_ms: f64,
}
impl SourceTimings {
    fn total_ms(self) -> f64 {
        self.lock_ms + self.lookup_ms + self.generate_ms
    }
    fn merge(&mut self, other: Self) {
        self.lock_ms += other.lock_ms;
        self.lookup_ms += other.lookup_ms;
        self.generate_ms += other.generate_ms;
        self.appearance_ms += other.appearance_ms;
        self.certificate_ms += other.certificate_ms;
        self.native_mesh_ms += other.native_mesh_ms;
    }
}
struct SourceSamples {
    gpu_job_allowed: bool,
    gpu_profiles: GpuProfilePreparationStats,
    profile_enabled: bool,
    source_stage: SourceStage,
    timings: SourceTimings,
    pages: SharedSourcePages,
    hits: u64,
    generated: u64,
    stages: BuildStages,
}
impl SourceSamples {
    fn new(bytes: usize) -> Self {
        Self {
            gpu_job_allowed: false,
            gpu_profiles: GpuProfilePreparationStats {
                enabled: integrated_profiles::enabled(),
                ..Default::default()
            },
            profile_enabled: std::env::var_os("FRONTIER_PROFILE_SOURCE").is_some_and(|v| v == "1"),
            source_stage: SourceStage::default(),
            timings: SourceTimings::default(),
            pages: SharedSourcePages::new(bytes),
            hits: 0,
            generated: 0,
            stages: BuildStages::default(),
        }
    }
    fn fork(&self) -> Self {
        Self {
            gpu_job_allowed: false,
            gpu_profiles: GpuProfilePreparationStats {
                enabled: self.gpu_profiles.enabled,
                ..Default::default()
            },
            profile_enabled: self.profile_enabled,
            source_stage: SourceStage::default(),
            timings: SourceTimings::default(),
            pages: self.pages.clone(),
            hits: 0,
            generated: 0,
            stages: BuildStages::default(),
        }
    }
    fn begin_job(&mut self) {
        self.pages.begin_job();
    }
    fn column(&mut self, world: &World, p: [i64; 2]) -> NativeColumn {
        // No clock reads in the default per-column path. Detailed diagnostics
        // deliberately perturb work; never compare enabled and disabled runs.
        if self.profile_enabled {
            return self.profiled_column(world, p);
        }
        let mut page = self.pages.shards[self.pages.shard(p)].lock().unwrap();
        if let Some(c) = page.pages.get(p) {
            self.hits += 1;
            return c;
        }
        let c = NativeColumn::canonical(world, p[0], p[1]);
        self.generated += 1;
        page.pages.insert(p, c);
        c
    }
    fn profiled_column(&mut self, world: &World, p: [i64; 2]) -> NativeColumn {
        let started = std::time::Instant::now();
        let mut page = self.pages.shards[self.pages.shard(p)].lock().unwrap();
        let lock_ms = started.elapsed().as_secs_f64() * 1000.;
        let lookup_started = std::time::Instant::now();
        let cached = page.pages.get(p);
        let mut lookup_ms = lookup_started.elapsed().as_secs_f64() * 1000.;
        let mut generate_ms = 0.;
        let c = if let Some(c) = cached {
            self.hits += 1;
            c
        } else {
            let generate_started = std::time::Instant::now();
            let c = NativeColumn::canonical(world, p[0], p[1]);
            generate_ms = generate_started.elapsed().as_secs_f64() * 1000.;
            self.generated += 1;
            let insert_started = std::time::Instant::now();
            page.pages.insert(p, c);
            lookup_ms += insert_started.elapsed().as_secs_f64() * 1000.;
            c
        };
        self.timings.lock_ms += lock_ms;
        self.timings.lookup_ms += lookup_ms;
        self.timings.generate_ms += generate_ms;
        let total_ms = lock_ms + lookup_ms + generate_ms;
        match self.source_stage {
            SourceStage::Appearance => self.timings.appearance_ms += total_ms,
            SourceStage::Certificate => self.timings.certificate_ms += total_ms,
            SourceStage::NativeMesh => self.timings.native_mesh_ms += total_ms,
            SourceStage::Proxy => {}
        }
        c
    }
    fn clear(&mut self) {
        self.pages.clear();
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum SmallTileExperiment {
    AffineExactAppearance,
    AffineSampledAppearance,
    SharedNearAppearance,
}
#[cfg(test)]
thread_local! { static FORCE_SPARSE_WORKSPACE: std::cell::Cell<bool> = const {std::cell::Cell::new(false)}; }
/// Direct addressing for repeated exact near-grid profile/certificate queries.
/// Large jobs and out-of-range diagnostic samples retain sparse storage.
struct LocalColumns {
    side: usize,
    dense: Vec<Option<NativeColumn>>,
    sparse: HashMap<[i64; 2], NativeColumn>,
    count: usize,
}
impl LocalColumns {
    fn new(width: u32) -> Self {
        let side = if width <= MAX_NATIVE_WIDTH {
            width as usize + 2
        } else {
            0
        };
        #[cfg(test)]
        let side = if FORCE_SPARSE_WORKSPACE.get() {
            0
        } else {
            side
        };
        Self {
            side,
            dense: vec![None; side * side],
            sparse: HashMap::new(),
            count: 0,
        }
    }
    fn index(&self, p: &[i64; 2]) -> Option<usize> {
        let x = p[0] + 1;
        let z = p[1] + 1;
        if x >= 0 && z >= 0 && x < self.side as i64 && z < self.side as i64 {
            Some(z as usize * self.side + x as usize)
        } else {
            None
        }
    }
    fn get(&self, p: &[i64; 2]) -> Option<&NativeColumn> {
        match self.index(p) {
            Some(i) => self.dense[i].as_ref(),
            None => self.sparse.get(p),
        }
    }
    fn insert(&mut self, p: [i64; 2], v: NativeColumn) {
        let old = match self.index(&p) {
            Some(i) => self.dense[i].replace(v),
            None => self.sparse.insert(p, v),
        };
        if old.is_none() {
            self.count += 1;
        }
    }
    fn len(&self) -> usize {
        self.count
    }
    fn values(&self) -> impl Iterator<Item = &NativeColumn> {
        self.dense.iter().flatten().chain(self.sparse.values())
    }
}
impl std::ops::Index<&[i64; 2]> for LocalColumns {
    type Output = NativeColumn;
    fn index(&self, p: &[i64; 2]) -> &NativeColumn {
        self.get(p).expect("queried local column missing")
    }
}
struct Generator<'a> {
    defer_profiles: bool,
    #[cfg(test)]
    experiment: Option<SmallTileExperiment>,
    world: &'a World,
    samples: &'a mut SourceSamples,
    key: TileKey,
    heights: LocalColumns,
    mesh: Mesh,
    empirical_error_m: f64,
    affine_error: f64,
    affine_range: [f64; 2],
    affine_complete: bool,
    affine_stats: AffineStats,
    face_profiles: Vec<[[f32; 4]; 5]>,
    spatial_nodes: HashMap<(i64, i64, i64), u32>,
    face_profile_indices: Vec<u32>,
    appearance: FaceAppearance,
}
impl Generator<'_> {
    fn height(&mut self, x: i64, z: i64) -> Result<i64, String> {
        let p = [x, z];
        if let Some(column) = self.heights.get(&p) {
            return Ok(column.height);
        }
        if self.heights.len() >= MAX_SAMPLES {
            return Err("adaptive sample budget exceeded".into());
        }
        let column = self
            .samples
            .column(self.world, [self.key.min[0] + x, self.key.min[1] + z]);
        let h = column.height;
        if (h as i128).abs() > 16_777_216 {
            return Err("adaptive local vertical precision exceeded".into());
        }
        self.heights.insert(p, column);
        Ok(h)
    }
    fn material(&mut self, x: i64, z: i64, y: i64) -> Result<Material, String> {
        self.height(x, z)?;
        Ok(self.heights[&[x, z]].material(y))
    }
    fn point(&mut self, x: i64, z: i64) -> Result<[f32; 3], String> {
        let h = self.height(x, z)?;
        Ok([
            (x as f64 * VOXEL_SIZE) as f32,
            (h as f64 * VOXEL_SIZE) as f32,
            (z as f64 * VOXEL_SIZE) as f32,
        ])
    }
    fn triangle(
        &mut self,
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
        material: Material,
    ) -> Result<(), String> {
        if self.mesh.vertices.len() + 3 > MAX_VERTICES {
            return Err("adaptive mesh byte budget exceeded".into());
        }
        let cross = (glam::Vec3::from(b) - glam::Vec3::from(a))
            .cross(glam::Vec3::from(c) - glam::Vec3::from(a));
        if cross.length_squared() == 0. {
            return Ok(());
        }
        let normal = cross.normalize().to_array();
        let base = self.mesh.vertices.len() as u32;
        self.mesh.vertices.extend([a, b, c].map(|position| Vertex {
            position,
            normal,
            color: material.color(),
        }));
        self.mesh.indices.extend([base, base + 1, base + 2]);
        Ok(())
    }
    fn patch(&mut self, x: i64, z: i64, s: i64) -> Result<(), String> {
        let start_vertex = self.mesh.vertices.len();
        let start_index = self.mesh.indices.len();
        let center = self.point(x + s / 2, z + s / 2)?;
        let corners = [
            self.point(x, z)?,
            self.point(x, z + s)?,
            self.point(x + s, z + s)?,
            self.point(x + s, z)?,
        ];
        let average = corners.iter().map(|p| p[1] as f64).sum::<f64>() / 4.;
        self.empirical_error_m = self
            .empirical_error_m
            .max((center[1] as f64 - average).abs());
        let h = self.height(x + s / 2, z + s / 2)?;
        let material = self.material(x + s / 2, z + s / 2, h - 1)?;
        let width = self.key.width_cells as i64;
        // Clockwise in x/z is upward in x/y/z. At exterior edges duplicate
        // native horizontal intervals plus their height jumps deliberately.
        for edge in 0..4 {
            let outside = match edge {
                0 => x == 0,
                1 => z + s == width,
                2 => x + s == width,
                _ => z == 0,
            };
            if !outside {
                self.triangle(center, corners[edge], corners[(edge + 1) % 4], material)?;
                continue;
            }
            let stride = self.key.edge_steps[edge] as i64;
            if stride > 1 {
                for n in (0..s).step_by(stride as usize) {
                    let (ax, az, bx, bz) = match edge {
                        0 => (x, z + n, x, z + n + stride),
                        1 => (x + n, z + s, x + n + stride, z + s),
                        2 => (x + s, z + s - n, x + s, z + s - n - stride),
                        _ => (x + s - n, z, x + s - n - stride, z),
                    };
                    let a = self.point(ax, az)?;
                    let b = self.point(bx, bz)?;
                    self.triangle(center, a, b, material)?;
                }
                continue;
            }
            let mut previous = Some(corners[edge]);
            for n in 0..s {
                let (ax, az, bx, bz, cx, cz) = match edge {
                    0 => (x, z + n, x, z + n + 1, x, z + n),
                    1 => (x + n, z + s, x + n + 1, z + s, x + n, z + s - 1),
                    2 => (
                        x + s,
                        z + s - n,
                        x + s,
                        z + s - n - 1,
                        x + s - 1,
                        z + s - n - 1,
                    ),
                    _ => (x + s - n, z, x + s - n - 1, z, x + s - n - 1, z),
                };
                let height = self.height(cx, cz)? as f64 * VOXEL_SIZE;
                let a = [
                    (ax as f64 * VOXEL_SIZE) as f32,
                    height as f32,
                    (az as f64 * VOXEL_SIZE) as f32,
                ];
                let b = [
                    (bx as f64 * VOXEL_SIZE) as f32,
                    height as f32,
                    (bz as f64 * VOXEL_SIZE) as f32,
                ];
                if let Some(p) = previous {
                    self.triangle(center, p, a, material)?
                }
                self.triangle(center, a, b, material)?;
                previous = Some(b);
            }
            if let Some(p) = previous {
                self.triangle(center, p, corners[(edge + 1) % 4], material)?
            }
        }
        self.record_affine(x, z, s, start_index);
        let started = std::time::Instant::now();
        self.samples.source_stage = SourceStage::Appearance;
        let method = crate::appearance_method::selected();
        let shared_near = near_profile_shared();
        #[cfg(test)]
        let shared_near = self.experiment.map_or(shared_near, |mode| {
            mode == SmallTileExperiment::SharedNearAppearance
        });
        let near_index = if shared_near {
            self.shared_near_profile(x, z, s)?
        } else {
            None
        };
        let index = if let Some(index) = near_index {
            index
        } else if self.key.width_cells > MAX_NATIVE_WIDTH
            && s >= 16
            && method == crate::appearance_method::Method::Visibility
            && (!crate::appearance_visibility::spatial_enabled()
                || s <= crate::appearance_visibility::SPATIAL_MAX_EDGE_CELLS)
        {
            if crate::appearance_visibility::spatial_enabled() {
                self.visible_spatial_profile(x, z, s)?
            } else {
                self.visible_profile(x, z, s)?
            }
        } else if self.key.width_cells > MAX_NATIVE_WIDTH
            && s >= 16
            && method == crate::appearance_method::Method::Map
        {
            self.mapped_profile(x, z, s)?
        } else if (appearance_mode(self.world).spatial_profiles
            || method == crate::appearance_method::Method::Hierarchy)
            && self.key.width_cells > MAX_NATIVE_WIDTH
            && s >= 16
        {
            self.spatial_profile(x, z, s)?
        } else {
            let profile = if self.defer_profiles {
                [[0.; 4]; 5]
            } else {
                self.patch_profile(x, z, s)?
            };
            let index = self.face_profiles.len() as u32;
            self.face_profiles.push(profile);
            index
        };
        self.samples.source_stage = SourceStage::Proxy;
        self.samples.stages.appearance_ms += started.elapsed().as_secs_f64() * 1000.;
        self.face_profile_indices
            .resize(self.mesh.vertices.len(), index);
        self.face_profile_indices[start_vertex..].fill(index);
        Ok(())
    }
    /// Raw exact native integrals, shared by all patches inside a canonical
    /// 16-cell block. Scale metadata by patch coverage, never the stored sums.
    fn shared_near_profile(&mut self, x: i64, z: i64, s: i64) -> Result<Option<u32>, String> {
        const BLOCK: i64 = 16;
        if self.key.width_cells > MAX_NATIVE_WIDTH || !(1..=BLOCK).contains(&s) {
            return Ok(None);
        }
        let global = [self.key.min[0] + x, self.key.min[1] + z];
        let min = global.map(|v| v.div_euclid(BLOCK) * BLOCK);
        if (0..2).any(|a| global[a] + s > min[a] + BLOCK) {
            return Ok(None);
        }
        let local = [min[0] - self.key.min[0], min[1] - self.key.min[1]];
        let memo = (local[0], local[1], -16);
        let id = if let Some(&id) = self.spatial_nodes.get(&memo) {
            id
        } else {
            let key = ProfileKey {
                min,
                size: BLOCK,
                stride: 1,
            };
            let entry = if let Some(entry) = self.samples.pages.profile(key) {
                for point in entry.extrema {
                    self.height(point[0] - self.key.min[0], point[1] - self.key.min[1])?;
                }
                entry
            } else {
                let entry = self.compute_patch_profile(local[0], local[1], BLOCK, 1, key)?;
                self.samples.pages.store_profile(entry);
                entry
            };
            let id = self.face_profiles.len() as u32;
            self.face_profiles.push(entry.value);
            self.spatial_nodes.insert(memo, id);
            id
        };
        let value = self.face_profiles[id as usize];
        self.note_profile_mean(&[value], (s * s) as f64 / (BLOCK * BLOCK) as f64);
        Ok(Some(id))
    }
    /// Shared corner integrals and a half-footprint halo; geometry is unchanged.
    /// Equal-resolution neighbours share boundary data. Cross-level appearance
    /// remains approximate and must be evaluated in the image, not certified.
    fn spatial_profile(&mut self, x: i64, z: i64, s: i64) -> Result<u32, String> {
        let mut nodes = [0; 4];
        let mut mean = [[0.; 4]; 5];
        for (i, (nx, nz)) in [(x, z), (x + s, z), (x, z + s), (x + s, z + s)]
            .into_iter()
            .enumerate()
        {
            let id = if let Some(&id) = self.spatial_nodes.get(&(nx, nz, s)) {
                id
            } else {
                let footprint = if appearance_mode(self.world).half_footprint {
                    s / 2
                } else {
                    s
                };
                let min = [nx - footprint / 2, nz - footprint / 2];
                let key = ProfileKey {
                    min: [self.key.min[0] + min[0], self.key.min[1] + min[1]],
                    size: footprint,
                    stride: (footprint / 4).max(1),
                };
                let mut value = if crate::appearance_method::selected()
                    == crate::appearance_method::Method::Hierarchy
                    && footprint <= crate::appearance_hierarchy::MAX_EDGE_CELLS.min(128)
                {
                    crate::appearance_hierarchy::profile(
                        [self.key.min[0] + nx, self.key.min[1] + nz],
                        footprint,
                        &mut |gx, gz| self.appearance_column(gx, gz),
                    )?
                } else {
                    self.compute_patch_profile(min[0], min[1], footprint, key.stride, key)?
                        .value
                };
                let area_scale = (s * s) as f32 / (footprint * footprint) as f32;
                for field in value.iter_mut().flatten() {
                    *field *= area_scale;
                }
                let id = self.face_profiles.len() as u32;
                self.face_profiles.push(value);
                self.spatial_nodes.insert((nx, nz, s), id);
                id
            };
            nodes[i] = id;
            for a in 0..5 {
                for c in 0..4 {
                    mean[a][c] += self.face_profiles[id as usize][a][c] * 0.25;
                }
            }
        }
        for (p, a) in [(0, 0), (1, 2), (2, 1), (3, 4), (4, 3)] {
            self.appearance.area_m2[a] += mean[p][3] as f64 * VOXEL_SIZE * VOXEL_SIZE;
            for c in 0..3 {
                self.appearance.color_area[a][c] += mean[p][c] as f64 * VOXEL_SIZE * VOXEL_SIZE;
            }
        }
        let id = self.face_profiles.len() as u32;
        self.face_profiles.push(crate::spatial_appearance::header(
            [
                (x as f64 * VOXEL_SIZE) as f32,
                (z as f64 * VOXEL_SIZE) as f32,
            ],
            (s as f64 * VOXEL_SIZE) as f32,
            nodes,
            id,
        ));
        Ok(id)
    }
    fn appearance_column(
        &mut self,
        gx: i64,
        gz: i64,
    ) -> Result<crate::appearance_hierarchy::AppearanceColumn, String> {
        let x = gx - self.key.min[0];
        let z = gz - self.key.min[1];
        let height = self.height(x, z)?;
        let column = self.heights[&[x, z]];
        Ok(crate::appearance_hierarchy::AppearanceColumn {
            height,
            colors: [
                column.material(height - 1).color(),
                column.material(height - 2).color(),
                column.material(height - 13).color(),
            ],
        })
    }
    fn note_profile_mean(&mut self, values: &[[[f32; 4]; 5]], area_scale: f64) {
        for value in values {
            for (p, a) in [(0, 0), (1, 2), (2, 1), (3, 4), (4, 3)] {
                self.appearance.area_m2[a] +=
                    value[p][3] as f64 * area_scale * VOXEL_SIZE * VOXEL_SIZE / values.len() as f64;
                for c in 0..3 {
                    self.appearance.color_area[a][c] +=
                        value[p][c] as f64 * area_scale * VOXEL_SIZE * VOXEL_SIZE
                            / values.len() as f64;
                }
            }
        }
    }
    fn visible_profile(&mut self, x: i64, z: i64, s: i64) -> Result<u32, String> {
        let origin = [self.key.min[0] + x, self.key.min[1] + z];
        let field = crate::appearance_visibility::build_visible_appearance(origin, s, |gx, gz| {
            self.appearance_column(gx, gz)
        })?;
        self.note_profile_mean(&field.profiles, 1.);
        let base = self.face_profiles.len() as u32;
        self.face_profiles.extend(field.profiles);
        let height = self.height(x + s / 2, z + s / 2)?;
        let at = self.face_profiles.len() as u32;
        self.face_profiles
            .push(crate::spatial_appearance::visibility_header(
                [
                    (x + s / 2) as f32 * VOXEL_SIZE as f32,
                    (z + s / 2) as f32 * VOXEL_SIZE as f32,
                    height as f32 * VOXEL_SIZE as f32,
                ],
                base,
                at,
            ));
        Ok(at)
    }
    fn visible_spatial_profile(&mut self, x: i64, z: i64, s: i64) -> Result<u32, String> {
        let footprint = (s / 2).max(1);
        let mut nodes = [0; 4];
        for (i, (nx, nz)) in [(x, z), (x + s, z), (x, z + s), (x + s, z + s)]
            .into_iter()
            .enumerate()
        {
            nodes[i] = if let Some(&id) = self.spatial_nodes.get(&(nx, nz, s)) {
                id
            } else {
                let id = self.visible_profile(nx - footprint / 2, nz - footprint / 2, footprint)?;
                self.spatial_nodes.insert((nx, nz, s), id);
                id
            };
        }
        let at = self.face_profiles.len() as u32;
        self.face_profiles
            .push(crate::spatial_appearance::visibility_spatial_header(
                [x as f32 * VOXEL_SIZE as f32, z as f32 * VOXEL_SIZE as f32],
                s as f32 * VOXEL_SIZE as f32,
                nodes,
                at,
            ));
        Ok(at)
    }
    fn mapped_profile(&mut self, x: i64, z: i64, s: i64) -> Result<u32, String> {
        let origin = [self.key.min[0] + x, self.key.min[1] + z];
        let soft_only = crate::appearance_method::map_filter_soft_surfaces();
        let map = crate::appearance_map::build_map_with_config_and_predicate(
            origin,
            s,
            crate::appearance_map::map_intervals(),
            crate::appearance_map::map_prefilter(),
            |rgb| !soft_only || rgb == Material::Grass.color() || rgb == Material::Soil.color(),
            |gx, gz| self.appearance_column(gx, gz),
        )?;
        self.note_profile_mean(&map.profiles, (s * s) as f64);
        // Maps store compact row-major node references. Raw profiles are shared
        // between adjacent maps using the existing per-tile world-aligned cache.
        let mut refs = Vec::with_capacity(map.profiles.len());
        let kind = crate::appearance_map::effective_filter_cache_key(map.spacing_cells);
        for (i, value) in map.profiles.into_iter().enumerate() {
            let nx = map.origin_cells[0] + (i % map.dimension) as i64 * map.spacing_cells
                - self.key.min[0];
            let nz = map.origin_cells[1] + (i / map.dimension) as i64 * map.spacing_cells
                - self.key.min[1];
            let id = if let Some(&id) = self.spatial_nodes.get(&(nx, nz, kind)) {
                id
            } else {
                let id = self.face_profiles.len() as u32;
                self.face_profiles.push(value);
                self.spatial_nodes.insert((nx, nz, kind), id);
                id
            };
            refs.push(id);
        }
        let at = self.face_profiles.len() as u32;
        let header = crate::spatial_appearance::map_header(
            [
                (map.origin_cells[0] - self.key.min[0]) as f32 * VOXEL_SIZE as f32,
                (map.origin_cells[1] - self.key.min[1]) as f32 * VOXEL_SIZE as f32,
            ],
            map.spacing_cells as f32 * VOXEL_SIZE as f32,
            map.dimension,
            &refs,
            at,
        );
        self.face_profiles.extend(header);
        Ok(at)
    }
    fn record_affine(&mut self, x: i64, z: i64, s: i64, start_index: usize) {
        // All native columns are already enumerated at <=8m, providing a
        // tighter exact residual and exact source interval; avoid duplicate AA.
        let exact_small = self.key.width_cells <= MAX_CERTIFICATE_WIDTH;
        #[cfg(test)]
        let exact_small = exact_small && self.experiment.is_none();
        if exact_small {
            return;
        }
        if self.affine_stats.quad_bounds >= MAX_AFFINE_QUADS {
            self.affine_complete = false;
            self.affine_stats.unavailable_quads += 1;
            return;
        }
        self.affine_stats.quad_bounds += 1;
        let started = std::time::Instant::now();
        let bound = self.samples.pages.bound(
            self.world,
            ColumnBox {
                min: [self.key.min[0] + x, self.key.min[1] + z],
                size_cells: [s as u32; 2],
            },
            BoundBudget::default(),
        );
        let Some(bound) = bound else {
            self.affine_stats.cpu_ms += started.elapsed().as_secs_f64() * 1000.;
            self.affine_complete = false;
            self.affine_stats.unavailable_quads += 1;
            return;
        };
        self.affine_stats.noise_cells += bound.noise_cells as u64;
        self.affine_stats.full_range_fallbacks += bound.full_range_fallbacks as u64;
        self.affine_range[0] = self.affine_range[0].min(bound.height_range[0]);
        self.affine_range[1] = self.affine_range[1].max(bound.height_range[1]);
        for tri in self.mesh.indices[start_index..].chunks_exact(3) {
            let vertices = std::array::from_fn(|a| {
                let p = self.mesh.vertices[tri[a] as usize].position;
                [
                    self.key.min[0] as f64 * VOXEL_SIZE + p[0] as f64,
                    p[1] as f64,
                    self.key.min[1] as f64 * VOXEL_SIZE + p[2] as f64,
                ]
            });
            if let Some(e) = bound
                .triangle_error(vertices)
                .filter(|e| e.is_finite() && *e >= 0.)
            {
                self.affine_error = self.affine_error.max(e);
            } else {
                self.affine_complete = false;
            }
        }
        self.affine_stats.cpu_ms += started.elapsed().as_secs_f64() * 1000.;
    }
    fn patch_profile(&mut self, x: i64, z: i64, s: i64) -> Result<[[f32; 4]; 5], String> {
        // Full native area for bounded near tiles; larger clusters retain an
        // explicitly sampled area estimate rather than allocating a native world.
        let exact_appearance = self.key.width_cells <= MAX_NATIVE_WIDTH;
        #[cfg(test)]
        let exact_appearance = exact_appearance
            && self.experiment != Some(SmallTileExperiment::AffineSampledAppearance);
        let stride = if exact_appearance { 1 } else { (s / 4).max(1) };
        let key = ProfileKey {
            min: [self.key.min[0] + x, self.key.min[1] + z],
            size: s,
            stride,
        };
        // Experimental appearance changes bypass memoization entirely.
        #[cfg(not(test))]
        let cache_enabled = true;
        #[cfg(test)]
        let cache_enabled = self.experiment.is_none();
        // Exact native-neighbor queries remain shareable with native extraction.
        // Sampled microprobes must not publish under the probe-free ProfileKey.
        let cache_enabled = cache_enabled
            && (stride == 1
                || self.key.width_cells <= MAX_NATIVE_WIDTH
                || appearance_mode(self.world).probe_cells == 1);
        let cached = cache_enabled
            .then(|| self.samples.pages.profile(key))
            .flatten();
        let profile = if let Some(entry) = cached {
            // Reproduce the height envelope even when the profile avoids all
            // repeated source queries. The entry contains canonical extrema,
            // never inferred or averaged heights.
            for point in entry.extrema {
                self.height(point[0] - self.key.min[0], point[1] - self.key.min[1])?;
            }
            entry.value
        } else {
            let entry = self.compute_patch_profile(x, z, s, stride, key)?;
            if cache_enabled {
                self.samples.pages.store_profile(entry);
            }
            entry.value
        };
        for (profile_axis, appearance_axis) in [(0, 0), (1, 2), (2, 1), (3, 4), (4, 3)] {
            self.appearance.area_m2[appearance_axis] +=
                profile[profile_axis][3] as f64 * VOXEL_SIZE * VOXEL_SIZE;
            for c in 0..3 {
                self.appearance.color_area[appearance_axis][c] +=
                    profile[profile_axis][c] as f64 * VOXEL_SIZE * VOXEL_SIZE;
            }
        }
        Ok(profile)
    }
    fn compute_patch_profile(
        &mut self,
        x: i64,
        z: i64,
        s: i64,
        stride: i64,
        key: ProfileKey,
    ) -> Result<ProfileEntry, String> {
        static REUSE_CENTER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let reuse_center = *REUSE_CENTER.get_or_init(|| {
            std::env::var_os("FRONTIER_PROFILE_CENTER_REUSE").is_some_and(|value| value == "1")
        });
        self.compute_patch_profile_with_center_reuse(x, z, s, stride, key, reuse_center)
    }
    fn compute_patch_profile_with_center_reuse(
        &mut self,
        x: i64,
        z: i64,
        s: i64,
        stride: i64,
        key: ProfileKey,
        reuse_center: bool,
    ) -> Result<ProfileEntry, String> {
        let mut profile = [[0.; 4]; 5];
        let mut extreme_heights = [i64::MAX, i64::MIN];
        let mut extrema = [key.min; 2];
        let mut note = |point: [i64; 2], height: i64| {
            if height < extreme_heights[0] {
                extreme_heights[0] = height;
                extrema[0] = point;
            }
            if height > extreme_heights[1] {
                extreme_heights[1] = height;
                extrema[1] = point;
            }
        };
        let weight = (stride * stride) as f32;
        let probe = if self.key.width_cells > MAX_NATIVE_WIDTH && stride > 1 {
            appearance_mode(self.world).probe_cells.min(stride)
        } else {
            1
        };
        for zz in (z..z + s).step_by(stride as usize) {
            for xx in (x..x + s).step_by(stride as usize) {
                #[cfg(test)]
                let (xx, zz) = if self.experiment
                    == Some(SmallTileExperiment::AffineSampledAppearance)
                    && stride > 1
                {
                    let mut h = (self.key.min[0] + xx) as u64
                        ^ ((self.key.min[1] + zz) as u64).rotate_left(29)
                        ^ self.world.seed();
                    h = h.wrapping_mul(0x9e3779b97f4a7c15);
                    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                    (
                        xx + (h % stride as u64) as i64,
                        zz + ((h >> 32) % stride as u64) as i64,
                    )
                } else {
                    (xx, zz)
                };
                let h = self.height(xx, zz)?;
                // Canonical column material depends only on this immutable
                // center value. Keep every height query, side-run and float
                // addition in its original order; avoid re-querying the same
                // local column for each exposed material span.
                let center = reuse_center.then(|| self.heights[&[xx, zz]]);
                note([self.key.min[0] + xx, self.key.min[1] + zz], h);
                let add =
                    |profile: &mut [[f32; 4]; 5], axis: usize, cells: i64, material: Material| {
                        let a = cells as f32 * weight;
                        for (channel, c) in material.color().into_iter().enumerate() {
                            profile[axis][channel] += c * a
                        }
                        profile[axis][3] += a;
                    };
                let top_material = match center {
                    Some(column) => column.material(h - 1),
                    None => self.material(xx, zz, h - 1)?,
                };
                add(&mut profile, 0, 1, top_material);
                for (axis, dx, dz) in [(1, 1, 0), (2, -1, 0), (3, 0, 1), (4, 0, -1)] {
                    let mut lo = self.height(xx + dx * probe, zz + dz * probe)?;
                    note(
                        [
                            self.key.min[0] + xx + dx * probe,
                            self.key.min[1] + zz + dz * probe,
                        ],
                        lo,
                    );
                    if probe > 1 {
                        // Approximate native side density with a finite microprobe.
                        // Divide the height difference BEFORE applying native depth
                        // bands; the raw probe span is not an exposed native wall.
                        let bands = profile_probe_bands(h, lo, probe);
                        let column = self.heights[&[xx, zz]];
                        for (area, depth) in bands.into_iter().zip([1, 2, 13]) {
                            if area > 0. {
                                let material = column.material(h - depth);
                                let a = area * weight;
                                for (channel, c) in material.color().into_iter().enumerate() {
                                    profile[axis][channel] += c * a;
                                }
                                profile[axis][3] += a;
                            }
                        }
                        continue;
                    }
                    for hi in [h - 12, h - 1, h] {
                        if hi > lo {
                            let material = match center {
                                Some(column) => column.material(lo),
                                None => self.material(xx, zz, lo)?,
                            };
                            add(&mut profile, axis, hi - lo, material);
                            lo = hi
                        }
                    }
                }
            }
        }
        Ok(ProfileEntry {
            key,
            value: profile,
            extrema,
        })
    }
    fn boundary_face(&mut self, axis: usize, positive: bool, t: i64) -> Result<(), String> {
        let w = self.key.width_cells as i64;
        let owned = if positive { w - 1 } else { 0 };
        let (x, z) = if axis == 0 { (owned, t) } else { (t, owned) };
        let (nx, nz) = if axis == 0 {
            (if positive { w } else { -1 }, t)
        } else {
            (t, if positive { w } else { -1 })
        };
        let h = self.height(x, z)?;
        let low = self.height(nx, nz)?;
        if low >= h {
            return Ok(());
        }
        let mut lo = low;
        for hi in [h - 12, h - 1, h] {
            if hi <= lo {
                continue;
            }
            let plane = if positive { w } else { 0 };
            let mut points = if axis == 0 {
                [
                    [plane, lo, t],
                    [plane, hi, t],
                    [plane, hi, t + 1],
                    [plane, lo, t + 1],
                ]
            } else {
                [
                    [t, lo, plane],
                    [t + 1, lo, plane],
                    [t + 1, hi, plane],
                    [t, hi, plane],
                ]
            }
            .map(|p| p.map(|v| (v as f64 * VOXEL_SIZE) as f32));
            if !positive {
                points.reverse()
            }
            let m = self.material(x, z, lo)?;
            self.triangle(points[0], points[1], points[2], m)?;
            self.triangle(points[0], points[2], points[3], m)?;
            self.mesh.exposed_native_faces += (hi - lo) as usize;
            lo = hi;
        }
        Ok(())
    }
}

/// Intersect each proxy triangle with each overlapped native column. A linear
/// height difference reaches its extrema at the clipped polygon vertices. This
/// includes triangle/grid intersections, native top corners and both endpoints
/// of every native side; sparse center probes alone cannot provide this bound.
fn certify(
    g: &mut Generator<'_>,
    proxy_triangles: usize,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<f64, String> {
    let mut max_error = 0.0_f64;
    for index in 0..proxy_triangles {
        if index % 128 == 0 && cancelled() {
            return Err("adaptive geometry cancelled".into());
        }
        let tri: [glam::DVec3; 3] = std::array::from_fn(|a| {
            glam::Vec3::from(g.mesh.vertices[g.mesh.indices[index * 3 + a] as usize].position)
                .as_dvec3()
                / VOXEL_SIZE
        });
        let lo = tri
            .iter()
            .fold(glam::DVec3::splat(f64::INFINITY), |a, b| a.min(*b));
        let hi = tri
            .iter()
            .fold(glam::DVec3::splat(f64::NEG_INFINITY), |a, b| a.max(*b));
        let mut z0 = (lo.z.floor() as i64).max(0);
        if z0 == 0 && lo.z == 0. && g.key.edge_steps[3] > 1 {
            z0 = -1;
        }
        let z1 = (hi.z.floor() as i64)
            .min(g.key.width_cells as i64 - i64::from(g.key.edge_steps[1] == 1));
        for z in z0..=z1 {
            if z % 32 == 0 && cancelled() {
                return Err("adaptive geometry cancelled".into());
            }
            // Scan-convert one Z strip first. A thin radial boundary triangle
            // only visits the few columns it actually intersects, rather than
            // clipping every column of its much wider rectangular bounding box.
            let mut lower = [glam::DVec3::ZERO; 16];
            let mut strip = [glam::DVec3::ZERO; 16];
            let n = clip_axis(&tri, 2, z as f64, 1., &mut lower);
            let n = clip_axis(&lower[..n], 2, (z + 1) as f64, -1., &mut strip);
            if n == 0 {
                continue;
            }
            let mut x0 = (strip[..n]
                .iter()
                .map(|p| p.x)
                .fold(f64::INFINITY, f64::min)
                .floor() as i64)
                .max(0);
            if x0 == 0 && strip[..n].iter().any(|p| p.x == 0.) && g.key.edge_steps[0] > 1 {
                x0 = -1;
            }
            let x1 = (strip[..n]
                .iter()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max)
                .floor() as i64)
                .min(g.key.width_cells as i64 - i64::from(g.key.edge_steps[2] == 1));
            for x in x0..=x1 {
                // A diagonal outside column owns no face of this rectangle.
                // Its corner-only height is not an endpoint of an owned wall.
                if (x < 0 || x >= g.key.width_cells as i64)
                    && (z < 0 || z >= g.key.width_cells as i64)
                {
                    continue;
                }
                let mut left = [glam::DVec3::ZERO; 16];
                let mut poly = [glam::DVec3::ZERO; 16];
                let length = clip_axis(&strip[..n], 0, x as f64, 1., &mut left);
                let length = clip_axis(&left[..length], 0, (x + 1) as f64, -1., &mut poly);
                if length > 0 {
                    let height = g.height(x, z)? as f64;
                    for p in &poly[..length] {
                        max_error = max_error.max((p.y - height).abs() * VOXEL_SIZE)
                    }
                }
            }
        }
    }
    // Continuous outer edges also checked halo columns at their shared edge:
    // a high inside column can own a wall whose lower endpoint is outside.
    // Native stair edges instead emit the exact canonical wall explicitly.
    // Native vertical sides lie in the closed envelope of adjacent top columns. At fixed XZ the distance to the proxy value is maximized at
    // an endpoint, already checked in the closed clipped polygons. All proxy
    // triangles cover the footprint, so no horizontal padding is necessary.
    Ok(max_error)
}
#[inline]
fn clip_axis(
    input: &[glam::DVec3],
    axis: usize,
    plane: f64,
    sign: f64,
    out: &mut [glam::DVec3; 16],
) -> usize {
    if input.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut last = *input.last().unwrap();
    let mut last_d = (last[axis] - plane) * sign;
    for &current in input {
        let d = (current[axis] - plane) * sign;
        if (d >= 0.) != (last_d >= 0.) {
            out[count] = last + (current - last) * (last_d / (last_d - d));
            count += 1;
        }
        if d >= 0. {
            out[count] = current;
            count += 1;
        }
        last = current;
        last_d = d;
    }
    count
}

fn native_appearance(mesh: &Mesh) -> FaceAppearance {
    let mut out = FaceAppearance {
        available: true,
        exhaustive: true,
        ..Default::default()
    };
    for tri in mesh.indices.chunks_exact(3) {
        let a = &mesh.vertices[tri[0] as usize];
        let b = &mesh.vertices[tri[1] as usize];
        let c = &mesh.vertices[tri[2] as usize];
        let area = (glam::Vec3::from(b.position) - glam::Vec3::from(a.position))
            .cross(glam::Vec3::from(c.position) - glam::Vec3::from(a.position))
            .length() as f64
            / 2.;
        let axis = if a.normal[1] > 0. {
            0
        } else if a.normal[0] < 0. {
            1
        } else if a.normal[0] > 0. {
            2
        } else if a.normal[2] < 0. {
            3
        } else {
            4
        };
        out.area_m2[axis] += area;
        for channel in 0..3 {
            out.color_area[axis][channel] += area * a.color[channel] as f64;
        }
    }
    out
}

#[derive(Default, Clone, Copy, Debug)]
pub struct CacheStats {
    pub gpu_profiles: GpuProfilePreparationStats,
    pub hits: u64,
    pub builds: u64,
    pub evictions: u64,
    pub resident_bytes: usize,
    /// Conservative combined mesh/source-cache peak, including page-allocation
    /// overlap. Builder/pending output and caller-owned Arcs are separate.
    pub peak_bytes: usize,
    pub total_build_ms: f64,
    pub max_build_ms: f64,
    pub source_cache_hits: u64,
    pub source_cold_enabled_shards: usize,
    pub source_cold_budget_bytes: usize,
    pub source_cold_resident_bytes: usize,
    pub source_cold_entries: usize,
    pub source_cold_retained_raw_bytes: usize,
    pub source_cold_hits: u64,
    pub source_cold_encoded_bytes: u64,
    pub source_cold_raw_bytes: u64,
    pub source_cold_encode_ms: f64,
    pub source_cold_decode_ms: f64,
    pub generated_columns: u64,
    /// Reserved source-cache capacity, carved from the total cache budget.
    pub source_budget_bytes: usize,
    pub source_cache_bytes: usize,
    pub source_cache_peak_bytes: usize,
    pub source_cache_pages: usize,
    pub source_cache_columns: usize,
    pub source_cache_promotions: u64,
    pub source_cache_page_evictions: u64,
    pub affine_quad_bounds: u64,
    pub affine_noise_cells: u64,
    pub affine_full_range_fallbacks: u64,
    pub affine_unavailable_quads: u64,
    pub affine_ms: f64,
    /// Coarse CPU stage totals; source queries are included in their calling stage.
    pub appearance_ms: f64,
    pub certificate_ms: f64,
    pub native_mesh_ms: f64,
    pub finalize_ms: f64,
    pub disk: crate::derived_disk_cache::DiskCacheStats,
    /// Unique admitted mesh payload still owned by cache or external Arcs.
    /// Excludes builder scratch and not-yet-admitted batch results.
    pub live_tile_payload_bytes: usize,
    pub live_evicted_payload_bytes: usize,
    pub affine_cache_hits: u64,
    pub affine_cache_misses: u64,
    pub affine_cache_bytes: usize,
    pub appearance_cache_hits: u64,
    pub appearance_cache_misses: u64,
    pub appearance_cache_bytes: usize,
    pub peak_live_tile_payload_bytes: usize,
    /// Detailed source instrumentation is opt-in via FRONTIER_PROFILE_SOURCE=1.
    pub profile_enabled: bool,
    pub source_lock_ms: f64,
    pub source_lookup_ms: f64,
    pub source_generate_ms: f64,
    /// Exclusive CPU stage sums. Valid only when profile_enabled; source lock,
    /// lookup and generation are removed from their enclosing stages.
    pub appearance_exclusive_ms: f64,
    pub certificate_exclusive_ms: f64,
    pub native_mesh_exclusive_ms: f64,
    pub proxy_exclusive_ms: f64,
    pub lru_metadata_bytes: usize,
    /// Main-thread source snapshot, exclusive admission and eviction CPU time.
    /// Snapshot time covers the admission path; stats observer cost is separate.
    pub cache_snapshot_ms: f64,
    pub cache_admission_ms: f64,
    pub cache_eviction_ms: f64,
}
struct Entry {
    tile: Arc<GeometryTile>,
    previous: Option<TileKey>,
    next: Option<TileKey>,
}
// Charge added resident link and lifetime-guard storage compared with the former
// u64 stamp. Original fixed Arc/cache headers remain outside payload accounting.
const LRU_ENTRY_BYTES: usize =
    std::mem::size_of::<Entry>() - std::mem::size_of::<Arc<GeometryTile>>() - 8
        + std::mem::size_of::<Option<PayloadLifetime>>();
/// Bounded source cache. Returned Arc handles may outlive eviction; the caller
/// must include upload/active handles in its independent in-flight budget.
pub struct GeometryCache {
    world: World,
    budget: usize,
    source_budget: usize,
    mesh_budget: usize,
    oldest: Option<TileKey>,
    newest: Option<TileKey>,
    entries: HashMap<TileKey, Entry>,
    samples: SourceSamples,
    mesh_bytes: usize,
    stats: CacheStats,
    disk: Option<crate::derived_disk_writeback::AsyncDiskCache>,
    live_payload_bytes: Arc<AtomicUsize>,
}
/// Optional MiB reservation changes the source/mesh split, never the total.
/// Invalid input retains the baseline; saturation prevents multiplication overflow.
fn source_budget_for(total: usize, override_mib: Option<&str>) -> usize {
    let baseline = (total / 8).min(32 * 1024 * 1024);
    override_mib
        .and_then(|value| value.parse::<usize>().ok())
        .map(|mib| mib.saturating_mul(1024 * 1024).min(total / 2))
        .unwrap_or(baseline)
}
impl GeometryCache {
    pub fn new(world: World, budget_bytes: usize) -> Self {
        let override_mib = std::env::var("FRONTIER_SOURCE_CACHE_MIB").ok();
        let source_budget = source_budget_for(budget_bytes, override_mib.as_deref());
        Self {
            world,
            budget: budget_bytes,
            source_budget,
            mesh_budget: budget_bytes - source_budget,
            oldest: None,
            newest: None,
            entries: HashMap::new(),
            samples: SourceSamples::new(source_budget),
            mesh_bytes: 0,
            stats: CacheStats::default(),
            disk: None,
            live_payload_bytes: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub(crate) fn enable_disk_cache(
        &mut self,
        path: std::path::PathBuf,
        budget_bytes: u64,
        max_entry_bytes: usize,
    ) -> std::io::Result<()> {
        self.disk = Some(crate::derived_disk_writeback::AsyncDiskCache::new(
            path,
            budget_bytes,
            max_entry_bytes,
            &self.world,
        )?);
        Ok(())
    }
    /// Meshes cannot borrow unused source capacity: pages may grow during the
    /// next build before mesh eviction/admission runs.
    pub fn mesh_budget_bytes(&self) -> usize {
        self.mesh_budget
    }
    /// Exact mesh-cache admission charge for one owner, including resident LRU
    /// links and payload lifetime metadata. Unknown payloads still reserve this
    /// per-key charge when planning a complete coherent replacement.
    pub(crate) fn entry_resident_bytes(payload_bytes: usize) -> usize {
        payload_bytes.saturating_add(LRU_ENTRY_BYTES)
    }
    pub fn contains(&self, key: &TileKey) -> bool {
        self.entries.contains_key(key)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    fn unlink(&mut self, key: TileKey) {
        let entry = &self.entries[&key];
        let (previous, next) = (entry.previous, entry.next);
        if let Some(previous) = previous {
            self.entries.get_mut(&previous).unwrap().next = next;
        } else {
            self.oldest = next;
        }
        if let Some(next) = next {
            self.entries.get_mut(&next).unwrap().previous = previous;
        } else {
            self.newest = previous;
        }
    }
    fn link_newest(&mut self, key: TileKey) {
        if let Some(newest) = self.newest {
            self.entries.get_mut(&newest).unwrap().next = Some(key);
        } else {
            self.oldest = Some(key);
        }
        let entry = self.entries.get_mut(&key).unwrap();
        entry.previous = self.newest;
        entry.next = None;
        self.newest = Some(key);
    }
    fn lru_metadata_bytes(&self) -> usize {
        self.entries.len() * LRU_ENTRY_BYTES
    }
    pub fn get(&mut self, key: TileKey) -> Option<Arc<GeometryTile>> {
        let tile = self.entries.get(&key)?.tile.clone();
        if self.newest != Some(key) {
            self.unlink(key);
            self.link_newest(key);
        }
        self.stats.hits += 1;
        Some(tile)
    }
    pub fn get_or_build(
        &mut self,
        key: TileKey,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Arc<GeometryTile>, String> {
        if let Some(tile) = self.get(key) {
            return Ok(tile);
        }
        if let Some(tile) = self
            .disk
            .as_mut()
            .filter(|_| !self.samples.gpu_profiles.enabled)
            .and_then(|disk| disk.load(key))
        {
            return self.admit_built(key, Ok(tile), 0.);
        }
        debug_assert!(self.mesh_bytes <= self.mesh_budget);
        let started = std::time::Instant::now();
        let built = build_cached(&self.world, key, cancelled, &mut self.samples);
        let elapsed = started.elapsed().as_secs_f64() * 1000.;
        self.admit_built(key, built, elapsed)
    }
    /// A fixed, bounded batch. Scoped jobs share spatial source-cache shards;
    /// products enter the mesh cache in canonical request order after joining.
    pub fn prepare_parallel(
        &mut self,
        keys: &[TileKey],
        workers: usize,
        cancelled: impl Fn() -> bool + Sync,
    ) -> Result<Vec<Arc<GeometryTile>>, String> {
        if keys.len() > workers.clamp(1, 4) {
            return Err("adaptive build batch exceeds bounded worker count".into());
        }
        if keys.iter().copied().collect::<HashSet<_>>().len() != keys.len() {
            return Err("adaptive build batch contains duplicate keys".into());
        }
        if workers <= 1 {
            return keys
                .iter()
                .map(|&key| self.get_or_build(key, &cancelled))
                .collect();
        }
        let mut results: Vec<Option<Arc<GeometryTile>>> =
            keys.iter().map(|&key| self.get(key)).collect();
        for (i, &key) in keys.iter().enumerate() {
            if results[i].is_none() {
                if let Some(tile) = self
                    .disk
                    .as_mut()
                    .filter(|_| !self.samples.gpu_profiles.enabled)
                    .and_then(|disk| disk.load(key))
                {
                    results[i] = Some(self.admit_built(key, Ok(tile), 0.)?);
                }
            }
        }
        let missing: Vec<_> = keys
            .iter()
            .enumerate()
            .filter(|(i, _)| results[*i].is_none())
            .map(|(i, &key)| (i, key, self.samples.fork()))
            .collect();
        let world = &self.world;
        let built = std::thread::scope(|scope| {
            let jobs: Vec<_> = missing
                .into_iter()
                .map(|(i, key, mut samples)| {
                    let cancelled = &cancelled;
                    scope.spawn(move || {
                        let started = std::time::Instant::now();
                        let tile = build_cached(world, key, cancelled, &mut samples);
                        (
                            i,
                            key,
                            tile,
                            started.elapsed().as_secs_f64() * 1000.,
                            samples,
                        )
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| {
                    job.join()
                        .map_err(|_| "adaptive geometry worker panicked".to_string())
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        for (_, _, _, _, samples) in &built {
            self.samples.gpu_profiles.merge(samples.gpu_profiles);
            self.samples.hits += samples.hits;
            self.samples.generated += samples.generated;
            self.samples.timings.merge(samples.timings);
            self.samples.stages.appearance_ms += samples.stages.appearance_ms;
            self.samples.stages.certificate_ms += samples.stages.certificate_ms;
            self.samples.stages.native_mesh_ms += samples.stages.native_mesh_ms;
            self.samples.stages.finalize_ms += samples.stages.finalize_ms;
        }
        let mut failure = None;
        for (i, key, tile, elapsed, _) in built {
            match self.admit_built(key, tile, elapsed) {
                Ok(tile) => results[i] = Some(tile),
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        results
            .into_iter()
            .map(|tile| tile.ok_or_else(|| "adaptive batch omitted result".into()))
            .collect()
    }
    fn admit_built(
        &mut self,
        key: TileKey,
        built: Result<GeometryTile, String>,
        elapsed: f64,
    ) -> Result<Arc<GeometryTile>, String> {
        self.stats.total_build_ms += elapsed;
        self.stats.max_build_ms = self.stats.max_build_ms.max(elapsed);
        self.stats.builds += u64::from(elapsed > 0.);
        let snapshot_started = std::time::Instant::now();
        let source_stats = self.samples.pages.snapshot();
        self.stats.cache_snapshot_ms += snapshot_started.elapsed().as_secs_f64() * 1000.;
        let source_bytes = source_stats.source_cache_bytes;
        self.stats.appearance_ms = self.samples.stages.appearance_ms;
        self.stats.certificate_ms = self.samples.stages.certificate_ms;
        self.stats.native_mesh_ms = self.samples.stages.native_mesh_ms;
        self.stats.finalize_ms = self.samples.stages.finalize_ms;
        self.stats.source_cache_hits = self.samples.hits;
        self.stats.source_cold_enabled_shards = source_stats.source_cold_enabled_shards;
        self.stats.source_cold_budget_bytes = source_stats.source_cold_budget_bytes;
        self.stats.source_cold_resident_bytes = source_stats.source_cold_resident_bytes;
        self.stats.source_cold_entries = source_stats.source_cold_entries;
        self.stats.source_cold_retained_raw_bytes = source_stats.source_cold_retained_raw_bytes;
        self.stats.source_cold_hits = source_stats.source_cold_hits;
        self.stats.source_cold_encoded_bytes = source_stats.source_cold_encoded_bytes;
        self.stats.source_cold_raw_bytes = source_stats.source_cold_raw_bytes;
        self.stats.source_cold_encode_ms = source_stats.source_cold_encode_ms;
        self.stats.source_cold_decode_ms = source_stats.source_cold_decode_ms;
        self.stats.generated_columns = self.samples.generated;
        self.stats.source_cache_bytes = source_bytes;
        self.stats.source_cache_peak_bytes = source_stats.source_cache_peak_bytes;
        self.stats.source_cache_pages = source_stats.source_cache_pages;
        self.stats.source_cache_columns = source_stats.source_cache_columns;
        self.stats.source_cache_promotions = source_stats.source_cache_promotions;
        self.stats.source_cache_page_evictions = source_stats.source_cache_page_evictions;
        debug_assert!(source_bytes <= self.source_budget);
        debug_assert!(source_stats.source_cache_peak_bytes <= self.source_budget);
        // Mesh residency is fixed during the source build. Its reservation
        // therefore bounds combined cache memory even during page promotion.
        self.stats.peak_bytes = self.stats.peak_bytes.max(
            self.mesh_bytes + self.lru_metadata_bytes() + source_stats.source_cache_peak_bytes,
        );
        debug_assert!(self.stats.peak_bytes <= self.budget);
        self.stats.resident_bytes = self.mesh_bytes + self.lru_metadata_bytes() + source_bytes;
        let mut built = built?;
        let packing_started = std::time::Instant::now();
        pack_proxy_triangles(&mut built)?;
        if elapsed > 0. {
            let packing_ms = packing_started.elapsed().as_secs_f64() * 1000.0;
            self.samples.stages.finalize_ms += packing_ms;
            self.stats.finalize_ms = self.samples.stages.finalize_ms;
            self.stats.total_build_ms += packing_ms;
            self.stats.max_build_ms = self.stats.max_build_ms.max(elapsed + packing_ms);
        }
        self.live_payload_bytes
            .fetch_add(built.bytes, Ordering::Relaxed);
        built.payload_lifetime = Some(PayloadLifetime {
            total: self.live_payload_bytes.clone(),
            bytes: built.bytes,
        });
        let tile = Arc::new(built);
        if elapsed > 0. {
            self.stats.affine_quad_bounds += tile.affine_stats.quad_bounds;
            self.stats.affine_noise_cells += tile.affine_stats.noise_cells;
            self.stats.affine_full_range_fallbacks += tile.affine_stats.full_range_fallbacks;
            self.stats.affine_unavailable_quads += tile.affine_stats.unavailable_quads;
            self.stats.affine_ms += tile.affine_stats.cpu_ms;
        }
        let admission_started = std::time::Instant::now();
        if tile.bytes.saturating_add(LRU_ENTRY_BYTES) > self.mesh_budget {
            return Err("single adaptive tile exceeds reserved mesh cache budget".into());
        }
        let eviction_started = std::time::Instant::now();
        while self
            .mesh_bytes
            .saturating_add(self.lru_metadata_bytes())
            .saturating_add(tile.bytes)
            .saturating_add(LRU_ENTRY_BYTES)
            > self.mesh_budget
        {
            let oldest = self.oldest.ok_or("adaptive cache accounting error")?;
            self.unlink(oldest);
            let old = self.entries.remove(&oldest).unwrap();
            self.mesh_bytes -= old.tile.bytes;
            self.stats.evictions += 1;
        }
        let eviction_ms = eviction_started.elapsed().as_secs_f64() * 1000.;
        self.stats.cache_eviction_ms += eviction_ms;
        self.mesh_bytes += tile.bytes;
        self.stats.resident_bytes =
            self.mesh_bytes + self.lru_metadata_bytes() + LRU_ENTRY_BYTES + source_bytes;
        self.stats.peak_bytes = self.stats.peak_bytes.max(
            self.mesh_bytes
                + self.lru_metadata_bytes()
                + LRU_ENTRY_BYTES
                + source_stats.source_cache_peak_bytes,
        );
        debug_assert!(
            self.stats.resident_bytes <= self.budget && self.stats.peak_bytes <= self.budget
        );
        self.stats.peak_live_tile_payload_bytes = self
            .stats
            .peak_live_tile_payload_bytes
            .max(self.live_payload_bytes.load(Ordering::Relaxed));
        self.entries.insert(
            key,
            Entry {
                tile: tile.clone(),
                previous: None,
                next: None,
            },
        );
        self.link_newest(key);
        self.stats.cache_admission_ms +=
            (admission_started.elapsed().as_secs_f64() * 1000. - eviction_ms).max(0.);
        if elapsed > 0. && tile.gpu_profiles.is_none() {
            if let Some(disk) = &mut self.disk {
                disk.queue_store(tile.clone());
            }
        }
        Ok(tile)
    }
    pub fn stats(&self) -> CacheStats {
        let source = self.samples.pages.snapshot();
        let live_tile_payload_bytes = self.live_payload_bytes.load(Ordering::Relaxed);
        let live_evicted_payload_bytes = live_tile_payload_bytes.saturating_sub(self.mesh_bytes);
        let timing = self.samples.timings;
        let stages = self.samples.stages;
        let proxy_source_ms = timing.total_ms()
            - timing.appearance_ms
            - timing.certificate_ms
            - timing.native_mesh_ms;
        CacheStats {
            gpu_profiles: self.samples.gpu_profiles,
            source_budget_bytes: self.source_budget,
            profile_enabled: self.samples.profile_enabled,
            source_lock_ms: timing.lock_ms,
            source_lookup_ms: timing.lookup_ms,
            source_generate_ms: timing.generate_ms,
            appearance_exclusive_ms: (stages.appearance_ms - timing.appearance_ms).max(0.),
            certificate_exclusive_ms: (stages.certificate_ms - timing.certificate_ms).max(0.),
            native_mesh_exclusive_ms: (stages.native_mesh_ms - timing.native_mesh_ms).max(0.),
            proxy_exclusive_ms: (self.stats.total_build_ms
                - stages.appearance_ms
                - stages.certificate_ms
                - stages.native_mesh_ms
                - stages.finalize_ms
                - self.stats.affine_ms
                - proxy_source_ms)
                .max(0.),
            source_cold_enabled_shards: source.source_cold_enabled_shards,
            source_cold_budget_bytes: source.source_cold_budget_bytes,
            source_cold_resident_bytes: source.source_cold_resident_bytes,
            source_cold_entries: source.source_cold_entries,
            source_cold_retained_raw_bytes: source.source_cold_retained_raw_bytes,
            source_cold_hits: source.source_cold_hits,
            source_cold_encoded_bytes: source.source_cold_encoded_bytes,
            source_cold_raw_bytes: source.source_cold_raw_bytes,
            source_cold_encode_ms: source.source_cold_encode_ms,
            source_cold_decode_ms: source.source_cold_decode_ms,
            source_cache_bytes: source.source_cache_bytes,
            source_cache_peak_bytes: source.source_cache_peak_bytes,
            source_cache_pages: source.source_cache_pages,
            source_cache_columns: source.source_cache_columns,
            source_cache_promotions: source.source_cache_promotions,
            source_cache_page_evictions: source.source_cache_page_evictions,
            resident_bytes: self.mesh_bytes + self.lru_metadata_bytes() + source.source_cache_bytes,
            peak_bytes: self
                .stats
                .peak_bytes
                .max(self.mesh_bytes + self.lru_metadata_bytes() + source.source_cache_peak_bytes),
            live_evicted_payload_bytes,
            live_tile_payload_bytes,
            lru_metadata_bytes: self.lru_metadata_bytes(),
            affine_cache_hits: source.affine_cache_hits,
            affine_cache_misses: source.affine_cache_misses,
            affine_cache_bytes: source.affine_cache_bytes,
            appearance_cache_hits: source.appearance_cache_hits,
            appearance_cache_misses: source.appearance_cache_misses,
            appearance_cache_bytes: source.appearance_cache_bytes,
            disk: self
                .disk
                .as_ref()
                .map(|disk| disk.stats())
                .unwrap_or_default(),
            ..self.stats
        }
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.oldest = None;
        self.newest = None;
        self.samples.clear();
        self.mesh_bytes = 0;
        self.stats.source_cache_bytes = 0;
        self.stats.resident_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpine_appearance_defaults_are_scoped_and_explicit_overrides_remain_available() {
        use frontier_world::FbmRecipe;
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
            FbmRecipe::Alpine,
        ] {
            let world = World::fbm(
                7,
                frontier_world::FbmParams {
                    recipe,
                    ..Default::default()
                },
            )
            .unwrap();
            let defaults = AppearanceMode::defaults(&world);
            let alpine = recipe == FbmRecipe::Alpine;
            assert_eq!(
                defaults,
                AppearanceMode {
                    probe_cells: if alpine { 4 } else { 1 },
                    spatial_profiles: alpine,
                    half_footprint: alpine,
                }
            );
            let baseline = AppearanceOverrides {
                probe_cells: Some(1),
                spatial_profiles: Some(false),
                half_footprint: Some(false),
            }
            .apply(defaults);
            assert_eq!(
                baseline,
                AppearanceMode {
                    probe_cells: 1,
                    spatial_profiles: false,
                    half_footprint: false,
                }
            );
            assert_eq!(AppearanceOverrides::default().apply(defaults), defaults);
        }
    }

    #[test]
    fn spatial_profiles_share_equal_resolution_corner_integrals_across_patches_and_tiles() {
        fn generator<'a>(
            world: &'a World,
            samples: &'a mut SourceSamples,
            min: [i64; 2],
        ) -> Generator<'a> {
            Generator {
                defer_profiles: false,
                experiment: None,
                world,
                samples,
                key: TileKey {
                    min,
                    width_cells: 256,
                    step_cells: 16,
                    edge_steps: [16; 4],
                    source_revision: 0,
                },
                heights: LocalColumns::new(256),
                mesh: Mesh {
                    vertices: vec![],
                    indices: vec![],
                    origin: Cell::default(),
                    exposed_native_faces: 0,
                },
                empirical_error_m: 0.,
                affine_error: 0.,
                affine_range: [f64::INFINITY, f64::NEG_INFINITY],
                affine_complete: true,
                affine_stats: AffineStats::default(),
                face_profiles: vec![[[0.; 4]; 5]],
                spatial_nodes: HashMap::new(),
                face_profile_indices: vec![],
                appearance: FaceAppearance::default(),
            }
        }
        fn nodes(g: &Generator<'_>, header: u32) -> [usize; 4] {
            assert!(crate::spatial_appearance::is_header(
                &g.face_profiles[header as usize]
            ));
            g.face_profiles[header as usize][1]
                .map(|offset| (header as i64 + offset as i64) as usize)
        }

        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut samples = SourceSamples::new(0);
        let mut g = generator(&world, &mut samples, [-256, -512]);
        let left = g.spatial_profile(0, 0, 16).unwrap();
        let right = g.spatial_profile(16, 0, 16).unwrap();
        let back = g.spatial_profile(0, 16, 16).unwrap();
        let l = nodes(&g, left);
        let r = nodes(&g, right);
        let b = nodes(&g, back);
        assert_eq!([l[1], l[3]], [r[0], r[2]]);
        assert_eq!([l[2], l[3]], [b[0], b[1]]);
        assert!(g.face_profiles[l[1]][0][3] > 0.);
        assert!(crate::spatial_appearance::valid_table(&g.face_profiles));

        // A separate tile computes the same global halo without sharing IDs or
        // source-cache entries. Its boundary integrals must still match exactly.
        let mut other_samples = SourceSamples::new(0);
        let mut other = generator(&world, &mut other_samples, [-240, -512]);
        let other_header = other.spatial_profile(0, 0, 16).unwrap();
        let o = nodes(&other, other_header);
        for (a, b) in [(l[1], o[0]), (l[3], o[2]), (r[1], o[1]), (r[3], o[3])] {
            assert_eq!(g.face_profiles[a], other.face_profiles[b]);
        }
        assert!(crate::spatial_appearance::valid_table(&other.face_profiles));
        // This test establishes equal-resolution sharing only. Unequal-LOD
        // footprints use different integrals and remain an uncertified visual
        // approximation, including their material coverage and narrow features.
    }

    #[test]
    fn microprobe_planar_quantization_phases_preserve_fractional_side_density() {
        // Eight phases of a quantized plane. Probes are bounded to the same
        // 2/4/8 cells used by the runtime; no continuous height is queried.
        for numerator in [1_i64, 3, 12, 106] {
            let expected_drop = numerator as f32 / 8.;
            let expected = [
                expected_drop.min(1.),
                (expected_drop - 1.).clamp(0., 11.),
                (expected_drop - 12.).max(0.),
            ];
            for probe in [2_i64, 4, 8] {
                let mut mean = [0.; 3];
                for phase in 0..8 {
                    let h = (phase * numerator).div_euclid(8);
                    let neighbor = ((phase - probe) * numerator).div_euclid(8);
                    for (sum, area) in mean.iter_mut().zip(profile_probe_bands(h, neighbor, probe))
                    {
                        *sum += area / 8.;
                    }
                    assert_eq!(profile_probe_bands(neighbor, h, probe), [0.; 3]);
                }
                assert_eq!(mean, expected, "slope={numerator}/8, probe={probe}");
            }
        }
    }

    #[test]
    fn microprobe_applies_native_strata_after_dividing_the_probe_span() {
        // A raw 24-cell wall would contain stone. Its 8-cell finite difference
        // estimates a 3-cell native drop: one top face and two soil faces.
        assert_eq!(profile_probe_bands(100, 76, 8), [1., 2., 0.]);
        assert_eq!(profile_probe_bands(100, 47, 4), [1., 11., 1.25]);
        assert_eq!(profile_probe_bands(100, 99, 8), [0.125, 0., 0.]);
        for drop in 0..32 {
            let bands = profile_probe_bands(100, 100 - drop, 1);
            assert_eq!(bands[0], drop.min(1) as f32);
            assert_eq!(bands[1], (drop - 1).clamp(0, 11) as f32);
            assert_eq!(bands[2], (drop - 12).max(0) as f32);
        }
    }
    use frontier_world::FbmParams;
    fn key(step: u32) -> TileKey {
        TileKey {
            min: [-16, -16],
            width_cells: 32,
            step_cells: step,
            edge_steps: [1; 4],
            source_revision: 0,
        }
    }
    fn proxy_primitive_bits(tile: &GeometryTile) -> Vec<[[u32; 10]; 3]> {
        let bits = |v: Vertex, profile: u32| {
            let mut out = [0; 10];
            out[..3].copy_from_slice(&v.position.map(f32::to_bits));
            out[3..6].copy_from_slice(&v.normal.map(f32::to_bits));
            out[6..9].copy_from_slice(&v.color.map(f32::to_bits));
            out[9] = profile;
            out
        };
        if !tile.proxy_triangles.is_empty() {
            tile.proxy_triangles
                .iter()
                .map(|triangle| {
                    std::array::from_fn(|i| {
                        bits(
                            Vertex {
                                position: triangle.positions[i],
                                normal: triangle.normal,
                                color: triangle.color,
                            },
                            triangle.profile_index,
                        )
                    })
                })
                .collect()
        } else {
            tile.mesh
                .indices
                .chunks_exact(3)
                .map(|indices| {
                    std::array::from_fn(|i| {
                        bits(
                            tile.mesh.vertices[indices[i] as usize],
                            tile.face_profile_indices
                                .get(indices[i] as usize)
                                .copied()
                                .unwrap_or(0),
                        )
                    })
                })
                .collect()
        }
    }
    #[test]
    fn packed_proxy_layout_and_ordered_primitive_bits_match_expanded_oracle() {
        assert_eq!(std::mem::size_of::<ProxyTriangle>(), 64);
        let world = World::fbm(7, FbmParams::default()).unwrap();
        for (width, step, min) in [
            (32, 8, [-16, -16]),
            (128, 8, [-127, -511]),
            (512, 64, [0, -1024]),
        ] {
            let key = TileKey {
                min,
                width_cells: width,
                step_cells: step,
                edge_steps: [step; 4],
                source_revision: 0,
            };
            let direct = build(&world, key, || false).unwrap();
            let original = proxy_primitive_bits(&direct);
            let original_count = direct.triangle_count();
            let original_bytes = direct.bytes;
            let mut cache = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
            let packed = cache.get_or_build(key, || false).unwrap();
            assert_eq!(proxy_primitive_bits(&packed), original);
            assert_eq!(packed.triangle_count(), original_count);
            assert_eq!(packed.is_empty(), direct.is_empty());
            assert_eq!(packed.bounds, direct.bounds);
            assert_eq!(
                packed.conservative_surface_bounds,
                direct.conservative_surface_bounds
            );
            assert_eq!(
                packed.best_certified_vertical_error_m.map(f64::to_bits),
                direct.best_certified_vertical_error_m.map(f64::to_bits)
            );
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&packed.face_profiles),
                bytemuck::cast_slice::<_, u8>(&direct.face_profiles)
            );
            assert!(packed.bytes <= original_bytes);
            assert!(!packed.proxy_triangles.is_empty());
            assert!(
                packed.mesh.vertices.is_empty()
                    && packed.mesh.indices.is_empty()
                    && packed.face_profile_indices.is_empty()
            );
        }
    }
    #[test]
    fn packed_cache_product_reloads_without_source_build_or_writeback() {
        let path = std::env::temp_dir().join(format!(
            "frontier-packed-warm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let expected = {
            let mut cold = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
            cold.enable_disk_cache(path.clone(), 64 * 1024 * 1024, 16 * 1024 * 1024)
                .unwrap();
            let tile = cold.get_or_build(key(8), || false).unwrap();
            assert!(!tile.proxy_triangles.is_empty());
            proxy_primitive_bits(&tile)
        }; // Drop joins bounded writeback before reopening the same directory.
        {
            let mut warm = GeometryCache::new(world, 128 * 1024 * 1024);
            warm.enable_disk_cache(path.clone(), 64 * 1024 * 1024, 16 * 1024 * 1024)
                .unwrap();
            let tile = warm.get_or_build(key(8), || false).unwrap();
            assert_eq!(proxy_primitive_bits(&tile), expected);
            assert!(!tile.proxy_triangles.is_empty());
            let stats = warm.stats();
            assert_eq!(stats.builds, 0);
            assert_eq!(stats.generated_columns, 0);
            assert_eq!(stats.disk.hits, 1);
            assert_eq!(stats.disk.writes, 0);
            assert_eq!(stats.disk.writeback_queued_entries, 0);
        }
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn proxy_packing_preserves_signed_zero_and_keeps_nonflat_or_shared_fallbacks() {
        let make = || {
            let vertex = |position| Vertex {
                position,
                normal: [-0.0, 1.0, 0.0],
                color: [0.25, -0.0, 0.75],
            };
            let mesh = Mesh {
                vertices: vec![
                    vertex([-0.0, 0.0, 0.0]),
                    vertex([1.0, 0.0, 0.0]),
                    vertex([0.0, 0.0, 1.0]),
                ],
                indices: vec![2, 0, 1],
                origin: Cell {
                    x: -1000,
                    y: 0,
                    z: 17,
                },
                exposed_native_faces: 0,
            };
            let mut tile = finish(key(8), mesh, 0, 0.0, false);
            tile.face_profiles = vec![[[0.0; 4]; 5]];
            tile.face_profile_indices = vec![0; 3];
            tile
        };
        let mut tile = make();
        let original = proxy_primitive_bits(&tile);
        let origin = tile.mesh.origin;
        pack_proxy_triangles(&mut tile).unwrap();
        assert_eq!(tile.proxy_triangles.len(), 1);
        assert_eq!(proxy_primitive_bits(&tile), original);
        assert_eq!(tile.mesh.origin, origin);
        let mut nonflat = make();
        nonflat.mesh.vertices[1].normal[0] = 0.5;
        let original = proxy_primitive_bits(&nonflat);
        pack_proxy_triangles(&mut nonflat).unwrap();
        assert!(nonflat.proxy_triangles.is_empty());
        assert_eq!(proxy_primitive_bits(&nonflat), original);
        let mut shared = make();
        shared.mesh.indices = [2, 0, 1].repeat(200);
        let original = proxy_primitive_bits(&shared);
        pack_proxy_triangles(&mut shared).unwrap();
        assert!(shared.proxy_triangles.is_empty());
        assert_eq!(proxy_primitive_bits(&shared), original);
    }
    #[test]
    fn native_and_proxy_bounds_and_profile_match_canonical_faces() {
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 40.,
                ..Default::default()
            },
        )
        .unwrap();
        let native = build(&world, key(1), || false).unwrap();
        let proxy = build(&world, key(8), || false).unwrap();
        assert!(native.exact);
        assert_eq!(native.certified_error_m, Some(0.));
        assert!(proxy.appearance.exhaustive);
        assert_eq!(proxy.face_profile_indices.len(), proxy.mesh.vertices.len());
        assert!(proxy.face_profiles.len() < proxy.mesh.vertices.len() / 3);
        assert_eq!(proxy.appearance.area_m2, native.appearance.area_m2);
        let certificate = proxy.certified_error_m.unwrap();
        // Independent vertical ray/triangle hits at non-grid locations.
        for z in 0..32 {
            for x in 0..32 {
                let px = (x as f64 + 0.19) * VOXEL_SIZE;
                let pz = (z as f64 + 0.38) * VOXEL_SIZE;
                let native_h = world
                    .column(key(8).min[0] + x, key(8).min[1] + z)
                    .height_m();
                let mut found = false;
                for tri in proxy.mesh.indices.chunks_exact(3) {
                    let a =
                        glam::Vec3::from(proxy.mesh.vertices[tri[0] as usize].position).as_dvec3();
                    let b =
                        glam::Vec3::from(proxy.mesh.vertices[tri[1] as usize].position).as_dvec3();
                    let c =
                        glam::Vec3::from(proxy.mesh.vertices[tri[2] as usize].position).as_dvec3();
                    let den = (b.z - c.z) * (a.x - c.x) + (c.x - b.x) * (a.z - c.z);
                    if den.abs() < 1e-12 {
                        continue;
                    }
                    let wa = ((b.z - c.z) * (px - c.x) + (c.x - b.x) * (pz - c.z)) / den;
                    let wb = ((c.z - a.z) * (px - c.x) + (a.x - c.x) * (pz - c.z)) / den;
                    let wc = 1. - wa - wb;
                    if wa >= -1e-9 && wb >= -1e-9 && wc >= -1e-9 {
                        let h = wa * a.y + wb * b.y + wc * c.y;
                        assert!((h - native_h).abs() <= certificate + 1e-6);
                        found = true;
                    }
                }
                assert!(found, "proxy coverage hole at {x},{z}");
            }
        }
    }
    #[test]
    fn native_boundary_profile_is_independent_of_coarse_step() {
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 40.,
                ..Default::default()
            },
        )
        .unwrap();
        let intervals = |tile: &GeometryTile| {
            let mut out = std::collections::BTreeSet::new();
            let plane = tile.key.width_cells as f32 * VOXEL_SIZE as f32;
            for triangle in tile.mesh.indices.chunks_exact(3) {
                let tri: [Vertex; 3] =
                    std::array::from_fn(|a| tile.mesh.vertices[triangle[a] as usize]);
                for edge in [(0, 1), (1, 2), (2, 0)] {
                    let a = tri[edge.0].position;
                    let b = tri[edge.1].position;
                    if a[0] == plane
                        && b[0] == plane
                        && a[1] == b[1]
                        && (a[2] - b[2]).abs() == VOXEL_SIZE as f32
                    {
                        out.insert((a[1].to_bits(), a[2].min(b[2]).to_bits()));
                    }
                }
            }
            out
        };
        let a = build(&world, key(4), || false).unwrap();
        let b = build(&world, key(8), || false).unwrap();
        let sa = intervals(&a);
        let sb = intervals(&b);
        assert_eq!(sa, sb);
        assert!(sa.len() >= 32);
    }
    #[test]
    fn neighboring_two_to_one_tiles_share_continuous_edge_segments() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let left = build(
            &world,
            TileKey {
                edge_steps: [4; 4],
                ..key(8)
            },
            || false,
        )
        .unwrap();
        let right = build(
            &world,
            TileKey {
                min: [16, -16],
                edge_steps: [4; 4],
                ..key(4)
            },
            || false,
        )
        .unwrap();
        let segments = |tile: &GeometryTile, plane: f32| {
            let mut out = std::collections::BTreeSet::new();
            for triangle in tile.mesh.indices.chunks_exact(3) {
                let tri: [Vertex; 3] =
                    std::array::from_fn(|a| tile.mesh.vertices[triangle[a] as usize]);
                for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                    let a = tri[a].position;
                    let b = tri[b].position;
                    if a[0] == plane && b[0] == plane && a[2] != b[2] {
                        let mut points = [
                            (a[2].to_bits(), a[1].to_bits()),
                            (b[2].to_bits(), b[1].to_bits()),
                        ];
                        points.sort();
                        out.insert(points);
                    }
                }
            }
            out
        };
        assert_eq!(segments(&left, 2.), segments(&right, 0.));
        assert_eq!(segments(&left, 2.).len(), 8);
    }
    #[test]
    fn coarse_edge_matches_two_half_width_neighbors_without_t_junction_gap() {
        use std::collections::BTreeSet;
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let coarse = build(
            &world,
            TileKey {
                min: [-64, 0],
                width_cells: 64,
                step_cells: 16,
                edge_steps: [16, 16, 4, 16],
                source_revision: 0,
            },
            || false,
        )
        .unwrap();
        let fine: Vec<_> = [0, 32]
            .into_iter()
            .map(|z| {
                build(
                    &world,
                    TileKey {
                        min: [0, z],
                        width_cells: 32,
                        step_cells: 8,
                        edge_steps: [4, 8, 8, 8],
                        source_revision: 0,
                    },
                    || false,
                )
                .unwrap()
            })
            .collect();
        let segments = |tile: &GeometryTile| {
            let mut set = BTreeSet::new();
            for tri in tile.mesh.indices.chunks_exact(3) {
                for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                    let a = tile.mesh.vertices[tri[a] as usize].position;
                    let b = tile.mesh.vertices[tri[b] as usize].position;
                    let plane = -(tile.mesh.origin.x as f64 * VOXEL_SIZE) as f32;
                    if a[0] == plane && b[0] == plane && a[2] != b[2] {
                        let mut pair = [
                            (
                                a[1].to_bits(),
                                ((tile.mesh.origin.z as f64 * VOXEL_SIZE) as f32 + a[2]).to_bits(),
                            ),
                            (
                                b[1].to_bits(),
                                ((tile.mesh.origin.z as f64 * VOXEL_SIZE) as f32 + b[2]).to_bits(),
                            ),
                        ];
                        pair.sort();
                        set.insert(pair);
                    }
                }
            }
            set
        };
        let union: BTreeSet<_> = fine.iter().flat_map(segments).collect();
        assert_eq!(segments(&coarse), union);
        assert_eq!(union.len(), 16);
    }
    #[test]
    fn canonical_profiles_preserve_directional_radiance_sums_and_normalization() {
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 40.,
                ..Default::default()
            },
        )
        .unwrap();
        let k = key(1);
        let canonical = column_mesh::build(
            &world,
            Cell {
                x: k.min[0],
                y: 0,
                z: k.min[1],
            },
            [32; 2],
        )
        .unwrap();
        let native = build(&world, k, || false).unwrap();
        let proxy = build(&world, key(8), || false).unwrap();
        let axes = [
            glam::DVec3::Y,
            glam::DVec3::X,
            -glam::DVec3::X,
            glam::DVec3::Z,
            -glam::DVec3::Z,
        ];
        // Same fixed linear-light transfer as the independent exact reference.
        let light = |n: glam::DVec3| {
            let sun = glam::DVec3::new(-0.5, 0.8, -0.3).normalize();
            let sky = 0.5 + 0.5 * n.y;
            glam::DVec3::new(0.12, 0.105, 0.085).lerp(glam::DVec3::new(0.28, 0.33, 0.40), sky)
                + glam::DVec3::new(0.78, 0.70, 0.58) * n.dot(sun).max(0.)
        };
        for view in [
            glam::DVec3::Y,
            glam::DVec3::X,
            -glam::DVec3::X,
            glam::DVec3::new(-1., 0.01, 0.7).normalize(),
            glam::DVec3::new(0.2, 1., -0.3).normalize(),
            -glam::DVec3::Y,
        ] {
            let mut exact_sum = glam::DVec3::ZERO;
            let mut exact_area = 0.;
            for tri in canonical.mesh.indices.chunks_exact(3) {
                let a = canonical.mesh.vertices[tri[0] as usize];
                let b = canonical.mesh.vertices[tri[1] as usize];
                let c = canonical.mesh.vertices[tri[2] as usize];
                let normal = glam::Vec3::from(a.normal).as_dvec3();
                let area = (glam::Vec3::from(b.position) - glam::Vec3::from(a.position))
                    .cross(glam::Vec3::from(c.position) - glam::Vec3::from(a.position))
                    .length() as f64
                    / 2.;
                let weight = area * normal.dot(view).max(0.);
                exact_area += weight;
                exact_sum += glam::Vec3::from(a.color).as_dvec3() * light(normal) * weight;
            }
            for tile in [&native, &proxy] {
                let mut sum = glam::DVec3::ZERO;
                let mut area = 0.;
                let mut lower = glam::DVec3::splat(f64::INFINITY);
                let mut upper = glam::DVec3::splat(f64::NEG_INFINITY);
                for profile in &tile.face_profiles {
                    for (a, p) in profile.iter().enumerate() {
                        assert!(p.iter().all(|v| v.is_finite() && *v >= 0.));
                        let weight = axes[a].dot(view).max(0.);
                        area += weight * p[3] as f64 * VOXEL_SIZE * VOXEL_SIZE;
                        let rgb = glam::DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64);
                        sum += rgb * light(axes[a]) * weight * VOXEL_SIZE * VOXEL_SIZE;
                        if weight > 0. && p[3] > 0. {
                            let color = rgb / p[3] as f64 * light(axes[a]);
                            lower = lower.min(color);
                            upper = upper.max(color);
                        }
                    }
                }
                assert!((area - exact_area).abs() < 1e-8);
                assert!((sum - exact_sum).abs().max_element() < 2e-5 * exact_area.max(1.));
                if area > 0. {
                    let normalized = sum / area;
                    assert!(normalized.cmpge(lower - glam::DVec3::splat(1e-10)).all());
                    assert!(normalized.cmple(upper + glam::DVec3::splat(1e-10)).all());
                } else {
                    assert_eq!(sum, glam::DVec3::ZERO);
                }
            }
        }
    }
    #[test]
    fn cache_reuses_source_content_cancels_and_enforces_budget() {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let mut cache = GeometryCache::new(world.clone(), 200_000);
        let a = cache.get_or_build(key(1), || false).unwrap();
        let b = cache.get_or_build(key(1), || true).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(cache.stats().hits, 1);
        assert!(cache.stats().resident_bytes <= 200_000);
        assert!(build(&world, key(4), || true).is_err());
        assert!(build(
            &world,
            TileKey {
                source_revision: 1,
                ..key(4)
            },
            || false
        )
        .is_err());
        let mut edited = world.clone();
        edited.set(Cell::default(), Material::Stone);
        assert!(build(
            &edited,
            TileKey {
                source_revision: edited.revision(),
                ..key(4)
            },
            || false
        )
        .is_err());
        assert!(build(
            &World::fbm_topology(7, Default::default()).unwrap(),
            key(4),
            || false
        )
        .is_err());
        assert!(GeometryCache::new(world, 1)
            .get_or_build(key(1), || false)
            .is_err());
    }
    fn expanded_native(tile: &GeometryTile) -> Mesh {
        let mut mesh = Mesh {
            vertices: vec![],
            indices: vec![],
            origin: tile.mesh.origin,
            exposed_native_faces: tile.mesh.exposed_native_faces,
        };
        for q in &tile.native_quads {
            let axis = (q.axis_sign & 3) as usize;
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            let positive = q.axis_sign & 4 == 0;
            let mut normal = [0.; 3];
            normal[axis] = if positive { 1. } else { -1. };
            let color = [
                Material::Air,
                Material::Grass,
                Material::Soil,
                Material::Stone,
                Material::Wood,
            ][q.material as usize]
                .color();
            let base = mesh.vertices.len() as u32;
            for (du, dv) in [
                (0., 0.),
                (q.extent[0], 0.),
                (q.extent[0], q.extent[1]),
                (0., q.extent[1]),
            ] {
                let mut p = q.anchor;
                p[u] += du;
                p[v] += dv;
                mesh.vertices.push(Vertex {
                    position: p,
                    normal,
                    color,
                });
            }
            mesh.indices.extend(
                if positive {
                    [0, 1, 2, 0, 2, 3]
                } else {
                    [0, 2, 1, 0, 3, 2]
                }
                .map(|i| base + i),
            );
        }
        mesh
    }
    fn mesh_ray(mesh: &Mesh, o: glam::DVec3, d: glam::DVec3) -> Option<(f64, [f32; 3])> {
        let shift = glam::DVec3::new(
            mesh.origin.x as f64,
            mesh.origin.y as f64,
            mesh.origin.z as f64,
        ) * VOXEL_SIZE;
        let point =
            |i: u32| glam::Vec3::from(mesh.vertices[i as usize].position).as_dvec3() + shift;
        let mut best: Option<(f64, [f32; 3])> = None;
        for tri in mesh.indices.chunks_exact(3) {
            let a = point(tri[0]);
            let e = point(tri[1]) - a;
            let f = point(tri[2]) - a;
            let p = d.cross(f);
            let determinant = e.dot(p);
            // Match the GPU back-face culling, rather than accepting wrong winding.
            if determinant <= 1e-12 {
                continue;
            }
            let s = o - a;
            let u = s.dot(p) / determinant;
            if !(-1e-9..=1. + 1e-9).contains(&u) {
                continue;
            }
            let q = s.cross(e);
            let v = d.dot(q) / determinant;
            if v < -1e-9 || u + v > 1. + 1e-9 {
                continue;
            }
            let t = f.dot(q) / determinant;
            if t >= 0. && best.is_none_or(|b| t < b.0) {
                best = Some((t, mesh.vertices[tri[0] as usize].color));
            }
        }
        best
    }
    #[test]
    fn native_proxy_and_stair_two_to_one_wall_depths_have_no_missing_or_duplicate_owner() {
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 40.,
                ..Default::default()
            },
        )
        .unwrap();
        for left_step in [1, 8] {
            let left = build(
                &world,
                TileKey {
                    min: [-32, -16],
                    ..key(left_step)
                },
                || false,
            )
            .unwrap();
            let left_native = if left.exact {
                Some(expanded_native(&left))
            } else {
                None
            };
            let right = build(
                &world,
                TileKey {
                    min: [0, -16],
                    ..key(4)
                },
                || false,
            )
            .unwrap();
            for z in -16..16 {
                let hl = (world.column(-1, z).height_m() / VOXEL_SIZE) as i64;
                let hr = (world.column(0, z).height_m() / VOXEL_SIZE) as i64;
                if hl == hr {
                    continue;
                }
                for y in hl.min(hr)..hl.max(hr) {
                    let sign = if hl > hr { -1. } else { 1. };
                    let o = glam::DVec3::new(
                        -sign * 0.00001,
                        (y as f64 + 0.61) * VOXEL_SIZE,
                        (z as f64 + 0.37) * VOXEL_SIZE,
                    );
                    let d = glam::DVec3::new(sign, 0., 0.);
                    let a = mesh_ray(left_native.as_ref().unwrap_or(&left.mesh), o, d);
                    let b = mesh_ray(&right.mesh, o, d);
                    let boundary_hits = [a, b]
                        .into_iter()
                        .flatten()
                        .filter(|hit| (hit.0 - 0.00001).abs() < 1e-8)
                        .count();
                    assert_eq!(
                        boundary_hits, 1,
                        "native side must have exactly one owner at step{left_step} z{z} y{y}"
                    );
                }
            }
        }
    }
    #[test]
    fn exact_volume_and_proxy_join_preserves_edited_edge_depth_and_opening() {
        let mut world = World::fbm_topology(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        world.set(
            Cell {
                x: 127,
                y: -1,
                z: 0,
            },
            Material::Air,
        );
        let owner = crate::volume::build(&world).unwrap();
        for step in [1, 8] {
            let surface = build(
                &world,
                TileKey {
                    min: [128, -16],
                    source_revision: world.revision(),
                    ..key(step)
                },
                || false,
            )
            .unwrap();
            let expanded = if surface.exact {
                Some(expanded_native(&surface))
            } else {
                None
            };
            for (o, d) in [
                (
                    glam::DVec3::new(127.5 * VOXEL_SIZE, -0.5 * VOXEL_SIZE, 0.37 * VOXEL_SIZE),
                    glam::DVec3::X,
                ),
                (
                    glam::DVec3::new(127.5 * VOXEL_SIZE, 2., 0.37 * VOXEL_SIZE),
                    -glam::DVec3::Y,
                ),
                (
                    glam::DVec3::new(128. * VOXEL_SIZE + 0.000001, 2., 0.37 * VOXEL_SIZE),
                    -glam::DVec3::Y,
                ),
                (
                    glam::DVec3::new(128. * VOXEL_SIZE - 0.000001, 2., 0.37 * VOXEL_SIZE),
                    -glam::DVec3::Y,
                ),
            ] {
                let exact =
                    frontier_world::raycast(&world, o.to_array(), d.to_array(), 10.).unwrap();
                let hit = [
                    mesh_ray(&owner, o, d),
                    mesh_ray(expanded.as_ref().unwrap_or(&surface.mesh), o, d),
                ]
                .into_iter()
                .flatten()
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .expect("native/proxy seam hole");
                assert!(
                    (hit.0 - exact.distance).abs() < 1e-7,
                    "phantom cap or seam depth: {hit:?} vs {exact:?}"
                );
                assert_eq!(hit.1, exact.material.color());
            }
        }
    }
    #[test]
    fn source_samples_reuse_across_steps_and_certificates_have_no_artificial_floor() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut cache = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
        let a = cache.get_or_build(key(4), || false).unwrap();
        let generated = cache.stats().generated_columns;
        let b = cache.get_or_build(key(8), || false).unwrap();
        assert_eq!(
            generated,
            cache.stats().generated_columns,
            "refinement must reuse all already sampled native columns"
        );
        assert!(cache.stats().source_cache_hits > 0);
        let direct = build(&world, key(8), || false).unwrap();
        assert_eq!(direct.certified_error_m, b.certified_error_m);
        assert_eq!(proxy_primitive_bits(&direct), proxy_primitive_bits(&b));
        assert_eq!(direct.face_profiles, b.face_profiles);
        assert_eq!(a.appearance.area_m2, b.appearance.area_m2);
        assert!(cache.stats().resident_bytes <= 128 * 1024 * 1024);
        let flat = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            build(&flat, key(8), || false).unwrap().certified_error_m,
            Some(0.)
        );
    }
    #[test]
    #[ignore = "manual CPU profile, no timing pass threshold"]
    fn profile_bounded_native_certification() {
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 1800.,
                amplitude: 1000.,
                octaves: 8,
                roughness: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
        let mut cache = GeometryCache::new(world, 128 * 1024 * 1024);
        for step in [32, 16, 8, 4, 2, 1] {
            let key = TileKey {
                min: [0, 0],
                width_cells: 128,
                step_cells: step,
                edge_steps: [1; 4],
                source_revision: 0,
            };
            let start = std::time::Instant::now();
            let tile = cache.get_or_build(key, || false).unwrap();
            eprintln!("step={step} ms={:.3} certificate={:?} columns={} triangles={} source_generated={} source_hits={} bytes={} vertices={} profiles={}",start.elapsed().as_secs_f64()*1000.,tile.certified_error_m,tile.sampled_columns,tile.triangle_count(),cache.stats().generated_columns,cache.stats().source_cache_hits,tile.bytes,tile.mesh.vertices.len(),tile.face_profiles.len());
        }
    }
    #[test]
    fn shared_parallel_builds_match_serial_and_reuse_overlapping_source() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let keys: Vec<_> = (0..4)
            .map(|i| TileKey {
                min: [i * 128, -512],
                width_cells: 128,
                step_cells: if i == 3 { 1 } else { 8 },
                edge_steps: [1; 4],
                source_revision: 0,
            })
            .collect();
        let mut serial = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
        FORCE_SPARSE_WORKSPACE.set(true);
        let expected: Vec<_> = keys
            .iter()
            .map(|&key| serial.get_or_build(key, || false).unwrap())
            .collect();
        FORCE_SPARSE_WORKSPACE.set(false);
        let mut parallel = GeometryCache::new(world, 128 * 1024 * 1024);
        let actual = parallel.prepare_parallel(&keys, 4, || false).unwrap();
        for (a, b) in expected.iter().zip(&actual) {
            assert_eq!(a.key, b.key);
            assert_eq!(proxy_primitive_bits(a), proxy_primitive_bits(b));
            assert_eq!(a.mesh.indices, b.mesh.indices);
            assert_eq!(a.mesh.vertices.len(), b.mesh.vertices.len());
            for (a, b) in a.mesh.vertices.iter().zip(&b.mesh.vertices) {
                assert_eq!(a.position, b.position);
                assert_eq!(a.normal, b.normal);
                assert_eq!(a.color, b.color);
            }
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&a.native_quads),
                bytemuck::cast_slice::<_, u8>(&b.native_quads)
            );
            assert_eq!(a.face_profiles, b.face_profiles);
            assert_eq!(a.face_profile_indices, b.face_profile_indices);
            assert_eq!(a.certified_error_m, b.certified_error_m);
            assert_eq!(a.conservative_surface_bounds, b.conservative_surface_bounds);
        }
        assert_eq!(
            serial.stats().generated_columns,
            parallel.stats().generated_columns,
            "shared shard miss ownership must avoid duplicate source generation"
        );
        assert!(parallel.stats().peak_bytes <= 128 * 1024 * 1024);
        assert!(parallel.prepare_parallel(&keys, 2, || false).is_err());
        assert!(parallel
            .prepare_parallel(&[keys[0], keys[0]], 2, || false)
            .is_err());
        assert!(parallel
            .prepare_parallel(
                &[TileKey {
                    min: [1024, -512],
                    ..keys[0]
                }],
                1,
                || true
            )
            .is_err());
    }
    #[test]
    fn eager_bound_storage_is_reported_before_first_build_and_after_clear() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut cache = GeometryCache::new(world, 128 * 1024 * 1024);
        let initial = cache.stats();
        assert!(initial.affine_cache_bytes > 0);
        assert!(initial.appearance_cache_bytes > 0);
        let fixed = initial.affine_cache_bytes + initial.appearance_cache_bytes;
        assert_eq!(initial.source_cache_bytes, fixed);
        assert_eq!(initial.resident_bytes, fixed);
        assert!(initial.peak_bytes >= initial.resident_bytes);
        let tile = cache.get_or_build(key(8), || false).unwrap();
        let built = cache.stats();
        cache.clear();
        let cleared = cache.stats();
        assert_eq!(cleared.source_cache_bytes, fixed);
        assert_eq!(cleared.resident_bytes, fixed);
        assert_eq!(cleared.source_cache_columns, 0);
        assert_eq!(cleared.source_cache_pages, 0);
        assert!(cleared.peak_bytes >= built.peak_bytes);
        assert_eq!(cleared.live_tile_payload_bytes, tile.bytes);
    }
    #[test]
    fn appearance_center_column_reuse_preserves_float_bits_extrema_and_sample_addresses() {
        for seed in [7, 11] {
            let world = World::fbm(seed, FbmParams::default()).unwrap();
            for (width, size, stride) in [(128, 4, 1), (128, 16, 1), (128, 32, 1), (256, 32, 8)] {
                let run = |reuse_center| {
                    let mut samples = SourceSamples::new(0);
                    let key = TileKey {
                        min: [-127, -511],
                        width_cells: width,
                        step_cells: size as u32,
                        edge_steps: [size as u32; 4],
                        source_revision: 0,
                    };
                    let mut generator = Generator {
                        defer_profiles: false,
                        experiment: None,
                        world: &world,
                        key,
                        samples: &mut samples,
                        heights: LocalColumns::new(width),
                        mesh: Mesh {
                            vertices: vec![],
                            indices: vec![],
                            origin: Cell::default(),
                            exposed_native_faces: 0,
                        },
                        empirical_error_m: 0.0,
                        affine_error: 0.0,
                        affine_range: [f64::INFINITY, f64::NEG_INFINITY],
                        affine_complete: true,
                        affine_stats: AffineStats::default(),
                        face_profiles: vec![[[0.; 4]; 5]],
                        spatial_nodes: HashMap::new(),
                        face_profile_indices: vec![],
                        appearance: FaceAppearance::default(),
                    };
                    let profile = generator
                        .compute_patch_profile_with_center_reuse(
                            0,
                            0,
                            size,
                            stride,
                            ProfileKey {
                                min: key.min,
                                size,
                                stride,
                            },
                            reuse_center,
                        )
                        .unwrap();
                    let addresses: Vec<_> = (-1..=size)
                        .flat_map(|z| (-1..=size).map(move |x| [x, z]))
                        .filter_map(|p| {
                            generator.heights.get(&p).copied().map(|column| (p, column))
                        })
                        .collect();
                    let count = generator.heights.len();
                    let bits = profile.value.map(|axis| axis.map(f32::to_bits));
                    (
                        bits,
                        profile.extrema,
                        addresses,
                        count,
                        samples.generated,
                        samples.hits,
                    )
                };
                let control = run(false);
                let reused = run(true);
                assert_eq!(
                    reused, control,
                    "seed={seed}, width={width}, size={size}, stride={stride}"
                );
                assert!(reused.4 > 0);
            }
        }
    }
    #[test]
    fn appearance_cache_preserves_tile_bits_across_edges_widths_and_clear() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut cache = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
        for (width, step, edge) in [
            (64, 16, 16),
            (128, 16, 16),
            (128, 16, 8),
            (128, 16, 1),
            (256, 64, 64),
            (512, 64, 32),
        ] {
            let key = TileKey {
                min: [-127, -511],
                width_cells: width,
                step_cells: step,
                edge_steps: [edge, step, step, step],
                source_revision: 0,
            };
            let cached = cache.get_or_build(key, || false).unwrap();
            let direct = build(&world, key, || false).unwrap();
            assert_appearance_tile_bits(&cached, &direct);
        }
        let before = cache.stats();
        assert!(before.appearance_cache_hits > 0);
        assert!(before.appearance_cache_misses > 0);
        assert!(before.appearance_cache_bytes <= 4 * 1024 * 1024);
        assert!(before.source_cache_bytes <= cache.source_budget);
        assert!(before.peak_bytes <= cache.budget);
        cache.clear();
        assert!(cache.samples.pages.shards.iter().all(|s| s
            .lock()
            .unwrap()
            .profiles
            .iter()
            .all(Option::is_none)));
        let key = TileKey {
            min: [-127, -511],
            width_cells: 128,
            step_cells: 16,
            edge_steps: [16; 4],
            source_revision: 0,
        };
        let rebuilt = cache.get_or_build(key, || false).unwrap();
        assert_appearance_tile_bits(&rebuilt, &build(&world, key, || false).unwrap());
        assert!(cache.stats().appearance_cache_misses > before.appearance_cache_misses);
    }
    fn assert_appearance_tile_bits(cached: &GeometryTile, direct: &GeometryTile) {
        assert_eq!(proxy_primitive_bits(cached), proxy_primitive_bits(direct));
        let profile_bits = |tile: &GeometryTile| {
            tile.face_profiles
                .iter()
                .flat_map(|profile| {
                    profile
                        .iter()
                        .flat_map(|axis| axis.iter().map(|v| v.to_bits()))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(profile_bits(cached), profile_bits(direct));
        assert_eq!(cached.certified_error_m, direct.certified_error_m);
        assert_eq!(
            cached.best_certified_vertical_error_m,
            direct.best_certified_vertical_error_m
        );
        assert_eq!(cached.exact_height_range_m, direct.exact_height_range_m);
        assert_eq!(
            cached.conservative_surface_bounds,
            direct.conservative_surface_bounds
        );
        assert_eq!(
            cached.empirical_error_m.to_bits(),
            direct.empirical_error_m.to_bits()
        );
        assert_eq!(
            cached.appearance.area_m2.map(f64::to_bits),
            direct.appearance.area_m2.map(f64::to_bits)
        );
        assert_eq!(
            cached
                .appearance
                .color_area
                .map(|axis| axis.map(f64::to_bits)),
            direct
                .appearance
                .color_area
                .map(|axis| axis.map(f64::to_bits))
        );
    }
    #[test]
    fn appearance_cache_parallel_products_and_source_scopes_match_uncached() {
        for seed in [7, 11] {
            let world = World::fbm(seed, FbmParams::default()).unwrap();
            let mut cache = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
            let keys = [16, 8].map(|edge| TileKey {
                min: [-128, -512],
                width_cells: 128,
                step_cells: 16,
                edge_steps: [edge, 16, 16, 16],
                source_revision: 0,
            });
            let products = cache.prepare_parallel(&keys, 2, || false).unwrap();
            for (tile, key) in products.iter().zip(keys) {
                assert_appearance_tile_bits(tile, &build(&world, key, || false).unwrap());
            }
            let key = TileKey {
                edge_steps: [4, 16, 16, 16],
                ..keys[0]
            };
            let tile = cache.get_or_build(key, || false).unwrap();
            assert_appearance_tile_bits(&tile, &build(&world, key, || false).unwrap());
            assert!(cache.stats().appearance_cache_hits > 0);
        }
    }
    #[test]
    fn affine_source_cache_reuses_edge_variants_and_preserves_exact_bounds() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut cache = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
        let first = TileKey {
            min: [0, -1024],
            width_cells: 512,
            step_cells: 128,
            edge_steps: [128; 4],
            source_revision: 0,
        };
        cache.get_or_build(first, || false).unwrap();
        let before = cache.stats();
        let second = TileKey {
            edge_steps: [64, 128, 128, 128],
            ..first
        };
        let reused = cache.get_or_build(second, || false).unwrap();
        let after = cache.stats();
        let direct = build(&world, second, || false).unwrap();
        assert!(after.affine_cache_hits > before.affine_cache_hits);
        assert_eq!(
            after.affine_cache_misses, before.affine_cache_misses,
            "small edge variant should reuse every source patch bound"
        );
        assert_eq!(reused.affine_error_m, direct.affine_error_m);
        assert_eq!(reused.affine_height_range_m, direct.affine_height_range_m);
        assert_eq!(
            reused.best_certified_vertical_error_m,
            direct.best_certified_vertical_error_m
        );
        assert_eq!(reused.face_profiles, direct.face_profiles);
        assert_eq!(proxy_primitive_bits(&reused), proxy_primitive_bits(&direct));
        assert!(after.affine_cache_bytes <= 4 * 1024 * 1024);
        assert!(after.peak_bytes <= 128 * 1024 * 1024);
        cache.clear();
        assert!(cache.samples.pages.shards.iter().all(|s| s
            .lock()
            .unwrap()
            .bounds
            .iter()
            .all(Option::is_none)));
    }
    #[test]
    fn linked_lru_matches_reference_order_and_charges_metadata() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut cache = GeometryCache::new(world, 128 * 1024);
        cache.mesh_budget = 3 * (1024 + LRU_ENTRY_BYTES);
        let mut reference = std::collections::VecDeque::new();
        // Repeated newest hits, middle hits, misses and eviction pressure.
        for i in 0..1000 {
            let k = TileKey {
                min: [(i * 17 % 7) * 64, 0],
                ..key(8)
            };
            if cache.get(k).is_none() {
                if reference.len() == 3 {
                    reference.pop_front();
                }
                let mut tile = finish(
                    k,
                    Mesh {
                        vertices: vec![],
                        indices: vec![],
                        origin: Cell { x: 0, y: 0, z: 0 },
                        exposed_native_faces: 0,
                    },
                    0,
                    0.,
                    false,
                );
                tile.bytes = 1024;
                cache.admit_built(k, Ok(tile), 0.).unwrap();
            } else {
                let index = reference.iter().position(|&entry| entry == k).unwrap();
                reference.remove(index);
            }
            reference.push_back(k);
            let mut actual = Vec::new();
            let mut current = cache.oldest;
            let mut previous = None;
            while let Some(k) = current {
                assert!(
                    actual.len() < cache.entries.len(),
                    "LRU links must not cycle"
                );
                let entry = &cache.entries[&k];
                assert_eq!(entry.previous, previous);
                previous = current;
                current = entry.next;
                actual.push(k);
            }
            assert_eq!(previous, cache.newest);
            assert_eq!(actual, reference.iter().copied().collect::<Vec<_>>());
            assert_eq!(
                cache.lru_metadata_bytes(),
                cache.entries.len() * LRU_ENTRY_BYTES
            );
            assert!(cache.mesh_bytes + cache.lru_metadata_bytes() <= cache.mesh_budget);
            assert_eq!(cache.stats().live_tile_payload_bytes, cache.mesh_bytes);
        }
        let retained = cache.get(*reference.front().unwrap()).unwrap();
        let size = retained.bytes;
        cache.clear();
        assert_eq!(cache.oldest, None);
        assert_eq!(cache.newest, None);
        assert_eq!(cache.stats().lru_metadata_bytes, 0);
        assert_eq!(cache.stats().live_evicted_payload_bytes, size);
        drop(retained);
        assert_eq!(cache.stats().live_tile_payload_bytes, 0);
    }
    #[test]
    fn optional_source_profile_preserves_bits_and_partitions_worker_cost() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut detailed = GeometryCache::new(world.clone(), 16 * 1024 * 1024);
        detailed.samples.profile_enabled = true;
        let mut normal = GeometryCache::new(world, 16 * 1024 * 1024);
        normal.samples.profile_enabled = false;
        for k in [key(8), key(1)] {
            let a = detailed.get_or_build(k, || false).unwrap();
            let b = normal.get_or_build(k, || false).unwrap();
            assert_eq!(proxy_primitive_bits(&a), proxy_primitive_bits(&b));
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&a.native_quads),
                bytemuck::cast_slice::<_, u8>(&b.native_quads)
            );
            assert_eq!(a.face_profiles, b.face_profiles);
            assert_eq!(a.certified_error_m, b.certified_error_m);
        }
        let stats = detailed.stats();
        assert!(stats.profile_enabled);
        assert!(
            stats.source_lock_ms > 0.
                && stats.source_lookup_ms > 0.
                && stats.source_generate_ms > 0.
        );
        let exclusive_sum = stats.appearance_exclusive_ms
            + stats.certificate_exclusive_ms
            + stats.native_mesh_exclusive_ms
            + stats.proxy_exclusive_ms
            + stats.affine_ms
            + stats.finalize_ms
            + stats.source_lock_ms
            + stats.source_lookup_ms
            + stats.source_generate_ms;
        assert!(
            (exclusive_sum - stats.total_build_ms).abs() < 1e-6,
            "exclusive sum {exclusive_sum}, worker total {}",
            stats.total_build_ms
        );
        let disabled = normal.stats();
        assert!(!disabled.profile_enabled);
        assert_eq!(
            disabled.source_lock_ms + disabled.source_lookup_ms + disabled.source_generate_ms,
            0.
        );
        assert_eq!(stats.generated_columns, disabled.generated_columns);
        assert_eq!(stats.source_cache_hits, disabled.source_cache_hits);
    }
    #[test]
    fn live_payload_tracks_evicted_arcs_until_last_external_owner_drops() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let a = build(&world, key(8), || false).unwrap();
        let size = a.bytes;
        let mut cache = GeometryCache::new(world, size * 2);
        cache.admit_built(a.key, Ok(a), 0.).unwrap();
        let retained = cache.get(key(8)).unwrap();
        let size = retained.bytes;
        for i in 1..8 {
            let k = TileKey {
                min: [key(8).min[0] + i * 64, key(8).min[1]],
                ..key(8)
            };
            let tile = build(&cache.world, k, || false).unwrap();
            cache.admit_built(k, Ok(tile), 0.).unwrap();
        }
        assert!(!cache.contains(&key(8)));
        assert_eq!(
            cache.stats().live_tile_payload_bytes,
            cache.mesh_bytes + size
        );
        drop(retained);
        assert_eq!(cache.stats().live_tile_payload_bytes, cache.mesh_bytes);
        assert!(cache.stats().peak_live_tile_payload_bytes >= cache.mesh_bytes + size);
    }
    #[test]
    fn dense_local_workspace_preserves_sparse_values_halos_and_replacements() {
        let mut dense = LocalColumns::new(128);
        let mut sparse = LocalColumns::new(1024);
        for z in -2..=130 {
            for x in -2..=130 {
                let p = [x, z];
                let v = NativeColumn {
                    height: x * 31 - z * 17,
                    bands: [Material::Grass, Material::Soil, Material::Stone],
                };
                dense.insert(p, v);
                sparse.insert(p, v);
                assert_eq!(dense.get(&p), sparse.get(&p));
            }
        }
        dense.insert([0, 0], NativeColumn::default());
        sparse.insert([0, 0], NativeColumn::default());
        assert_eq!(dense.len(), sparse.len());
        for z in -3..=131 {
            for x in -3..=131 {
                assert_eq!(dense.get(&[x, z]), sparse.get(&[x, z]));
            }
        }
    }
    #[test]
    #[ignore = "bounded manual CPU stage profile; no timing pass threshold"]
    fn profile_primary_cold_stages() {
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 1800.,
                amplitude: 1000.,
                octaves: 8,
                roughness: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
        let keys: Vec<_> = (0..4)
            .map(|i| TileKey {
                min: [i * 128, -512],
                width_cells: 128,
                step_cells: 8,
                edge_steps: [1; 4],
                source_revision: 0,
            })
            .collect();
        for workers in [1, 4] {
            let mut cache = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
            let started = std::time::Instant::now();
            for batch in keys.chunks(workers) {
                cache.prepare_parallel(batch, workers, || false).unwrap();
            }
            eprintln!(
                "batch workers={workers} wall_ms={:.3} generated={} hits={}",
                started.elapsed().as_secs_f64() * 1000.,
                cache.stats().generated_columns,
                cache.stats().source_cache_hits
            );
        }
        for (min, step) in [
            ([0, -512], 8),
            ([-256, -1536], 16),
            ([512, -3072], 32),
            ([0, -512], 1),
            ([0, -512], 128),
        ] {
            let width = if step == 128 { 2048 } else { 128 };
            let key = TileKey {
                min,
                width_cells: width,
                step_cells: step,
                edge_steps: [step; 4],
                source_revision: 0,
            };
            FORCE_SPARSE_WORKSPACE.set(true);
            let mut sparse = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
            let sparse_tile = sparse.get_or_build(key, || false).unwrap();
            eprintln!("sparse min={min:?} step={step} total_ms={:.3} appearance_ms={:.3} certificate_ms={:.3}",sparse.stats().total_build_ms,sparse.stats().appearance_ms,sparse.stats().certificate_ms);
            FORCE_SPARSE_WORKSPACE.set(false);
            let mut cold = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
            let tile = cold.get_or_build(key, || false).unwrap();
            assert_eq!(sparse_tile.face_profiles, tile.face_profiles);
            assert_eq!(sparse_tile.certified_error_m, tile.certified_error_m);
            assert_eq!(sparse_tile.mesh.indices, tile.mesh.indices);
            eprintln!(
                "cold min={min:?} step={step} columns={} triangles={} stages={:?}",
                tile.sampled_columns,
                tile.triangle_count(),
                cold.stats()
            );
            if width == 128 {
                let mut samples = SourceSamples::new(16 * 1024 * 1024);
                let started = std::time::Instant::now();
                for z in min[1] - 1..=min[1] + width as i64 {
                    for x in min[0] - 1..=min[0] + width as i64 {
                        samples.column(&world, [x, z]);
                    }
                }
                let source_ms = started.elapsed().as_secs_f64() * 1000.;
                let started = std::time::Instant::now();
                let warm = build_cached(&world, key, || false, &mut samples).unwrap();
                eprintln!("prefilled min={min:?} step={step} source_ms={source_ms:.3} warm_build_ms={:.3} stages={:?}",started.elapsed().as_secs_f64()*1000.,samples.stages);
                assert_eq!(proxy_primitive_bits(&tile), proxy_primitive_bits(&warm));
                assert_eq!(tile.face_profiles, warm.face_profiles);
                assert_eq!(tile.certified_error_m, warm.certified_error_m);
            }
        }
        let mut cache = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
        for edge in [128, 64] {
            let key = TileKey {
                min: [0, -512],
                width_cells: 2048,
                step_cells: 128,
                edge_steps: [edge, 128, 128, 128],
                source_revision: 0,
            };
            let before = cache.stats();
            let started = std::time::Instant::now();
            cache.get_or_build(key, || false).unwrap();
            let after = cache.stats();
            eprintln!(
                "affine_edge edge={edge} wall_ms={:.3} affine_ms={:.3} hits={} misses={}",
                started.elapsed().as_secs_f64() * 1000.,
                after.affine_ms - before.affine_ms,
                after.affine_cache_hits - before.affine_cache_hits,
                after.affine_cache_misses - before.affine_cache_misses
            );
        }
    }
    #[test]
    fn topology_owner_excludes_caps_and_owns_boundary_faces_after_edits() {
        let mut world = World::fbm_topology(7, FbmParams::default()).unwrap();
        let base = world.topology_base_cells().unwrap();
        world.set(
            Cell {
                x: 0,
                y: base + 100,
                z: 0,
            },
            Material::Stone,
        );
        let outside = TileKey {
            min: [128, -64],
            source_revision: world.revision(),
            ..key(4)
        };
        let tile = build(&world, outside, || false).unwrap();
        assert_eq!(tile.key.source_revision, world.revision());
        for triangle in tile.mesh.indices.chunks_exact(3) {
            let tri: [Vertex; 3] =
                std::array::from_fn(|a| tile.mesh.vertices[triangle[a] as usize]);
            assert!(
                !tri.iter().all(|v| v.position[0] == 0.),
                "surface must not own carved volume boundary side"
            );
        }
        assert!(build(
            &world,
            TileKey {
                source_revision: world.revision(),
                ..key(4)
            },
            || false
        )
        .is_err());
        world.set(
            Cell {
                x: 256,
                y: base + 100,
                z: 0,
            },
            Material::Wood,
        );
        assert!(build(
            &world,
            TileKey {
                source_revision: world.revision(),
                ..outside
            },
            || false
        )
        .is_err());
    }
    #[test]
    fn vertex_compaction_preserves_every_triangle_attribute_and_profile() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let key = TileKey {
            min: [0, 0],
            width_cells: 128,
            step_cells: 4,
            edge_steps: [1; 4],
            source_revision: 0,
        };
        let mut tile = build_surface(&world, key, || false, &mut SourceSamples::new(0)).unwrap();
        let before: Vec<_> = tile
            .mesh
            .indices
            .iter()
            .map(|&i| {
                let vertex = tile.mesh.vertices[i as usize];
                (
                    vertex.position.map(f32::to_bits),
                    vertex.normal.map(f32::to_bits),
                    vertex.color.map(f32::to_bits),
                    tile.face_profile_indices[i as usize],
                )
            })
            .collect();
        let old_bytes = tile.bytes;
        compact_vertices(&mut tile);
        let after: Vec<_> = tile
            .mesh
            .indices
            .iter()
            .map(|&i| {
                let vertex = tile.mesh.vertices[i as usize];
                (
                    vertex.position.map(f32::to_bits),
                    vertex.normal.map(f32::to_bits),
                    vertex.color.map(f32::to_bits),
                    tile.face_profile_indices[i as usize],
                )
            })
            .collect();
        assert_eq!(before, after);
        assert!(tile.bytes < old_bytes);
        assert!(
            tile.face_profiles.len() <= 1025,
            "one directional profile perpatch, not pervertex"
        );
    }
    #[test]
    fn native_summary_reuse_matches_proxy_bits_extrema_and_both_cache_directions() {
        use frontier_world::FbmRecipe;
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
            FbmRecipe::Alpine,
        ] {
            for seed in [7, 11] {
                let world = World::fbm(
                    seed,
                    FbmParams {
                        recipe,
                        horizontal_scale: 16.,
                        amplitude: 40.,
                        ..Default::default()
                    },
                )
                .unwrap();
                let k = TileKey {
                    min: [-32, -64],
                    width_cells: 16,
                    step_cells: 1,
                    edge_steps: [1; 4],
                    source_revision: 0,
                };
                let original = column_mesh::build(
                    &world,
                    Cell {
                        x: k.min[0],
                        y: 0,
                        z: k.min[1],
                    },
                    [16; 2],
                )
                .unwrap();
                let profile_key = ProfileKey {
                    min: k.min,
                    size: 16,
                    stride: 1,
                };
                let mut samples = SourceSamples::new(8 * 1024 * 1024);
                let run = |pages: Option<&SharedSourcePages>| {
                    native_profiles_with_summary_reuse(
                        &world,
                        k,
                        Mesh {
                            vertices: original.mesh.vertices.clone(),
                            indices: original.mesh.indices.clone(),
                            origin: original.mesh.origin,
                            exposed_native_faces: original.mesh.exposed_native_faces,
                        },
                        &original.heights,
                        &mut || false,
                        true,
                        pages,
                    )
                    .unwrap()
                };
                let assert_native =
                    |a: &(Mesh, Vec<[[f32; 4]; 5]>, Vec<u32>),
                     b: &(Mesh, Vec<[[f32; 4]; 5]>, Vec<u32>)| {
                        assert_eq!(
                            bytemuck::cast_slice::<Vertex, u8>(&a.0.vertices),
                            bytemuck::cast_slice::<Vertex, u8>(&b.0.vertices)
                        );
                        assert_eq!(a.0.indices, b.0.indices);
                        assert_eq!(a.0.exposed_native_faces, b.0.exposed_native_faces);
                        assert_eq!(
                            a.1.iter()
                                .map(|p| p.map(|a| a.map(f32::to_bits)))
                                .collect::<Vec<_>>(),
                            b.1.iter()
                                .map(|p| p.map(|a| a.map(f32::to_bits)))
                                .collect::<Vec<_>>()
                        );
                        assert_eq!(a.2, b.2);
                    };
                let direct_native = run(None);
                assert_native(&direct_native, &run(Some(&samples.pages)));
                let native_entry = samples.pages.profile(profile_key).unwrap();
                let proxy_key = TileKey {
                    step_cells: 16,
                    edge_steps: [16; 4],
                    ..k
                };
                let before = samples.pages.snapshot();
                let reused_proxy =
                    build_surface(&world, proxy_key, || false, &mut samples).unwrap();
                assert!(
                    samples.pages.snapshot().appearance_cache_hits > before.appearance_cache_hits
                );
                let mut direct_samples = SourceSamples::new(8 * 1024 * 1024);
                let direct_proxy =
                    build_surface(&world, proxy_key, || false, &mut direct_samples).unwrap();
                assert_appearance_tile_bits(&reused_proxy, &direct_proxy);
                let proxy_entry = direct_samples.pages.profile(profile_key).unwrap();
                assert_eq!(
                    native_entry.value.map(|a| a.map(f32::to_bits)),
                    proxy_entry.value.map(|a| a.map(f32::to_bits))
                );
                assert_eq!(native_entry.extrema, proxy_entry.extrema);
                let before = direct_samples.pages.snapshot();
                assert_native(&direct_native, &run(Some(&direct_samples.pages)));
                assert_eq!(
                    direct_samples.pages.snapshot().appearance_cache_hits,
                    before.appearance_cache_hits + 1
                );
                // A sampled profile is never a substitute for this exact footprint.
                assert!(samples
                    .pages
                    .profile(ProfileKey {
                        stride: 4,
                        ..profile_key
                    })
                    .is_none());
                let bytes = samples.pages.snapshot().appearance_cache_bytes;
                samples.pages.clear();
                assert!(samples.pages.profile(profile_key).is_none());
                assert_eq!(samples.pages.snapshot().appearance_cache_bytes, bytes);
                assert!(samples.pages.snapshot().source_cache_bytes <= 8 * 1024 * 1024);
                // Storage and immutable source scope stay independent of another world.
                let fresh = SharedSourcePages::new(8 * 1024 * 1024);
                assert!(fresh.profile(profile_key).is_none());
                assert_native(&direct_native, &run(Some(&samples.pages)));
            }
        }
    }
    #[test]
    fn native_summary_reuse_never_admits_clipped_blocks() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let k = TileKey {
            min: [-15, -15],
            width_cells: 16,
            ..key(1)
        };
        let original = column_mesh::build(
            &world,
            Cell {
                x: -15,
                y: 0,
                z: -15,
            },
            [16; 2],
        )
        .unwrap();
        let pages = SharedSourcePages::new(8 * 1024 * 1024);
        let out = native_profiles_with_summary_reuse(
            &world,
            k,
            original.mesh,
            &original.heights,
            &mut || false,
            true,
            Some(&pages),
        )
        .unwrap();
        assert_eq!(out.1.len(), 5);
        let stats = pages.snapshot();
        assert_eq!(stats.appearance_cache_hits, 0);
        assert_eq!(stats.appearance_cache_misses, 0);
        assert!(pages.shards.iter().all(|s| s
            .lock()
            .unwrap()
            .profiles
            .iter()
            .all(Option::is_none)));
    }
    #[test]
    fn native_profile_top_reuse_matches_direct_paint_bits_and_quads() {
        use frontier_world::FbmRecipe;
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
        ] {
            for seed in [7, 11] {
                let world = World::fbm(
                    seed,
                    FbmParams {
                        recipe,
                        horizontal_scale: 16.,
                        amplitude: 40.,
                        ..Default::default()
                    },
                )
                .unwrap();
                // Nonaligned footprints exercise clipped profile blocks, negative
                // coordinates, slopes and their soil/stone side strata.
                for min in [[-127, -511], [31, -33]] {
                    let mut k = key(1);
                    k.min = min;
                    k.width_cells = 33;
                    let original = column_mesh::build(
                        &world,
                        Cell {
                            x: min[0],
                            y: 0,
                            z: min[1],
                        },
                        [33; 2],
                    )
                    .unwrap();
                    let run = |reuse_top| {
                        native_profiles_with_top_reuse(
                            &world,
                            k,
                            Mesh {
                                vertices: original.mesh.vertices.clone(),
                                indices: original.mesh.indices.clone(),
                                origin: original.mesh.origin,
                                exposed_native_faces: original.mesh.exposed_native_faces,
                            },
                            &original.heights,
                            &mut || false,
                            reuse_top,
                        )
                        .unwrap()
                    };
                    let direct = run(false);
                    let reused = run(true);
                    assert_eq!(direct.0.indices, reused.0.indices);
                    assert_eq!(direct.0.origin, reused.0.origin);
                    assert_eq!(direct.0.exposed_native_faces, reused.0.exposed_native_faces);
                    assert_eq!(
                        bytemuck::cast_slice::<Vertex, u8>(&direct.0.vertices),
                        bytemuck::cast_slice::<Vertex, u8>(&reused.0.vertices)
                    );
                    assert_eq!(
                        direct
                            .1
                            .iter()
                            .map(|p| p.map(|a| a.map(f32::to_bits)))
                            .collect::<Vec<_>>(),
                        reused
                            .1
                            .iter()
                            .map(|p| p.map(|a| a.map(f32::to_bits)))
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(direct.2, reused.2);
                }
            }
        }
    }
    #[test]
    #[ignore = "bounded same-binary CPU comparison; no timing pass threshold"]
    fn profile_native_top_material_reuse() {
        use frontier_world::FbmRecipe;
        for recipe in [FbmRecipe::Legacy, FbmRecipe::Regional] {
            let world = World::fbm(
                7,
                FbmParams {
                    recipe,
                    horizontal_scale: 1800.,
                    amplitude: 1000.,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut k = key(1);
            k.min = [-127, -511];
            k.width_cells = 128;
            let original = column_mesh::build(
                &world,
                Cell {
                    x: k.min[0],
                    y: 0,
                    z: k.min[1],
                },
                [128; 2],
            )
            .unwrap();
            for reuse_top in [false, true, true, false] {
                let mesh = Mesh {
                    vertices: original.mesh.vertices.clone(),
                    indices: original.mesh.indices.clone(),
                    origin: original.mesh.origin,
                    exposed_native_faces: original.mesh.exposed_native_faces,
                };
                let started = std::time::Instant::now();
                let out = native_profiles_with_top_reuse(
                    &world,
                    k,
                    mesh,
                    &original.heights,
                    &mut || false,
                    reuse_top,
                )
                .unwrap();
                eprintln!("native_profile recipe={recipe:?} reuse_top={reuse_top} ms={:.3} avoided_paints={} workspace_bytes={} profiles={} quads={}",
                    started.elapsed().as_secs_f64() * 1000., if reuse_top { 128 * 128 } else { 0 },
                    original.heights.allocated_bytes(), out.1.len(), out.0.vertices.len() / 4);
                std::hint::black_box(out);
            }
        }
    }
    #[test]
    fn one_meter_native_profile_splits_preserve_exact_unit_face_identity_and_materials() {
        use std::collections::BTreeSet;
        let face_set = |mesh: &Mesh| {
            let mut faces = BTreeSet::new();
            let origin = [mesh.origin.x, mesh.origin.y, mesh.origin.z];
            for q in mesh.vertices.chunks_exact(4) {
                let axis = q[0].normal.iter().position(|&v| v != 0.).unwrap();
                let positive = q[0].normal[axis] > 0.;
                let u = (axis + 1) % 3;
                let v = (axis + 2) % 3;
                let p = q[0]
                    .position
                    .map(|n| (n as f64 / VOXEL_SIZE).round() as i64);
                let end = q[2]
                    .position
                    .map(|n| (n as f64 / VOXEL_SIZE).round() as i64);
                for a in p[u]..end[u] {
                    for b in p[v]..end[v] {
                        let mut c = p;
                        c[u] = a;
                        c[v] = b;
                        c[axis] -= i64::from(positive);
                        for n in 0..3 {
                            c[n] += origin[n];
                        }
                        assert!(
                            faces.insert((c, axis, positive, q[0].color.map(f32::to_bits))),
                            "duplicate native face"
                        );
                    }
                }
            }
            faces
        };
        for params in [
            FbmParams::default(),
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 40.,
                ..Default::default()
            },
        ] {
            let world = World::fbm(7, params).unwrap();
            let k = key(1);
            let original = column_mesh::build(
                &world,
                Cell {
                    x: k.min[0],
                    y: 0,
                    z: k.min[1],
                },
                [32; 2],
            )
            .unwrap();
            let split = build_surface(&world, k, || false, &mut SourceSamples::new(0)).unwrap();
            assert_eq!(face_set(&original.mesh), face_set(&split.mesh));
            assert_eq!(
                original.mesh.exposed_native_faces,
                split.mesh.exposed_native_faces
            );
            for (i, q) in split.mesh.vertices.chunks_exact(4).enumerate() {
                assert!(split.face_profile_indices[i * 4..i * 4 + 4]
                    .iter()
                    .all(|&id| id == split.face_profile_indices[i * 4]));
                for a in [0, 2] {
                    let offset = if a == 0 { k.min[0] } else { k.min[1] };
                    let lo = q
                        .iter()
                        .map(|v| (v.position[a] as f64 / VOXEL_SIZE).round() as i64 + offset)
                        .min()
                        .unwrap();
                    let hi = q
                        .iter()
                        .map(|v| (v.position[a] as f64 / VOXEL_SIZE).round() as i64 + offset)
                        .max()
                        .unwrap();
                    if lo != hi {
                        assert_eq!(
                            lo.div_euclid(16),
                            (hi - 1).div_euclid(16),
                            "quad crosses profile block"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn conforming_native_records_preserve_exact_faces_materials_and_profiles() {
        assert_eq!(std::mem::size_of::<NativeQuad>(), 32);
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 40.,
                ..Default::default()
            },
        )
        .unwrap();
        for min in [[-16, -16], [1_600_000, -1_600_000]] {
            let k = TileKey { min, ..key(1) };
            let mut tile = build_surface(&world, k, || false, &mut SourceSamples::new(0)).unwrap();
            let vertices = tile.mesh.vertices.clone();
            let mut expected = std::collections::BTreeSet::new();
            for (i, q) in vertices.chunks_exact(4).enumerate() {
                let axis = q[0].normal.iter().position(|&n| n != 0.).unwrap();
                let u = (axis + 1) % 3;
                let v = (axis + 2) % 3;
                let p = q[0]
                    .position
                    .map(|n| (n as f64 / VOXEL_SIZE).round() as i64);
                let hi = q[2]
                    .position
                    .map(|n| (n as f64 / VOXEL_SIZE).round() as i64);
                for vv in p[v]..hi[v] {
                    for uu in p[u]..hi[u] {
                        let mut cell = p;
                        cell[u] = uu;
                        cell[v] = vv;
                        assert!(expected.insert((
                            cell,
                            axis as u32 | if q[0].normal[axis] < 0. { 4 } else { 0 },
                            q[0].color.map(f32::to_bits),
                            tile.face_profile_indices[i * 4]
                        )));
                    }
                }
            }
            pack_native_quads(&mut tile).unwrap();
            assert!(
                tile.mesh.vertices.is_empty()
                    && tile.mesh.indices.is_empty()
                    && tile.face_profile_indices.is_empty()
            );
            assert!(!tile.is_empty());
            let palette = [
                Material::Air,
                Material::Grass,
                Material::Soil,
                Material::Stone,
                Material::Wood,
            ];
            let mut actual = std::collections::BTreeSet::new();
            for q in &tile.native_quads {
                // Every shared edge has exactly one canonical-cell segment:
                // no edge can contain another unit face's vertex in its interior.
                assert_eq!(q.extent, [VOXEL_SIZE as f32; 2]);
                assert!(actual.insert((
                    q.anchor.map(|n| (n as f64 / VOXEL_SIZE).round() as i64),
                    q.axis_sign,
                    palette[q.material as usize].color().map(f32::to_bits),
                    q.profile_index
                )));
            }
            assert_eq!(expected, actual);
            assert_eq!(tile.triangle_count(), expected.len() as u64 * 2);
        }
    }
    #[test]
    fn oversized_native_tessellation_is_rejected_before_packed_allocation() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut tile = build_surface(&world, key(1), || false, &mut SourceSamples::new(0)).unwrap();
        let axis = tile.mesh.vertices[0]
            .normal
            .iter()
            .position(|&n| n != 0.)
            .unwrap();
        for a in (0..3).filter(|&a| a != axis) {
            tile.mesh.vertices[2].position[a] = tile.mesh.vertices[0].position[a] + 1000.;
        }
        assert_eq!(
            pack_native_quads(&mut tile).unwrap_err(),
            "native tessellation exceeds output memory budget"
        );
        assert!(tile.native_quads.is_empty());
        assert!(!tile.mesh.vertices.is_empty());
    }
    #[test]
    fn volume_boundary_profiles_use_canonical_face_fallback_instead_of_base_halo() {
        let mut world = World::fbm_topology(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        world.set(
            Cell {
                x: 127,
                y: -1,
                z: 0,
            },
            Material::Air,
        );
        let tile = build(
            &world,
            TileKey {
                min: [128, -16],
                source_revision: world.revision(),
                ..key(1)
            },
            || false,
        )
        .unwrap();
        assert!(!tile.appearance.exhaustive);
        let mut exact_band = false;
        let mut outside_band = false;
        for q in &tile.native_quads {
            if q.anchor[0] < 1. {
                assert_eq!(tile.face_profiles[q.profile_index as usize], [[0.; 4]; 5]);
                exact_band = true;
            } else {
                assert!(tile.face_profiles[q.profile_index as usize][0][3] > 0.);
                outside_band = true;
            }
        }
        assert!(exact_band && outside_band);
        let proxy = build(
            &world,
            TileKey {
                min: [128, -16],
                source_revision: world.revision(),
                ..key(8)
            },
            || false,
        )
        .unwrap();
        assert!(!proxy.appearance.exhaustive);
        for (i, v) in proxy.mesh.vertices.iter().enumerate() {
            if v.position[0] == 0. {
                assert_eq!(
                    proxy.face_profiles[proxy.face_profile_indices[i] as usize],
                    [[0.; 4]; 5]
                );
            }
        }
    }
    #[test]
    fn occluder_boxes_are_canonical_solid_and_unknown_candidates_use_global_bounds() {
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 40.,
                ..Default::default()
            },
        )
        .unwrap();
        for step in [1, 8] {
            let k = key(step);
            let tile = build(&world, k, || false).unwrap();
            let solid = tile
                .guaranteed_solid_box
                .expect("bounded certificate supplies solid interior");
            for z in 0..32 {
                for x in 0..32 {
                    let cx = k.min[0] + x;
                    let cz = k.min[1] + z;
                    let h = world.column(cx, cz).height_m();
                    assert!(h >= solid.max[1]);
                    assert!(
                        h >= tile.conservative_surface_bounds[0][1]
                            && h <= tile.conservative_surface_bounds[1][1]
                    );
                    for y in [solid.min[1] + 0.00001, solid.max[1] - 0.00001] {
                        assert_ne!(
                            world.material(Cell::from_meters([
                                (cx as f64 + 0.5) * VOXEL_SIZE,
                                y,
                                (cz as f64 + 0.5) * VOXEL_SIZE
                            ])),
                            Material::Air
                        );
                    }
                }
            }
        }
        let unknown = build(
            &world,
            TileKey {
                min: [1_i64 << 33, 0],
                width_cells: 65_536,
                step_cells: 8192,
                edge_steps: [4096; 4],
                source_revision: 0,
            },
            || false,
        )
        .unwrap();
        assert!(unknown.guaranteed_solid_box.is_none());
        let envelope = frontier_world::summary::fbm_height_envelope(world.fbm_terrain().unwrap());
        assert_eq!(unknown.conservative_surface_bounds[0][1], envelope[0]);
        assert_eq!(unknown.conservative_surface_bounds[1][1], envelope[1]);
    }
    #[test]
    fn affine_fan_certificate_encloses_exhaustive_children_without_parent_native_scan() {
        use frontier_world::FbmRecipe;
        for recipe in [FbmRecipe::Legacy, FbmRecipe::Regional] {
            let world = World::fbm(
                7,
                FbmParams {
                    recipe,
                    ..Default::default()
                },
            )
            .unwrap();
            let parent = build(
                &world,
                TileKey {
                    min: [0, 0],
                    width_cells: 256,
                    step_cells: 32,
                    edge_steps: [32; 4],
                    source_revision: 0,
                },
                || false,
            )
            .unwrap();
            assert!(parent.certified_error_m.is_none());
            let aa = parent
                .affine_error_m
                .expect("all64quadforms cover actual fan triangles");
            assert_eq!(parent.affine_stats.quad_bounds, 64);
            assert_eq!(parent.affine_stats.unavailable_quads, 0);
            assert!(
                parent.sampled_columns < 10_000,
                "analytic-first cannot enumerate native parent"
            );
            assert!(parent.best_certified_vertical_error_m.unwrap() <= aa);
            let range = parent.affine_height_range_m.unwrap();
            for z in [0, 128] {
                for x in [0, 128] {
                    // These four exact children have IDENTICAL fan geometry and
                    // continuous shared edges to the analytic parent partition.
                    let child = build(
                        &world,
                        TileKey {
                            min: [x, z],
                            width_cells: 128,
                            step_cells: 32,
                            edge_steps: [32; 4],
                            source_revision: 0,
                        },
                        || false,
                    )
                    .unwrap();
                    assert!(aa + 1e-8 >= child.certified_error_m.unwrap());
                    assert_eq!(
                        child.affine_stats.quad_bounds, 0,
                        "exact8m tiles skip redundant AA"
                    );
                    let exact = child.exact_height_range_m.unwrap();
                    assert!(range[0] <= exact[0] + 1e-8 && range[1] >= exact[1] - 1e-8);
                }
            }
            let solid = parent.guaranteed_solid_box.unwrap();
            assert!(solid.max[1] <= parent.bounds[0][1]);
            for z in (0..256).step_by(8) {
                for x in (0..256).step_by(8) {
                    assert!(solid.max[1] <= world.column(x, z).height_m());
                }
            }
        }
    }
    #[test]
    fn incomplete_affine_budget_is_discarded_in_favor_of_full_global_certificate() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let tile = build(
            &world,
            TileKey {
                min: [0, 0],
                width_cells: 256,
                step_cells: 8,
                edge_steps: [8; 4],
                source_revision: 0,
            },
            || false,
        )
        .unwrap();
        assert!(tile.certified_error_m.is_none());
        assert!(tile.affine_error_m.is_none());
        assert!(tile.affine_height_range_m.is_none());
        assert_eq!(tile.affine_stats.quad_bounds, MAX_AFFINE_QUADS);
        assert_eq!(tile.affine_stats.unavailable_quads, 1024 - MAX_AFFINE_QUADS);
        assert_eq!(tile.best_certified_vertical_error_m, tile.global_error_m);
    }
    #[test]
    fn every_proxy_triangle_has_one_profile_id_after_edge_stitching_and_compaction() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        for (step, edges) in [(4, [1; 4]), (8, [1, 2, 4, 8]), (16, [4, 8, 16, 2])] {
            let tile = build(
                &world,
                TileKey {
                    edge_steps: edges,
                    ..key(step)
                },
                || false,
            )
            .unwrap();
            for tri in tile.mesh.indices.chunks_exact(3) {
                let id = tile.face_profile_indices[tri[0] as usize];
                assert!(tri
                    .iter()
                    .all(|&v| tile.face_profile_indices[v as usize] == id));
            }
        }
    }
    #[test]
    fn continuous_boundary_certificate_includes_lower_outside_wall_endpoint() {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        for native_edge in [false, true] {
            let mut samples = SourceSamples::new(0);
            let key = TileKey {
                min: [0, 0],
                width_cells: 2,
                step_cells: 2,
                edge_steps: if native_edge { [2, 2, 1, 2] } else { [2; 4] },
                source_revision: 0,
            };
            let mut g = Generator {
                defer_profiles: false,
                experiment: None,
                world: &world,
                key,
                samples: &mut samples,
                heights: LocalColumns::new(key.width_cells),
                mesh: Mesh {
                    vertices: vec![],
                    indices: vec![],
                    origin: Cell::default(),
                    exposed_native_faces: 0,
                },
                empirical_error_m: 0.,
                affine_error: 0.,
                affine_range: [f64::INFINITY, f64::NEG_INFINITY],
                affine_complete: true,
                affine_stats: AffineStats::default(),
                face_profiles: vec![[[0.; 4]; 5]],
                spatial_nodes: HashMap::new(),
                face_profile_indices: vec![],
                appearance: FaceAppearance::default(),
            };
            // Flat owned columns and flat coarse anchors hide a one-column
            // valley just OUTSIDE +X. The inside column owns its tall wall.
            for z in -1..=3 {
                for x in -1..=3 {
                    g.heights.insert(
                        [x, z],
                        NativeColumn {
                            height: if x == 2 && z == 1 { 0 } else { 10 },
                            bands: [Material::Grass, Material::Soil, Material::Stone],
                        },
                    );
                }
            }
            g.patch(0, 0, 2).unwrap();
            let proxy_triangles = g.mesh.indices.len() / 3;
            if native_edge {
                g.boundary_face(0, true, 1).unwrap();
            }
            let error = certify(&mut g, proxy_triangles, &mut || false).unwrap();
            if native_edge {
                assert_eq!(
                    error, 0.,
                    "explicit exact wall needs no extra proxy deviation"
                );
            } else {
                assert_eq!(
                    error,
                    10. * VOXEL_SIZE,
                    "outside endpoint must participate in per-tile wall certificate"
                );
            }
        }
    }
    #[test]
    fn geometry_builder_rejects_coarse_edges_at_each_explicit_volume_boundary() {
        let world = World::fbm_topology(7, FbmParams::default()).unwrap();
        for (min, edge) in [
            ([128, -16], 0),
            ([-16, -160], 1),
            ([-160, -16], 2),
            ([-16, 128], 3),
        ] {
            let mut k = TileKey { min, ..key(8) };
            k.edge_steps[edge] = 8;
            let error = build(&world, k, || false)
                .err()
                .expect("owner join must be native independent of caller planner");
            assert!(error.contains("owner boundary requires native edge stride 1"));
        }
    }
    #[test]
    fn native_direction_bins_preserve_order_and_prove_only_whole_bin_backfaces() {
        let world = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 40.,
                ..Default::default()
            },
        )
        .unwrap();
        let tile = build(&world, key(1), || false).unwrap();
        let mut next = 0;
        assert!(!tile.native_bins.is_empty() && tile.native_bins.len() <= 6);
        for &bin in &tile.native_bins {
            assert_eq!(bin.first_quad, next);
            next += bin.quad_count;
            let quads = &tile.native_quads[bin.first_quad as usize..next as usize];
            assert!(quads.iter().all(|q| q.axis_sign == bin.axis_sign));
            let axis = (bin.axis_sign & 3) as usize;
            assert_eq!(
                bin.min_plane,
                quads
                    .iter()
                    .map(|q| q.anchor[axis])
                    .fold(f32::INFINITY, f32::min)
            );
            assert_eq!(
                bin.max_plane,
                quads
                    .iter()
                    .map(|q| q.anchor[axis])
                    .fold(f32::NEG_INFINITY, f32::max)
            );
            let mut eye = [0.; 3];
            eye[axis] = if bin.axis_sign & 4 != 0 {
                bin.max_plane as f64 + 1.
            } else {
                bin.min_plane as f64 - 1.
            };
            assert!(bin.backfacing(eye, VOXEL_SIZE / 1024.));
            for q in quads {
                let normal = if q.axis_sign & 4 != 0 { -1. } else { 1. };
                assert!(normal * (eye[axis] - q.anchor[axis] as f64) < 0.);
            }
            eye[axis] = (bin.min_plane as f64 + bin.max_plane as f64) / 2.;
            assert!(!bin.backfacing(eye, 0.));
            assert!(!bin.backfacing([f64::NAN; 3], 0.));
        }
        assert_eq!(next as usize, tile.native_quads.len());
        let grazing = NativeBin {
            axis_sign: 0,
            first_quad: 0,
            quad_count: 1,
            min_plane: 1.,
            max_plane: 1.,
        };
        assert!(!grazing.backfacing([1. - 1e-6, 0., 0.], 0.));
        assert!(!grazing.backfacing([0.5, 100_000., 0.], 0.));
        assert!(grazing.backfacing([-1., 0., 0.], 0.));
    }
    #[test]
    fn native_extraction_reuses_proxy_columns_without_changing_quad_records() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let mut cache = GeometryCache::new(world.clone(), 128 * 1024 * 1024);
        cache.get_or_build(key(8), || false).unwrap();
        let before = cache.stats().generated_columns;
        let native = cache.get_or_build(key(1), || false).unwrap();
        assert!(
            cache.stats().generated_columns - before <= 4,
            "only previously unsampled corner halo columns may be generated"
        );
        let direct = build(&world, key(1), || false).unwrap();
        assert_eq!(
            bytemuck::cast_slice::<NativeQuad, u8>(&native.native_quads),
            bytemuck::cast_slice::<NativeQuad, u8>(&direct.native_quads)
        );
        assert_eq!(native.face_profiles, direct.face_profiles);
        assert_eq!(native.native_bins, direct.native_bins);
        assert_eq!(native.best_certified_vertical_error_m, Some(0.));
        assert_eq!(native.bounds, direct.bounds);
    }
    #[test]
    fn source_budget_override_is_bounded_and_preserves_total_reservation() {
        let mib = 1024 * 1024;
        for total in [0, 1, 31, 256 * 1024, 128 * mib, 512 * mib, usize::MAX] {
            let default = (total / 8).min(32 * mib);
            for invalid in [None, Some(""), Some("oops"), Some("-1"), Some("1.5")] {
                assert_eq!(source_budget_for(total, invalid), default);
            }
            for input in ["0", "1", "64", "128", "512"] {
                let source = source_budget_for(total, Some(input));
                assert!(source <= total / 2);
                assert_eq!(source + (total - source), total);
                assert_eq!(
                    source,
                    input
                        .parse::<usize>()
                        .unwrap()
                        .saturating_mul(mib)
                        .min(total / 2)
                );
            }
            assert_eq!(
                source_budget_for(total, Some(&usize::MAX.to_string())),
                total / 2
            );
            assert_eq!(source_budget_for(total, Some("0")), 0);
        }
        assert_eq!(source_budget_for(512 * mib, None), 32 * mib);
        assert_eq!(source_budget_for(512 * mib, Some("64")), 64 * mib);
        assert_eq!(source_budget_for(512 * mib, Some("128")), 128 * mib);
        // Decimal values outside usize are invalid, rather than wrapping.
        assert_eq!(
            source_budget_for(512 * mib, Some("999999999999999999999999999999999999")),
            32 * mib
        );
    }
    #[test]
    fn full_source_reservation_bounds_combined_cache_peak_during_later_growth() {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let total = 256 * 1024;
        let mut cache = GeometryCache::new(world, total);
        assert_eq!(cache.mesh_budget_bytes() + cache.source_budget, total);
        // Tiny disjoint jobs allocate very few source values. Force source
        // eviction between them, modeling a nearly empty source cache while
        // cached geometry accumulates. Meshes must not steal that free reserve.
        for i in 0..1200 {
            cache.samples.clear();
            cache
                .get_or_build(
                    TileKey {
                        min: [2 * i + 1, 1],
                        width_cells: 1,
                        step_cells: 1,
                        edge_steps: [1; 4],
                        source_revision: 0,
                    },
                    || false,
                )
                .unwrap();
            assert!(cache.mesh_bytes <= cache.mesh_budget_bytes());
        }
        let mesh_during_build = cache.mesh_bytes + cache.lru_metadata_bytes();
        assert!(mesh_during_build > cache.mesh_budget_bytes() * 9 / 10);
        cache.samples.clear();
        cache
            .get_or_build(
                TileKey {
                    min: [-128, -128],
                    // 64² canonical top records fit this deliberately tiny
                    // mesh budget; the source halo still fills its reserve.
                    width_cells: 64,
                    step_cells: 1,
                    edge_steps: [1; 4],
                    source_revision: 0,
                },
                || false,
            )
            .unwrap();
        let source_peak = cache.samples.pages.stats().peak_allocated_bytes;
        assert!(
            source_peak > cache.source_budget / 2,
            "large native build must exercise source growth"
        );
        // PageCache records all old+new allocation overlap. Mesh eviction only
        // follows the build, so this sum bounds the complete in-build cache peak.
        assert!(mesh_during_build + source_peak <= total);
        assert!(cache.mesh_bytes + source_peak <= total);
        assert!(cache.stats().peak_bytes <= total && cache.stats().resident_bytes <= total);
        assert!(cache.stats().evictions > 0);
    }
    #[test]
    fn shared_near_profiles_match_native_cache_and_preserve_proxy_geometry() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let k = TileKey {
            min: [-16, -32],
            width_cells: 16,
            step_cells: 4,
            edge_steps: [1; 4],
            source_revision: 0,
        };
        let native = column_mesh::build(
            &world,
            Cell {
                x: k.min[0],
                y: 0,
                z: k.min[1],
            },
            [16; 2],
        )
        .unwrap();
        let profile_key = ProfileKey {
            min: k.min,
            size: 16,
            stride: 1,
        };
        let entry = native_profile_entry::<true>(
            world.fbm_terrain().unwrap(),
            &native.heights,
            profile_key,
            [k.min[0] + 16, k.min[1] + 16],
            true,
        )
        .unwrap();
        // Exercise both a native-populated cache and the direct proxy painter.
        for cached in [false, true] {
            let mut samples = SourceSamples::new(8 * 1024 * 1024);
            if cached {
                samples.pages.store_profile(entry);
            }
            let mut g = Generator {
                defer_profiles: false,
                experiment: Some(SmallTileExperiment::SharedNearAppearance),
                world: &world,
                samples: &mut samples,
                key: k,
                heights: LocalColumns::new(k.width_cells),
                mesh: Mesh {
                    vertices: vec![],
                    indices: vec![],
                    origin: Cell::default(),
                    exposed_native_faces: 0,
                },
                empirical_error_m: 0.,
                affine_error: 0.,
                affine_range: [f64::INFINITY, f64::NEG_INFINITY],
                affine_complete: true,
                affine_stats: AffineStats::default(),
                face_profiles: vec![[[0.; 4]; 5]],
                spatial_nodes: HashMap::new(),
                face_profile_indices: vec![],
                appearance: FaceAppearance::default(),
            };
            let first = g.shared_near_profile(0, 0, 4).unwrap().unwrap();
            let second = g.shared_near_profile(4, 8, 4).unwrap().unwrap();
            assert_eq!(first, second);
            assert_eq!(g.face_profiles.len(), 2);
            assert_eq!(
                g.face_profiles[first as usize].map(|a| a.map(f32::to_bits)),
                entry.value.map(|a| a.map(f32::to_bits))
            );
            for p in entry.extrema {
                assert!(g.heights.get(&[p[0] - k.min[0], p[1] - k.min[1]]).is_some());
            }
            assert!(g.shared_near_profile(12, 0, 8).unwrap().is_none());
            assert!(g.shared_near_profile(0, 0, 32).unwrap().is_none());
            // Distant fine-step tiles must keep bounded sampled support.
            let width = g.key.width_cells;
            g.key.width_cells = 1024;
            assert!(g.shared_near_profile(0, 0, 16).unwrap().is_none());
            g.key.width_cells = width;
            assert_eq!(
                g.appearance.area_m2[0],
                entry.value[0][3] as f64 * VOXEL_SIZE * VOXEL_SIZE / 8.
            );
        }
        let baseline =
            small_tile_experiment(&world, k, Some(SmallTileExperiment::AffineExactAppearance)).0;
        let shared =
            small_tile_experiment(&world, k, Some(SmallTileExperiment::SharedNearAppearance)).0;
        let geometry_bits = |tile: &GeometryTile| {
            proxy_primitive_bits(tile)
                .into_iter()
                .map(|tri| {
                    tri.map(|mut v| {
                        v[9] = 0;
                        v
                    })
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(geometry_bits(&baseline), geometry_bits(&shared));
        assert_eq!(baseline.empirical_error_m, shared.empirical_error_m);
        assert_eq!(baseline.affine_error_m, shared.affine_error_m);
    }
    fn small_tile_experiment(
        world: &World,
        key: TileKey,
        mode: Option<SmallTileExperiment>,
    ) -> (GeometryTile, u64, f64) {
        let started = std::time::Instant::now();
        let mut samples = SourceSamples::new(32 * 1024 * 1024);
        samples.begin_job();
        let mut g = Generator {
            defer_profiles: false,
            experiment: mode,
            world,
            key,
            samples: &mut samples,
            heights: LocalColumns::new(key.width_cells),
            mesh: Mesh {
                vertices: vec![],
                indices: vec![],
                origin: Cell {
                    x: key.min[0],
                    y: 0,
                    z: key.min[1],
                },
                exposed_native_faces: 0,
            },
            empirical_error_m: 0.,
            affine_error: 0.,
            affine_range: [f64::INFINITY, f64::NEG_INFINITY],
            affine_complete: true,
            affine_stats: AffineStats::default(),
            face_profiles: vec![[[0.; 4]; 5]],
            spatial_nodes: HashMap::new(),
            face_profile_indices: vec![],
            appearance: FaceAppearance {
                available: true,
                exhaustive: mode != Some(SmallTileExperiment::AffineSampledAppearance),
                ..Default::default()
            },
        };
        let w = key.width_cells as i64;
        let step = key.step_cells as i64;
        for z in (0..w).step_by(step as usize) {
            for x in (0..w).step_by(step as usize) {
                g.patch(x, z, step).unwrap();
            }
        }
        let triangles = g.mesh.indices.len() / 3;
        for (edge, axis, positive) in [(0, 0, false), (1, 2, true), (2, 0, true), (3, 2, false)] {
            if key.edge_steps[edge] == 1 {
                for t in 0..w {
                    g.boundary_face(axis, positive, t).unwrap();
                }
            }
        }
        g.face_profile_indices.resize(g.mesh.vertices.len(), 0);
        let aa = if g.affine_complete && g.affine_stats.quad_bounds > 0 {
            Some(g.affine_error)
        } else {
            None
        };
        let local = if mode.is_none()
            || mode == Some(SmallTileExperiment::SharedNearAppearance)
            || aa.is_none()
        {
            Some(certify(&mut g, triangles, &mut || false).unwrap())
        } else {
            None
        };
        let exact_range = if local.is_some() {
            let mut r = [f64::INFINITY, f64::NEG_INFINITY];
            for c in g.heights.values() {
                let h = c.height as f64 * VOXEL_SIZE;
                r[0] = r[0].min(h);
                r[1] = r[1].max(h);
            }
            Some(r)
        } else {
            None
        };
        let columns = g.heights.len();
        let generated = g.samples.generated;
        let stats = g.affine_stats;
        let ar = g.affine_range;
        let mut tile = finish(key, g.mesh, columns, g.empirical_error_m, false);
        tile.face_profiles = g.face_profiles;
        tile.face_profile_indices = g.face_profile_indices;
        tile.appearance = g.appearance;
        tile.certified_error_m = local;
        tile.affine_error_m = aa;
        tile.affine_stats = stats;
        tile.exact_height_range_m = exact_range;
        if aa.is_some() {
            tile.affine_height_range_m = Some(ar);
        }
        compact_vertices(&mut tile);
        configure_occlusion_bounds(world, &mut tile);
        (tile, generated, started.elapsed().as_secs_f64() * 1000.)
    }
    #[test]
    #[ignore = "bounded CPU experiment; run only while hardware measurement is idle"]
    fn compare_small_tile_affine_first_primary() {
        let world = World::fbm(
            7,
            FbmParams {
                recipe: frontier_world::FbmRecipe::Legacy,
                horizontal_scale: 1800.,
                amplitude: 1000.,
                octaves: 8,
                roughness: 0.5,
            },
        )
        .unwrap();
        let mut camera = crate::Camera::fixture();
        camera.far = 300.;
        camera.fov_y = 60.;
        camera.set_explicit_pose(
            Some([0., 94.8875, 0.]),
            Some([
                -8.731370115810932e-8,
                94.83756238296628,
                -0.9987523555755615,
            ]),
        );
        let axes = [
            glam::DVec3::Y,
            glam::DVec3::X,
            -glam::DVec3::X,
            glam::DVec3::Z,
            -glam::DVec3::Z,
        ];
        let light = |n: glam::DVec3| {
            let sky = 0.5 + 0.5 * n.y;
            glam::DVec3::new(0.12, 0.105, 0.085).lerp(glam::DVec3::new(0.28, 0.33, 0.40), sky)
                + glam::DVec3::new(0.78, 0.70, 0.58)
                    * n.dot(glam::DVec3::new(-0.5, 0.8, -0.3).normalize()).max(0.)
        };
        let radiance = |p: &[[f32; 4]; 5], view: glam::DVec3| {
            let mut rgb = glam::DVec3::ZERO;
            let mut area = 0.;
            for a in 0..5 {
                let weight = axes[a].dot(view).max(0.);
                rgb += glam::DVec3::new(p[a][0] as f64, p[a][1] as f64, p[a][2] as f64)
                    * light(axes[a])
                    * weight;
                area += p[a][3] as f64 * weight;
            }
            if area > 1e-8 {
                Some(rgb / area)
            } else {
                None
            }
        };
        let mut report=String::from("x_m,z_m,step,mode,ms,generated,error_m,pixels,aa_forms,aa_ms,appearance_rmse,appearance_max
");
        for min_m in [[0, -32], [-16, -96], [32, -192]] {
            for step in [8, 16, 32] {
                let key = TileKey {
                    min: min_m.map(|v| v * 16),
                    width_cells: 128,
                    step_cells: step,
                    edge_steps: [step; 4],
                    source_revision: 0,
                };
                let (exact, generated, ms) = small_tile_experiment(&world, key, None);
                let exact_e = exact.best_certified_vertical_error_m.unwrap();
                let exact_px = crate::adaptive_stream::projected_cached_vertical_error(
                    exact_e,
                    exact.bounds,
                    glam::DVec3::new(
                        exact.mesh.origin.x as f64,
                        exact.mesh.origin.y as f64,
                        exact.mesh.origin.z as f64,
                    ) * VOXEL_SIZE,
                    &camera,
                    1920,
                    1080,
                );
                report.push_str(&format!("{},{},{step},exhaustive,{ms:.4},{generated},{exact_e:.6},{exact_px:.4},0,0,0,0\n",min_m[0],min_m[1]));
                for mode in [
                    SmallTileExperiment::AffineExactAppearance,
                    SmallTileExperiment::AffineSampledAppearance,
                ] {
                    let (tile, generated, ms) = small_tile_experiment(&world, key, Some(mode));
                    let e = tile.best_certified_vertical_error_m.unwrap();
                    let px = crate::adaptive_stream::projected_cached_vertical_error(
                        e,
                        tile.bounds,
                        glam::DVec3::new(
                            tile.mesh.origin.x as f64,
                            tile.mesh.origin.y as f64,
                            tile.mesh.origin.z as f64,
                        ) * VOXEL_SIZE,
                        &camera,
                        1920,
                        1080,
                    );
                    assert_eq!(tile.mesh.indices, exact.mesh.indices);
                    assert_eq!(
                        bytemuck::cast_slice::<Vertex, u8>(&tile.mesh.vertices),
                        bytemuck::cast_slice::<Vertex, u8>(&exact.mesh.vertices)
                    );
                    assert!(
                        tile.affine_error_m.unwrap() + 1e-8 >= exact.certified_error_m.unwrap()
                    );
                    let mut sum = 0.;
                    let mut count = 0;
                    let mut max = 0.0_f64;
                    for (i, (a, b)) in exact
                        .face_profiles
                        .iter()
                        .zip(&tile.face_profiles)
                        .enumerate()
                        .skip(1)
                    {
                        let patch = i - 1;
                        let n = 128 / step as usize;
                        let center = glam::DVec3::new(
                            (key.min[0] as f64
                                + (patch % n) as f64 * step as f64
                                + step as f64 / 2.)
                                * VOXEL_SIZE,
                            (exact.bounds[0][1] + exact.bounds[1][1]) / 2.,
                            (key.min[1] as f64
                                + (patch / n) as f64 * step as f64
                                + step as f64 / 2.)
                                * VOXEL_SIZE,
                        );
                        let view = (camera.position - center).normalize();
                        if let (Some(a), Some(b)) = (radiance(a, view), radiance(b, view)) {
                            let d = (a - b).abs();
                            sum += d.length_squared();
                            count += 3;
                            max = max.max(d.max_element());
                        }
                    }
                    let rmse = (sum / (count.max(1) as f64)).sqrt();
                    report.push_str(&format!("{},{},{step},{mode:?},{ms:.4},{generated},{e:.6},{px:.4},{},{:.4},{rmse:.8},{max:.8}\n",min_m[0],min_m[1],tile.affine_stats.quad_bounds,tile.affine_stats.cpu_ms));
                }
            }
        }
        report = report.replace(
            "\n", "
",
        );
        std::fs::write("/tmp/frontier-small-tile-affine.csv", &report).unwrap();
        println!("{report}");
    }
    #[test]
    fn thirty_two_meter_cluster_uses_analytic_first_without_native_scan() {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let tile = build(
            &world,
            TileKey {
                min: [0, 0],
                width_cells: 512,
                step_cells: 64,
                edge_steps: [64; 4],
                source_revision: 0,
            },
            || false,
        )
        .unwrap();
        assert!(tile.certified_error_m.is_none());
        assert!(tile.affine_error_m.unwrap() <= VOXEL_SIZE + 1e-8);
        assert_eq!(tile.affine_stats.quad_bounds, 64);
        assert_eq!(tile.affine_stats.unavailable_quads, 0);
        assert!(tile.sampled_columns < 10_000);
        assert!(tile.sampled_columns <= MAX_SAMPLES);
        assert!(tile.bytes < 1024 * 1024);
    }
    #[test]
    fn kilometer_cluster_has_bounded_source_work_and_shared_coarse_anchors() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let key = TileKey {
            min: [0, 0],
            width_cells: 65_536,
            step_cells: 8192,
            edge_steps: [4096; 4],
            source_revision: 0,
        };
        let tile = build(&world, key, || false).unwrap();
        assert!(tile.sampled_columns < 10_000);
        assert!(tile.mesh.vertices.len() < 10_000);
        assert!(tile.certified_error_m.is_none());
        assert!(!tile.appearance.exhaustive);
    }
    #[test]
    fn integrated_gpu_profiles_preserve_geometry_and_skip_cpu_painter() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let key = TileKey {
            min: [-512, -512],
            width_cells: 128,
            step_cells: 16,
            edge_steps: [1; 4],
            source_revision: 0,
        };
        let mut baseline_samples = SourceSamples::new(8 * 1024 * 1024);
        baseline_samples.gpu_profiles.enabled = false;
        let baseline = build_cached(&world, key, || false, &mut baseline_samples).unwrap();
        let mut candidate_samples = SourceSamples::new(8 * 1024 * 1024);
        candidate_samples.gpu_profiles.enabled = true;
        let mut candidate = build_cached(&world, key, || false, &mut candidate_samples).unwrap();
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&candidate.mesh.vertices),
            bytemuck::cast_slice::<_, u8>(&baseline.mesh.vertices)
        );
        assert_eq!(candidate.mesh.indices, baseline.mesh.indices);
        assert_eq!(
            candidate.face_profile_indices,
            baseline.face_profile_indices
        );
        assert_eq!(candidate.certified_error_m, baseline.certified_error_m);
        assert_eq!(
            candidate.exact_height_range_m,
            baseline.exact_height_range_m
        );
        assert_eq!(candidate.bounds, baseline.bounds);
        assert_eq!(
            candidate.conservative_surface_bounds,
            baseline.conservative_surface_bounds
        );
        assert_eq!(candidate.sampled_columns, baseline.sampled_columns);
        assert!(baseline.appearance.available && baseline.appearance.exhaustive);
        assert!(!candidate.appearance.available && !candidate.appearance.exhaustive);
        assert!(candidate
            .face_profiles
            .iter()
            .flatten()
            .flatten()
            .all(|&v| v == 0.));
        let payload = candidate
            .gpu_profiles
            .as_ref()
            .expect("eligible profile dispatch");
        assert_eq!(payload.profile_count(), baseline.face_profiles.len());
        let oracle = crate::gpu_ordered_profiles::cpu_profiles(
            std::slice::from_ref(&payload.page),
            &payload.jobs,
        )
        .unwrap();
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&oracle),
            bytemuck::cast_slice::<_, u8>(&baseline.face_profiles[1..])
        );
        assert_eq!(
            candidate_samples.pages.snapshot().appearance_cache_misses,
            0
        );
        assert!(baseline_samples.pages.snapshot().appearance_cache_misses > 0);
        assert_eq!(candidate_samples.gpu_profiles.deferred_tiles, 1);
        assert_eq!(candidate_samples.gpu_profiles.deferred_jobs, 64);
        assert_eq!(candidate_samples.gpu_profiles.fallback_tiles, 0);
        let original_bytes = candidate.bytes;
        let payload_bytes = payload.payload_bytes();
        pack_proxy_triangles(&mut candidate).unwrap();
        assert!(candidate.bytes <= original_bytes);
        assert!(candidate.bytes >= payload_bytes);
        assert!(candidate
            .proxy_triangles
            .iter()
            .any(|q| q.profile_index == 0));
    }

    #[test]
    fn integrated_gpu_profiles_keep_ineligible_ownership_on_cpu() {
        let world = World::fbm_topology(7, FbmParams::default()).unwrap();
        let mut samples = SourceSamples::new(8 * 1024 * 1024);
        samples.gpu_profiles.enabled = true;
        let tile = build_cached(
            &world,
            TileKey {
                min: [256, 256],
                width_cells: 128,
                step_cells: 16,
                edge_steps: [1; 4],
                source_revision: 0,
            },
            || false,
            &mut samples,
        )
        .unwrap();
        assert!(tile.gpu_profiles.is_none());
        assert!(tile.appearance.available);
        assert_eq!(samples.gpu_profiles.eligible_tiles, 0);
        let plain = World::fbm(7, FbmParams::default()).unwrap();
        for (width_cells, step_cells) in [(32, 8), (128, 4), (128, 1), (256, 16)] {
            assert!(!integrated_profiles::eligible(
                &plain,
                TileKey {
                    width_cells,
                    step_cells,
                    ..key(8)
                }
            ));
        }
    }

    #[test]
    fn integrated_gpu_profile_relief_falls_back_to_cpu() {
        let world = World::fbm(
            7,
            FbmParams {
                amplitude: 100_000.,
                ..FbmParams::default()
            },
        )
        .unwrap();
        let key = TileKey {
            min: [-512, -512],
            width_cells: 128,
            step_cells: 16,
            edge_steps: [16; 4],
            source_revision: 0,
        };
        let mut samples = SourceSamples::new(8 * 1024 * 1024);
        samples.gpu_profiles.enabled = true;
        let tile = build_cached(&world, key, || false, &mut samples).unwrap();
        assert!(tile.gpu_profiles.is_none());
        assert!(tile.appearance.available && tile.appearance.exhaustive);
        assert_eq!(samples.gpu_profiles.eligible_tiles, 1);
        assert_eq!(samples.gpu_profiles.fallback_tiles, 1);
        assert!(samples.pages.snapshot().appearance_cache_misses > 0);
        let mut baseline_samples = SourceSamples::new(8 * 1024 * 1024);
        baseline_samples.gpu_profiles.enabled = false;
        let baseline = build_cached(&world, key, || false, &mut baseline_samples).unwrap();
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&tile.face_profiles),
            bytemuck::cast_slice::<_, u8>(&baseline.face_profiles)
        );
    }
}
