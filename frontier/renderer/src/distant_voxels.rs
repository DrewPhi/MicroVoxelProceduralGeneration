//! Coarse voxel LOD surface for the canonical heightfield. Each rectangle is
//! a run of occupied columns with one common top. Side faces are emitted once,
//! by the higher run, and no interior volume is allocated.

use super::{
    profile_probe_bands, Generator, GeometryTile, NativeBin, NativeColumn, NativeQuad, SourceStage,
    MAX_AFFINE_QUADS, MAX_CERTIFICATE_WIDTH, MAX_VERTICES,
};
use crate::{mesh::Vertex, spatial_appearance::Profile};
use frontier_world::{
    affine_bounds::{BoundBudget, ColumnBox},
    Material, VOXEL_SIZE,
};
use std::{collections::BTreeMap, ops::Range, sync::OnceLock};

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
                origin.map(|v| (v as f64 * VOXEL_SIZE) as f32),
                (stride as f64 * VOXEL_SIZE) as f32,
                dimension,
                &refs,
                at,
            ));
        self.samples.stages.appearance_ms += started.elapsed().as_secs_f64() * 1000.;
        self.samples.source_stage = SourceStage::Proxy;
        Ok(at)
    }

    fn coarse_run(&mut self, sample: [i64; 2], pitch: i64, profile: u32) -> Result<Run, String> {
        let column = self.column(sample)?;
        let wet = column.height > column.ground_height;
        let ground = if wet {
            column.ground_height
        } else {
            round_to_pitch(column.ground_height, pitch)
        };
        Ok(Run {
            top: if wet { column.height } else { ground },
            ground,
            top_material: column.material(column.height - 1),
            deep_material: column.bands[2],
            profile,
        })
    }

    fn coarse_quad(
        &mut self,
        anchor: [i64; 3],
        axis: usize,
        positive: bool,
        extent: [i64; 2],
        material: Material,
        profile: u32,
    ) -> Result<(), String> {
        if self.mesh.vertices.len() + 4 > MAX_VERTICES {
            return Err("adaptive voxel mesh byte budget exceeded".into());
        }
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let mut points = [anchor; 4];
        points[1][u] += extent[0];
        points[2][u] += extent[0];
        points[2][v] += extent[1];
        points[3][v] += extent[1];
        let base = self.mesh.vertices.len() as u32;
        let mut normal = [0.; 3];
        normal[axis] = if positive { 1. } else { -1. };
        self.mesh.vertices.extend(points.map(|p| Vertex {
            position: p.map(|c| (c as f64 * VOXEL_SIZE) as f32),
            normal,
            color: material.color(),
        }));
        self.mesh.indices.extend(if positive {
            [base, base + 1, base + 2, base, base + 2, base + 3]
        } else {
            [base, base + 3, base + 2, base, base + 2, base + 1]
        });
        self.face_profile_indices.extend([profile; 4]);
        Ok(())
    }

    fn coarse_rect(
        &mut self,
        edges: &mut Edges,
        rect: [Range<i64>; 2],
        run: Run,
    ) -> Result<(), String> {
        let [x, z] = rect;
        if x.is_empty() || z.is_empty() {
            return Ok(());
        }
        self.coarse_quad(
            [x.start, run.top, z.start],
            1,
            true,
            [z.end - z.start, x.end - x.start],
            run.top_material,
            run.profile,
        )?;
        for (axis, line, span) in [(0, x.clone(), z.clone()), (2, z, x)] {
            for (side, plane) in [line.start, line.end].into_iter().enumerate() {
                edges.entry((axis, plane)).or_default()[side].push(Edge {
                    span: span.clone(),
                    run,
                });
            }
        }
        Ok(())
    }

    fn coarse_wall(
        &mut self,
        axis: usize,
        plane: i64,
        positive: bool,
        span: Range<i64>,
        low: i64,
        high: Run,
    ) -> Result<(), String> {
        let ground = high.ground.clamp(low, high.top);
        for (lo, hi, material) in [
            (low, ground, high.deep_material),
            (ground, high.top, high.top_material),
        ] {
            if lo == hi {
                continue;
            }
            let (anchor, extent) = if axis == 0 {
                ([plane, lo, span.start], [hi - lo, span.end - span.start])
            } else {
                ([span.start, lo, plane], [span.end - span.start, hi - lo])
            };
            self.coarse_quad(anchor, axis, positive, extent, material, high.profile)?;
        }
        Ok(())
    }

    fn record_coarse_error(
        &mut self,
        min: [i64; 2],
        pitch: i64,
        first_vertex: usize,
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
            for vertex in (first_vertex..self.mesh.vertices.len()).step_by(4) {
                let cell = |v: Vertex| v.position.map(|p| (f64::from(p) / VOXEL_SIZE) as i64);
                let [x0, top, z0] = cell(self.mesh.vertices[vertex]);
                let [x1, _, z1] = cell(self.mesh.vertices[vertex + 2]);
                for z in z0..z1 {
                    if z % 32 == 0 && cancelled() {
                        return Err("adaptive geometry cancelled".into());
                    }
                    for x in x0..x1 {
                        let h = self.height(x, z)? as f64 * VOXEL_SIZE;
                        range[0] = range[0].min(h);
                        range[1] = range[1].max(h);
                        error = error.max((h - top as f64 * VOXEL_SIZE).abs());
                    }
                }
            }
            true
        } else if self.affine_stats.quad_bounds < MAX_AFFINE_QUADS {
            self.affine_stats.quad_bounds += 1;
            let bound = cached_bound.unwrap_or_else(|| {
                self.samples.pages.bound(
                    self.world,
                    ColumnBox {
                        min: [key.min[0] + min[0], key.min[1] + min[1]],
                        size_cells: [pitch as u32; 2],
                    },
                    BoundBudget::default(),
                )
            });
            if let Some(bound) = bound {
                range = bound.height_range;
                self.affine_stats.noise_cells += bound.noise_cells as u64;
                self.affine_stats.full_range_fallbacks += bound.full_range_fallbacks as u64;
                let mut complete = true;
                for quad in self.mesh.vertices[first_vertex..].chunks_exact(4) {
                    let position = |i: usize| {
                        let [x, y, z] = quad[i].position.map(f64::from);
                        [
                            x + key.min[0] as f64 * VOXEL_SIZE,
                            y,
                            z + key.min[1] as f64 * VOXEL_SIZE,
                        ]
                    };
                    if let Some(value) = bound.quad_error(std::array::from_fn(position)) {
                        error = error.max(value);
                    } else {
                        complete = false;
                    }
                }
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
                self.affine_stats.cpu_ms += started.elapsed().as_secs_f64() * 1000.;
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
        let mut edges = Edges::new();
        for z in (0..width).step_by(pitch as usize) {
            for x in (0..width).step_by(pitch as usize) {
                if cancelled() {
                    return Err("adaptive geometry cancelled".into());
                }
                let first_vertex = self.mesh.vertices.len();
                let profile = match seed_profile {
                    Some(profile) => profile,
                    None => self.patch_appearance(x, z, pitch)?,
                };
                let on_border = [x == 0, z + pitch == width, x + pitch == width, z == 0];
                let mut top_range = [i64::MAX, i64::MIN];
                let mut rect = |g: &mut Self, footprint, run: Run| {
                    top_range[0] = top_range[0].min(run.top);
                    top_range[1] = top_range[1].max(run.top);
                    g.coarse_rect(&mut edges, footprint, run)
                };
                let interior = self.coarse_run([x + pitch / 2, z + pitch / 2], pitch, profile)?;
                let [a, b, c, d] = on_border.map(i64::from);
                rect(self, [x + a..x + pitch - c, z + d..z + pitch - b], interior)?;

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
                            let run = self.coarse_run(sample_xz, sample_pitch, profile)?;
                            let footprint = if axis == 0 {
                                [inside..inside + 1, clipped]
                            } else {
                                [clipped, inside..inside + 1]
                            };
                            rect(self, footprint, run)?;
                        }
                    }
                }
                let center = self.height(x + pitch / 2, z + pitch / 2)?;
                self.empirical_error_m = self.empirical_error_m.max(
                    (center - top_range[0])
                        .abs()
                        .max((center - top_range[1]).abs()) as f64
                        * VOXEL_SIZE,
                );
                if can_certify {
                    let bound = bounds
                        .as_ref()
                        .map(|bounds| bounds[((z / pitch) * i64::from(grid) + x / pitch) as usize]);
                    self.record_coarse_error([x, z], pitch, first_vertex, bound, cancelled)?;
                }
            }
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
                        self.coarse_wall(
                            axis,
                            plane,
                            positive,
                            edge.span.clone(),
                            neighbor,
                            edge.run,
                        )?;
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
                    self.coarse_wall(axis, plane, outward, span, low.run.top, high.run)?;
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
pub(super) fn pack(tile: &mut GeometryTile) -> Result<(), String> {
    let mut quads = Vec::with_capacity(tile.mesh.vertices.len() / 4);
    for (index, face) in tile.mesh.vertices.chunks_exact(4).enumerate() {
        let axis = face[0]
            .normal
            .iter()
            .position(|&n| n != 0.)
            .ok_or("coarse face has no normal")?;
        let material = TERRAIN_PALETTE
            .iter()
            .position(|m| m.color() == face[0].color)
            .ok_or("coarse face color is outside the material palette")?
            as u32;
        let extent =
            [(axis + 1) % 3, (axis + 2) % 3].map(|a| face[2].position[a] - face[0].position[a]);
        quads.push(NativeQuad {
            anchor: face[0].position,
            extent,
            axis_sign: axis as u32 | if face[0].normal[axis] < 0. { 4 } else { 0 },
            material,
            profile_index: tile.face_profile_indices[index * 4],
        });
    }
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
                (q.anchor[u] as f64 / VOXEL_SIZE) as i64,
            )
        };
        quads.sort_unstable_by_key(order);
        let mut merged: Vec<NativeQuad> = Vec::with_capacity(quads.len());
        for quad in quads {
            if let Some(prev) = merged.last_mut() {
                let axis = (quad.axis_sign & 3) as usize;
                let u = (axis + 1 + merge) % 3;
                let (a, b, c, d, e, f, _) = order(prev);
                let (g, h, i, j, k, l, _) = order(&quad);
                if (a, b, c, d, e, f) == (g, h, i, j, k, l)
                    && prev.anchor[u] + prev.extent[merge] == quad.anchor[u]
                {
                    prev.extent[merge] += quad.extent[merge];
                    continue;
                }
            }
            merged.push(quad);
        }
        quads = merged;
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
    tile.native_quads = quads;
    tile.mesh.vertices = vec![];
    tile.mesh.indices = vec![];
    tile.face_profile_indices = vec![];
    tile.bytes = tile.native_quads.capacity() * std::mem::size_of::<NativeQuad>()
        + tile.native_bins.capacity() * std::mem::size_of::<NativeBin>()
        + tile.face_profiles.capacity() * std::mem::size_of::<[[f32; 4]; 5]>();
    Ok(())
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
        pack(&mut tile).unwrap();
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
                let fine_z = ((t as f64 + 0.5) * VOXEL_SIZE) as f32;
                let coarse_z = fine_z + (offset as f64 * VOXEL_SIZE) as f32;
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
            let z = ((t as f64 + 0.5) * VOXEL_SIZE) as f32;
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
