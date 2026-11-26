//! Coarse voxel LOD surface for the canonical heightfield. Each rectangle is
//! a run of occupied columns with one common top. Side faces are emitted once,
//! by the higher run, and no interior volume is allocated.

use super::{
    profile_probe_bands, Generator, GeometryTile, NativeBin, NativeColumn, NativeQuad, SourceStage,
    TileKey, MAX_AFFINE_QUADS, MAX_CERTIFICATE_WIDTH, MAX_VERTICES,
};
use crate::spatial_appearance::Profile;
use frontier_world::{
    affine_bounds::{BoundBudget, ColumnBox},
    voxel_size, FbmRecipe, Material, World,
};
use std::{
    collections::BTreeMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Arc, Mutex, OnceLock,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Surface,
    Voxy,
    Seed,
}

static MODE: OnceLock<Mode> = OnceLock::new();

impl Mode {
    fn resolve(value: Option<&str>, default: Self) -> Self {
        match value {
            Some("surface") => Self::Surface,
            Some("voxy") => Self::Voxy,
            Some("voxy-seed") => Self::Seed,
            _ => default,
        }
    }

    fn from_environment(default: Self) -> Self {
        Self::resolve(
            std::env::var("FRONTIER_DISTANT_TERRAIN").ok().as_deref(),
            default,
        )
    }

    pub(crate) fn initialize_application() {
        MODE.get_or_init(|| Self::from_environment(Self::Seed));
    }

    pub(crate) fn current() -> Self {
        // Offline construction and fixtures can still choose their control
        // representation without inheriting the interactive launcher default.
        *MODE.get_or_init(|| Self::from_environment(Self::Surface))
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Surface => "surface",
            Self::Voxy => "voxy",
            Self::Seed => "voxy-seed",
        }
    }
}

pub(crate) fn enabled() -> bool {
    Mode::current() != Mode::Surface
}

#[derive(Clone, Copy)]
struct Run {
    top: i64,
    ground: i64,
    top_material: Material,
    deep_material: Material,
    profile: u32,
}

#[derive(Clone)]
struct Edge {
    span: Range<i64>,
    run: Run,
}

type Plane = (usize, i64);
type Edges = BTreeMap<Plane, [Vec<Edge>; 2]>;

/// Unmerged faces have patch-local profile slots. These can be reused when a
/// neighboring tile changes pitch, before the new tile's profiles are assigned.
struct InteriorPatch {
    run: Run,
    center_height: i64,
    roof: Option<NativeQuad>,
}

struct Interior {
    patches: Vec<InteriorPatch>,
    walls: Vec<NativeQuad>,
}

type InteriorEntry = Option<(TileKey, Arc<Interior>)>;

pub(super) struct InteriorCache {
    slots: Vec<Mutex<InteriorEntry>>,
    pub(super) reserved_bytes: usize,
    pub(super) hits: AtomicU64,
}

impl InteriorCache {
    pub(super) fn new(budget: usize) -> Option<Self> {
        // Each patch has a roof and at most two regular seams with two material
        // intervals each. Reserve the cap before admitting a cache slot.
        let per_slot = std::mem::size_of::<Mutex<InteriorEntry>>()
            + 128
            + MAX_AFFINE_QUADS as usize
                * (std::mem::size_of::<InteriorPatch>() + 4 * std::mem::size_of::<NativeQuad>());
        let slots = budget.saturating_sub(std::mem::size_of::<Self>()) / per_slot;
        (slots > 0).then(|| Self {
            slots: (0..slots).map(|_| Mutex::new(None)).collect(),
            reserved_bytes: std::mem::size_of::<Self>() + slots * per_slot,
            hits: AtomicU64::new(0),
        })
    }

    fn slot(&self, key: TileKey) -> &Mutex<InteriorEntry> {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        &self.slots[hasher.finish() as usize % self.slots.len()]
    }

    pub(super) fn clear(&self) {
        for slot in &self.slots {
            *slot.lock().unwrap() = None;
        }
    }
}

fn packed_quad(
    anchor: [i64; 3],
    axis: usize,
    positive: bool,
    extent: [i64; 2],
    material: Material,
    profile: u32,
) -> NativeQuad {
    let meters = |c: i64| (c as f64 * voxel_size()) as f32;
    let axes = [(axis + 1) % 3, (axis + 2) % 3];
    let material = TERRAIN_PALETTE
        .iter()
        .position(|&entry| entry == material)
        .expect("all terrain materials have a palette entry");
    NativeQuad {
        anchor: anchor.map(meters),
        extent: std::array::from_fn(|i| {
            meters(anchor[axes[i]] + extent[i]) - meters(anchor[axes[i]])
        }),
        axis_sign: axis as u32 | if positive { 0 } else { 4 },
        material: material as u32,
        profile_index: profile,
    }
}

fn wall_quads(
    axis: usize,
    plane: i64,
    positive: bool,
    span: Range<i64>,
    low: i64,
    high: Run,
) -> impl Iterator<Item = NativeQuad> {
    let ground = high.ground.clamp(low, high.top);
    [
        (low, ground, high.deep_material),
        (ground, high.top, high.top_material),
    ]
    .into_iter()
    .filter_map(move |(lo, hi, material)| {
        if lo == hi {
            return None;
        }
        let (anchor, extent) = if axis == 0 {
            ([plane, lo, span.start], [hi - lo, span.end - span.start])
        } else {
            ([span.start, lo, plane], [span.end - span.start, hi - lo])
        };
        Some(packed_quad(
            anchor,
            axis,
            positive,
            extent,
            material,
            high.profile,
        ))
    })
}

fn roof_quad(x: &Range<i64>, z: &Range<i64>, run: Run) -> NativeQuad {
    packed_quad(
        [x.start, run.top, z.start],
        1,
        true,
        [z.end - z.start, x.end - x.start],
        run.top_material,
        run.profile,
    )
}

fn rect_edges(
    edges: &mut Edges,
    x: Range<i64>,
    z: Range<i64>,
    run: Run,
    interior_width: Option<i64>,
) {
    for (axis, line, span) in [(0, x.clone(), z.clone()), (2, z, x)] {
        for (side, plane) in [line.start, line.end].into_iter().enumerate() {
            if interior_width.is_some_and(|width| plane != 1 && plane != width - 1) {
                continue;
            }
            edges.entry((axis, plane)).or_default()[side].push(Edge {
                span: span.clone(),
                run,
            });
        }
    }
}

/// Keep the horizontal stair grid while changing how coarsely dry roofs round
/// in Y. Shared seams still derive their height from the same sample and pitch.
pub(crate) fn vertical_pitch_divisor(world: &World) -> u32 {
    static DIVISOR: OnceLock<Option<u32>> = OnceLock::new();
    DIVISOR
        .get_or_init(
            || match std::env::var("FRONTIER_COARSE_Y_PITCH_DIVISOR").as_deref() {
                Ok("1") => Some(1),
                Ok("2") => Some(2),
                Ok("4") => Some(4),
                Ok("8") => Some(8),
                _ => None,
            },
        )
        .unwrap_or_else(
            || match world.fbm_terrain().map(|terrain| terrain.params().recipe) {
                Some(
                    FbmRecipe::Alpine
                    | FbmRecipe::Watershed
                    | FbmRecipe::Landscapes
                    | FbmRecipe::Landforms
                    | FbmRecipe::Eroded,
                ) => 4,
                _ => 1,
            },
        )
}

fn round_to_pitch(height: i64, pitch: i64) -> i64 {
    (height + pitch / 2).div_euclid(pitch) * pitch
}

/// Estimate native top and exposed side area from the seed samples. The rise
/// is normalized by the horizontal sample distance before applying paint bands.
fn seed_node(column: NativeColumn, neighbors: [NativeColumn; 4], stride: i64) -> Profile {
    let mut result = [[0.; 4]; 5];
    let [r, g, b] = column.material(column.height - 1).color();
    result[0] = [r, g, b, 1.];
    for (face, neighbor) in result[1..].iter_mut().zip(neighbors) {
        let dry_area = profile_probe_bands(column.ground_height, neighbor.height, stride);
        let water_area = (column.height - column.ground_height.max(neighbor.height)).max(0) as f32
            / stride as f32;
        for (area, material) in dry_area
            .into_iter()
            .chain([water_area])
            .zip(column.bands.into_iter().chain([Material::Water]))
        {
            for (value, color) in face[..3].iter_mut().zip(material.color()) {
                *value += area * color;
            }
            face[3] += area;
        }
    }
    result
}

impl Generator<'_> {
    /// One bounded paint grid for the entire tile. Both the directional terrain
    /// profile and the mesh use canonical seed samples, with no per-face maps.
    /// Ground-cover decoration can bind its existing vegetation map once here.
    fn seed_appearance(&mut self, cancelled: &mut impl FnMut() -> bool) -> Result<u32, String> {
        let width = i64::from(self.key.width_cells);
        let map = crate::appearance_map::layout(self.key.min, width, 8)?;
        let stride = map.spacing_cells;
        let dimension = map.dimension;
        let origin = [
            map.origin_cells[0] - self.key.min[0],
            map.origin_cells[1] - self.key.min[1],
        ];
        self.samples.source_stage = SourceStage::Appearance;
        let started = std::time::Instant::now();
        let mut columns = Vec::with_capacity((dimension + 2).pow(2));
        for z in -1..=dimension as i64 {
            if cancelled() {
                return Err("adaptive geometry cancelled".into());
            }
            for x in -1..=dimension as i64 {
                let p = [origin[0] + x * stride, origin[1] + z * stride];
                columns.push(self.column(p)?);
            }
        }
        let row = dimension + 2;
        let first = self.face_profiles.len();
        for z in 1..=dimension {
            for x in 1..=dimension {
                let i = z * row + x;
                self.face_profiles.push(seed_node(
                    columns[i],
                    [
                        columns[i + 1],
                        columns[i - 1],
                        columns[i + row],
                        columns[i - row],
                    ],
                    stride,
                ));
            }
        }
        let at = self.face_profiles.len() as u32;
        let refs: Vec<_> = (first as u32..at).collect();
        let nodes = self.face_profiles[first..].to_vec();
        self.note_profile_mean(&nodes, (width * width) as f64);
        self.appearance.exhaustive = false;
        self.face_profiles
            .extend(crate::spatial_appearance::map_header(
                origin.map(|v| (v as f64 * voxel_size()) as f32),
                (stride as f64 * voxel_size()) as f32,
                dimension,
                &refs,
                at,
            ));
        self.samples.stages.appearance_ms += started.elapsed().as_secs_f64() * 1000.;
        self.samples.source_stage = SourceStage::Proxy;
        Ok(at)
    }

    fn coarse_run(
        &mut self,
        sample: [i64; 2],
        pitch: i64,
        profile: u32,
    ) -> Result<(Run, i64), String> {
        let column = self.column(sample)?;
        let wet = column.height > column.ground_height;
        let ground = if wet {
            column.ground_height
        } else {
            let vertical_pitch = (pitch / i64::from(vertical_pitch_divisor(self.world))).max(1);
            round_to_pitch(column.ground_height, vertical_pitch)
        };
        Ok((
            Run {
                top: if wet { column.height } else { ground },
                ground,
                top_material: column.material(column.height - 1),
                deep_material: column.bands[2],
                profile,
            },
            column.height,
        ))
    }

    fn append_quads(&mut self, quads: impl IntoIterator<Item = NativeQuad>) -> Result<(), String> {
        for quad in quads {
            if self.coarse_quads.len() >= MAX_VERTICES / 4 {
                return Err("adaptive voxel mesh byte budget exceeded".into());
            }
            self.coarse_quads.push(quad);
        }
        Ok(())
    }

    fn build_interior(
        &mut self,
        width: i64,
        pitch: i64,
        grid: usize,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Interior, String> {
        let mut interior = Interior {
            patches: Vec::<InteriorPatch>::with_capacity(grid * grid),
            walls: Vec::with_capacity(4 * grid * grid),
        };
        for z in (0..width).step_by(pitch as usize) {
            for x in (0..width).step_by(pitch as usize) {
                if cancelled() {
                    return Err("adaptive geometry cancelled".into());
                }
                let index = interior.patches.len();
                let (run, center_height) =
                    self.coarse_run([x + pitch / 2, z + pitch / 2], pitch, index as u32)?;
                let xx = x.max(1)..(x + pitch).min(width - 1);
                let zz = z.max(1)..(z + pitch).min(width - 1);
                let roof = (!xx.is_empty() && !zz.is_empty()).then(|| roof_quad(&xx, &zz, run));
                if roof.is_some() {
                    for (axis, plane, span, delta) in [(0, x, zz, 1), (2, z, xx, grid)] {
                        if plane == 0 {
                            continue;
                        }
                        let neighbor = interior.patches[index - delta].run;
                        if neighbor.top > run.top {
                            interior
                                .walls
                                .extend(wall_quads(axis, plane, true, span, run.top, neighbor));
                        } else if run.top > neighbor.top {
                            interior.walls.extend(wall_quads(
                                axis,
                                plane,
                                false,
                                span,
                                neighbor.top,
                                run,
                            ));
                        }
                    }
                }
                interior.patches.push(InteriorPatch {
                    run,
                    center_height,
                    roof,
                });
            }
        }
        Ok(interior)
    }

    fn interior(
        &mut self,
        width: i64,
        pitch: i64,
        grid: usize,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Arc<Interior>, String> {
        let key = TileKey {
            edge_steps: [0; 4],
            ..self.key
        };
        let cache = self.samples.pages.coarse_interiors.clone();
        if let Some(cache) = &cache {
            let slot = cache.slot(key).lock().unwrap();
            if let Some((_, data)) = slot.as_ref().filter(|(cached, _)| *cached == key) {
                cache.hits.fetch_add(1, Relaxed);
                return Ok(data.clone());
            }
        }
        let data = Arc::new(self.build_interior(width, pitch, grid, cancelled)?);
        if let Some(cache) = &cache {
            *cache.slot(key).lock().unwrap() = Some((key, data.clone()));
        }
        Ok(data)
    }

    fn record_coarse_error(
        &mut self,
        min: [i64; 2],
        pitch: i64,
        first_roof: usize,
        roof_range: [i64; 2],
        cached_bound: Option<Option<frontier_world::affine_bounds::HeightBound>>,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), String> {
        let started = std::time::Instant::now();
        let key = self.key;
        let mut range = [f64::INFINITY, f64::NEG_INFINITY];
        let mut error: f64 = 0.;
        let complete = if key.width_cells <= MAX_CERTIFICATE_WIDTH {
            // These rectangles partition the tile. Exhaustively compare the
            // actual roof of each one, including the thin boundary band.
            for roof in first_roof..self.coarse_quads.len() {
                let quad = self.coarse_quads[roof];
                let cell = |p: f32| (f64::from(p) / voxel_size()).round() as i64;
                let [x0, top, z0] = quad.anchor.map(cell);
                let x1 = x0 + cell(quad.extent[1]);
                let z1 = z0 + cell(quad.extent[0]);
                for z in z0..z1 {
                    if z % 32 == 0 && cancelled() {
                        return Err("adaptive geometry cancelled".into());
                    }
                    for x in x0..x1 {
                        let h = self.height(x, z)? as f64 * voxel_size();
                        range[0] = range[0].min(h);
                        range[1] = range[1].max(h);
                        error = error.max((h - top as f64 * voxel_size()).abs());
                    }
                }
            }
            true
        } else if self.affine_stats.quad_bounds < MAX_AFFINE_QUADS {
            self.affine_stats.quad_bounds += 1;
            let bound = cached_bound.unwrap_or_else(|| {
                let started = std::time::Instant::now();
                let result = self.samples.pages.bound(
                    self.world,
                    ColumnBox {
                        min: [key.min[0] + min[0], key.min[1] + min[1]],
                        size_cells: [pitch as u32; 2],
                    },
                    BoundBudget::default(),
                );
                self.affine_stats.coarse_source_ms += started.elapsed().as_secs_f64() * 1000.;
                result
            });
            let roof_started = std::time::Instant::now();
            if let Some(bound) = bound {
                range = bound.height_range;
                self.affine_stats.noise_cells += bound.noise_cells as u64;
                self.affine_stats.full_range_fallbacks += bound.full_range_fallbacks as u64;
                // A patch whose entire range fits below the accumulated
                // error cannot change the tile's conservative certificate.
                let dominated = bound.roof_range_error(roof_range.map(|y| y as f64 * voxel_size()))
                    <= self.affine_error;
                #[cfg(test)]
                let dominated = dominated && !self.samples.coarse_reference;
                let complete = if dominated {
                    self.affine_stats.coarse_roofs_skipped += 1;
                    true
                } else {
                    let mut complete = true;
                    for quad in &self.coarse_quads[first_roof..] {
                        let [x, y, z] = quad.anchor.map(f64::from);
                        let x = x + key.min[0] as f64 * voxel_size();
                        let z = z + key.min[1] as f64 * voxel_size();
                        let x1 = x + f64::from(quad.extent[1]);
                        let z1 = z + f64::from(quad.extent[0]);
                        if let Some(value) =
                            bound.quad_error([[x, y, z], [x, y, z1], [x1, y, z1], [x1, y, z]])
                        {
                            error = error.max(value);
                        } else {
                            complete = false;
                        }
                    }
                    complete
                };
                self.affine_stats.coarse_roof_ms += roof_started.elapsed().as_secs_f64() * 1000.;
                complete
            } else {
                false
            }
        } else {
            false
        };
        self.affine_stats.cpu_ms += started.elapsed().as_secs_f64() * 1000.;
        if complete {
            self.affine_range[0] = self.affine_range[0].min(range[0]);
            self.affine_range[1] = self.affine_range[1].max(range[1]);
            self.affine_error = self.affine_error.max(error);
        } else {
            self.affine_complete = false;
            self.affine_stats.unavailable_quads += 1;
        }
        Ok(())
    }

    pub(super) fn coarse_surface(
        &mut self,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), String> {
        let key = self.key;
        let width = i64::from(key.width_cells);
        let pitch = i64::from(key.step_cells);
        let grid = key.width_cells / key.step_cells;
        let patches = u64::from(grid) * u64::from(grid);
        let needs_affine = key.width_cells > MAX_CERTIFICATE_WIDTH;
        let can_certify = !needs_affine || patches <= MAX_AFFINE_QUADS;
        // An incomplete certificate is discarded by the caller. Avoid doing
        // the first 256 analyses when the entire tile needs a global fallback.
        if !can_certify {
            self.affine_complete = false;
            self.affine_stats.unavailable_quads = patches;
        }
        let bounds = if needs_affine && can_certify {
            if let Some(cache) = &self.samples.pages.terrain_bounds {
                let started = std::time::Instant::now();
                let bounds = cache.tile(
                    self.world.fbm_terrain().unwrap(),
                    key.min,
                    key.width_cells,
                    key.step_cells,
                    cancelled,
                )?;
                let elapsed = started.elapsed().as_secs_f64() * 1000.;
                self.affine_stats.cpu_ms += elapsed;
                self.affine_stats.coarse_source_ms += elapsed;
                bounds
            } else {
                None
            }
        } else {
            None
        };
        let seed_profile = if Mode::current() == Mode::Seed && key.width_cells >= 128 && pitch >= 4
        {
            Some(self.seed_appearance(cancelled)?)
        } else {
            None
        };
        let use_interior = needs_affine && can_certify;
        #[cfg(test)]
        let use_interior = use_interior && !self.samples.coarse_reference;
        let interior = use_interior
            .then(|| self.interior(width, pitch, grid as usize, cancelled))
            .transpose()?;
        let mut profiles = Vec::with_capacity(interior.as_ref().map_or(0, |i| i.patches.len()));
        let mut edges = Edges::new();
        for z in (0..width).step_by(pitch as usize) {
            for x in (0..width).step_by(pitch as usize) {
                if cancelled() {
                    return Err("adaptive geometry cancelled".into());
                }
                let first_roof = self.coarse_quads.len();
                let profile = match seed_profile {
                    Some(profile) => profile,
                    None => self.patch_appearance(x, z, pitch)?,
                };
                let on_border = [x == 0, z + pitch == width, x + pitch == width, z == 0];
                let index = ((z / pitch) * i64::from(grid) + x / pitch) as usize;
                let cached = interior.as_ref().map(|data| &data.patches[index]);
                let [a, b, c, d] = on_border.map(i64::from);
                let xx = x + a..x + pitch - c;
                let zz = z + d..z + pitch - b;
                let (run, center, roof) = if let Some(patch) = cached {
                    profiles.push(profile);
                    (
                        Run {
                            profile,
                            ..patch.run
                        },
                        patch.center_height,
                        patch.roof.map(|q| NativeQuad {
                            profile_index: profile,
                            ..q
                        }),
                    )
                } else {
                    let (run, center) =
                        self.coarse_run([x + pitch / 2, z + pitch / 2], pitch, profile)?;
                    let roof = (!xx.is_empty() && !zz.is_empty()).then(|| roof_quad(&xx, &zz, run));
                    (run, center, roof)
                };
                let mut top_range = [run.top; 2];
                if let Some(roof) = roof {
                    self.append_quads([roof])?;
                    rect_edges(&mut edges, xx, zz, run, cached.map(|_| width));
                }

                for (edge, on_border) in on_border.into_iter().enumerate() {
                    if !on_border {
                        continue;
                    }
                    let stride = i64::from(key.edge_steps[edge]);
                    let [axis, along] = edge_axes(edge);
                    let plane = if edge == 0 || edge == 3 { 0 } else { width };
                    let inside = if plane == 0 { 0 } else { width - 1 };
                    let t0 = [x, z][along];
                    // The X strips own the corners. At every stride endpoint
                    // both sides use the canonical columns, including corners
                    // where a large tile meets two smaller ones.
                    let valid = if axis == 0 {
                        t0..t0 + pitch
                    } else {
                        x + a..x + pitch - c
                    };
                    for t in (t0..t0 + pitch).step_by(stride as usize) {
                        if cancelled() {
                            return Err("adaptive geometry cancelled".into());
                        }
                        for (span, sample, sample_pitch) in seam_spans(t, stride, plane, inside) {
                            let clipped = span.start.max(valid.start)..span.end.min(valid.end);
                            if clipped.is_empty() {
                                continue;
                            }
                            let mut sample_xz = [0; 2];
                            sample_xz[axis] = sample[0];
                            sample_xz[along] = sample[1];
                            let (run, _) = self.coarse_run(sample_xz, sample_pitch, profile)?;
                            let footprint = if axis == 0 {
                                [inside..inside + 1, clipped]
                            } else {
                                [clipped, inside..inside + 1]
                            };
                            let [xx, zz] = footprint;
                            top_range[0] = top_range[0].min(run.top);
                            top_range[1] = top_range[1].max(run.top);
                            self.append_quads([roof_quad(&xx, &zz, run)])?;
                            rect_edges(&mut edges, xx, zz, run, None);
                        }
                    }
                }
                self.empirical_error_m = self.empirical_error_m.max(
                    (center - top_range[0])
                        .abs()
                        .max((center - top_range[1]).abs()) as f64
                        * voxel_size(),
                );
                if can_certify {
                    let bound = bounds.as_ref().map(|bounds| bounds[index]);
                    self.record_coarse_error(
                        [x, z],
                        pitch,
                        first_roof,
                        top_range,
                        bound,
                        cancelled,
                    )?;
                }
            }
        }
        if let Some(interior) = interior {
            self.append_quads(interior.walls.iter().map(|&q| NativeQuad {
                profile_index: profiles[q.profile_index as usize],
                ..q
            }))?;
        }
        for ((axis, plane), mut sides) in edges {
            if cancelled() {
                return Err("adaptive geometry cancelled".into());
            }
            for edge_list in &mut sides {
                edge_list.sort_unstable_by_key(|e| e.span.start);
            }
            if plane == 0 || plane == width {
                let positive = plane == width;
                let edge = match (axis, positive) {
                    (0, false) => 0,
                    (2, true) => 1,
                    (0, true) => 2,
                    _ => 3,
                };
                let stride = i64::from(key.edge_steps[edge]);
                // Middle spans have identical tops in the two tiles. Only
                // native edge cells need an exposed face across the seam.
                for edge in &sides[usize::from(positive)] {
                    if edge.span.end - edge.span.start != 1 || !seam_native(edge.span.start, stride)
                    {
                        continue;
                    }
                    let t = edge.span.start;
                    let outside = if positive { width } else { -1 };
                    let neighbor = if axis == 0 {
                        self.height(outside, t)?
                    } else {
                        self.height(t, outside)?
                    };
                    if edge.run.top > neighbor {
                        self.append_quads(wall_quads(
                            axis,
                            plane,
                            positive,
                            edge.span.clone(),
                            neighbor,
                            edge.run,
                        ))?;
                    }
                }
                continue;
            }
            let [negative, positive] = &sides;
            let mut i = 0;
            let mut j = 0;
            while let (Some(right), Some(left)) = (negative.get(i), positive.get(j)) {
                let span = right.span.start.max(left.span.start)..right.span.end.min(left.span.end);
                if !span.is_empty() && left.run.top != right.run.top {
                    let outward = left.run.top > right.run.top;
                    let (high, low) = if outward {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    self.append_quads(wall_quads(
                        axis,
                        plane,
                        outward,
                        span,
                        low.run.top,
                        high.run,
                    ))?;
                }
                if right.span.end <= left.span.end {
                    i += 1;
                }
                if left.span.end <= right.span.end {
                    j += 1;
                }
            }
        }
        Ok(())
    }
}

fn edge_axes(edge: usize) -> [usize; 2] {
    if edge % 2 == 0 {
        [0, 1]
    } else {
        [1, 0]
    }
}

fn seam_native(t: i64, stride: i64) -> bool {
    let slot = t.rem_euclid(stride);
    slot == 0 || slot == stride - 1
}

fn seam_spans(
    t: i64,
    stride: i64,
    plane: i64,
    inside: i64,
) -> impl Iterator<Item = (Range<i64>, [i64; 2], i64)> {
    let end = t + stride;
    [
        (t..t + 1, [inside, t], 1),
        (t + 1..end - 1, [plane, t + stride / 2], stride),
        (end - 1..end, [inside, end - 1], 1),
    ]
    .into_iter()
    .take(if stride == 1 { 1 } else { 3 })
    .filter(|(span, _, _)| !span.is_empty())
}

pub(crate) const TERRAIN_PALETTE: [Material; 10] = [
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
];

/// Use the existing instanced axis-face layout, without re-tessellating coarse
/// faces into native cells. Profiles have already been decorated at this point.
pub(super) fn merge_quads(tile: &mut GeometryTile) {
    let quads = &mut tile.native_quads;
    for merge in [0, 1] {
        let order = |q: &NativeQuad| {
            let axis = (q.axis_sign & 3) as usize;
            let u = (axis + 1 + merge) % 3;
            let v = (axis + 1 + (1 - merge)) % 3;
            (
                q.axis_sign,
                q.material,
                q.profile_index,
                q.anchor[axis].to_bits(),
                q.anchor[v].to_bits(),
                q.extent[1 - merge].to_bits(),
                (q.anchor[u] as f64 / voxel_size()).round() as i64,
            )
        };
        quads.sort_unstable_by_key(order);
        quads.dedup_by(|quad, prev| {
            let axis = (quad.axis_sign & 3) as usize;
            let u = (axis + 1 + merge) % 3;
            let (a, b, c, d, e, f, _) = order(prev);
            let (g, h, i, j, k, l, _) = order(quad);
            if (a, b, c, d, e, f) == (g, h, i, j, k, l)
                && prev.anchor[u] + prev.extent[merge] == quad.anchor[u]
            {
                prev.extent[merge] += quad.extent[merge];
                true
            } else {
                false
            }
        });
    }
    quads.shrink_to_fit();
    for (index, quad) in quads.iter().enumerate() {
        let plane = quad.anchor[(quad.axis_sign & 3) as usize];
        if let Some(bin) = tile
            .native_bins
            .last_mut()
            .filter(|bin| bin.axis_sign == quad.axis_sign)
        {
            bin.quad_count += 1;
            bin.min_plane = bin.min_plane.min(plane);
            bin.max_plane = bin.max_plane.max(plane);
        } else {
            tile.native_bins.push(NativeBin {
                axis_sign: quad.axis_sign,
                first_quad: index as u32,
                quad_count: 1,
                min_plane: plane,
                max_plane: plane,
            });
        }
    }
    tile.bytes = tile.native_quads.capacity() * std::mem::size_of::<NativeQuad>()
        + tile.native_bins.capacity() * std::mem::size_of::<NativeBin>()
        + tile.face_profiles.capacity() * std::mem::size_of::<[[f32; 4]; 5]>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_geometry::{build_surface_with_geometry, SourceSamples, TileKey};
    use frontier_world::{FbmParams, World};

    fn mixed_tile(
        world: &World,
        min: [i64; 2],
        width: u32,
        step: u32,
        edges: [u32; 4],
    ) -> GeometryTile {
        let key = TileKey {
            min,
            width_cells: width,
            step_cells: step,
            edge_steps: edges,
            source_revision: 0,
        };
        let mut tile =
            build_surface_with_geometry(world, key, || false, &mut SourceSamples::new(0), true)
                .unwrap();
        merge_quads(&mut tile);
        tile
    }

    fn edge_top(tile: &GeometryTile, boundary_x: f32, positive: bool, z: f32) -> f32 {
        let tops: Vec<_> = tile
            .native_quads
            .iter()
            .filter(|q| {
                q.axis_sign == 1
                    && q.anchor[2] <= z
                    && q.anchor[2] + q.extent[0] > z
                    && if positive {
                        q.anchor[0] + q.extent[1] == boundary_x
                    } else {
                        q.anchor[0] == boundary_x
                    }
            })
            .collect();
        assert_eq!(tops.len(), 1);
        tops[0].anchor[1]
    }

    fn side_span(tile: &GeometryTile, face: u32, x: f32, z: f32) -> Vec<[f32; 2]> {
        tile.native_quads
            .iter()
            .filter(|q| {
                q.axis_sign == face
                    && q.anchor[0] == x
                    && q.anchor[2] <= z
                    && q.anchor[2] + q.extent[1] > z
            })
            .map(|q| [q.anchor[1], q.anchor[1] + q.extent[0]])
            .collect()
    }

    #[test]
    fn coarse_and_fine_tiles_share_a_closed_boundary_with_unique_side_ownership() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let coarse = mixed_tile(&world, [-64, -64], 64, 8, [8, 8, 4, 8]);
        for offset in [0, 32] {
            let fine = mixed_tile(&world, [0, -64 + offset], 32, 4, [4; 4]);
            for t in 0..32 {
                let fine_z = ((t as f64 + 0.5) * voxel_size()) as f32;
                let coarse_z = fine_z + (offset as f64 * voxel_size()) as f32;
                let a = edge_top(&coarse, 4., true, coarse_z);
                let b = edge_top(&fine, 0., false, fine_z);
                let a_faces = side_span(&coarse, 0, 4., coarse_z);
                let b_faces = side_span(&fine, 4, 0., fine_z);
                if a > b {
                    assert_eq!(a_faces, vec![[b, a]]);
                    assert!(b_faces.is_empty());
                } else if b > a {
                    assert_eq!(b_faces, vec![[a, b]]);
                    assert!(a_faces.is_empty());
                } else {
                    assert!(a_faces.is_empty() && b_faces.is_empty());
                }
            }
        }
    }

    #[test]
    fn exact_neighbor_receives_native_height_and_face_ownership() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let coarse = mixed_tile(&world, [-32, 0], 32, 4, [4, 4, 1, 4]);
        for t in 0..32 {
            let z = ((t as f64 + 0.5) * voxel_size()) as f32;
            let a = world.column(-1, t).height_m() as f32;
            let b = world.column(0, t).height_m() as f32;
            assert_eq!(edge_top(&coarse, 2., true, z), a);
            assert_eq!(
                side_span(&coarse, 0, 2., z),
                if a > b { vec![[b, a]] } else { vec![] }
            );
        }
    }

    #[test]
    fn neighboring_pitch_change_reuses_interior_with_fresh_seams_and_profiles() {
        let world = World::fbm(
            7,
            FbmParams {
                recipe: frontier_world::FbmRecipe::Alpine,
                ..Default::default()
            },
        )
        .unwrap();
        let key = TileKey {
            min: [-768, -4608],
            width_cells: 512,
            step_cells: 64,
            edge_steps: [64; 4],
            source_revision: world.revision(),
        };
        let mut samples = SourceSamples::new(0);
        let cache = Arc::new(InteriorCache::new(1 << 20).unwrap());
        samples.pages.coarse_interiors = Some(cache.clone());
        let build = |key, samples: &mut SourceSamples| {
            let mut tile =
                build_surface_with_geometry(&world, key, || false, samples, true).unwrap();
            merge_quads(&mut tile);
            tile
        };
        let initial = build(key, &mut samples);
        let refined_key = TileKey {
            edge_steps: [64, 16, 32, 64],
            ..key
        };
        let reused = build(refined_key, &mut samples);
        let mut reference = SourceSamples::new(0);
        reference.coarse_reference = true;
        let cold = build(refined_key, &mut reference);
        assert_eq!(cache.hits.load(Relaxed), 1);
        assert_ne!(initial.triangle_count(), reused.triangle_count());
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&reused.native_quads),
            bytemuck::cast_slice::<_, u8>(&cold.native_quads)
        );
        assert_eq!(reused.face_profiles, cold.face_profiles);
        if let (Some(reused_error), Some(cold_error)) = (reused.affine_error_m, cold.affine_error_m)
        {
            assert!((reused_error - cold_error).abs() < 1e-10);
        } else {
            panic!("both builds must certify this tile");
        }
        assert_eq!(reused.bounds, cold.bounds);
        assert_eq!(reused.empirical_error_m, cold.empirical_error_m);
        cache.clear();
        build(refined_key, &mut samples);
        assert_eq!(cache.hits.load(Relaxed), 1);
    }

    #[test]
    fn seed_appearance_tracks_visible_water_and_directional_ground() {
        let center = NativeColumn {
            height: 12,
            ground_height: 8,
            bands: [Material::Grass, Material::Soil, Material::Stone],
        };
        let lower = NativeColumn {
            height: 4,
            ground_height: 4,
            ..center
        };
        let profile = seed_node(center, [lower, center, center, center], 4);
        let water = Material::Water.color();
        let grass = Material::Grass.color();
        assert_eq!(profile[0], [water[0], water[1], water[2], 1.]);
        for i in 0..3 {
            assert_eq!(profile[1][i], water[i] + grass[i]);
        }
        assert_eq!(profile[1][3], 2.);
        assert_eq!(profile[2..], [[0.; 4]; 3]);
    }

    #[test]
    fn application_default_and_comparison_overrides_are_distinct() {
        assert!(Mode::resolve(None, Mode::Seed) == Mode::Seed);
        for (value, mode) in [
            ("surface", Mode::Surface),
            ("voxy", Mode::Voxy),
            ("voxy-seed", Mode::Seed),
        ] {
            assert!(Mode::resolve(Some(value), Mode::Seed) == mode);
        }
    }

    #[test]
    fn quantization_is_world_aligned_through_zero() {
        assert_eq!(round_to_pitch(-7, 8), -8);
        assert_eq!(round_to_pitch(-3, 8), 0);
        assert_eq!(round_to_pitch(3, 8), 0);
        assert_eq!(round_to_pitch(5, 8), 8);
    }
}
