//! Playable adapter for the immutable geographic atlas. Physical coordinates
//! precede voxelization and world-scale compression. Edits remain world overlays.
mod data;
mod paint;
mod refine;
mod valley;
use super::{landscape_paint, summary, Column, Generator, GeneratorIdentity, Run};
use crate::{materials::Appearance, CellPos, GridScale, MaterialId, Voxel, WorldSpec};
use data::Data;
use refine::Patch;
use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Cache {
    patches: HashMap<u32, Arc<Patch>>,
    fifo: VecDeque<u32>,
}
#[derive(Clone, Copy)]
struct PaintedColumn {
    column: Column,
    paint: landscape_paint::Paint,
}
#[derive(Default)]
struct ColumnCache {
    values: HashMap<(i64, i64), PaintedColumn>,
    fifo: VecDeque<(i64, i64)>,
}
pub struct AtlasTerrain {
    data: Arc<Data>,
    field: Arc<valley::Field>,
    spec: WorldSpec,
    scale: u32,
    datum: f64,
    summaries: Arc<SummaryIndex>,
    cache: Arc<[Mutex<Cache>; 16]>,
    columns: [Mutex<ColumnCache>; 16],
}
impl AtlasTerrain {
    pub fn load(path: &Path, grid: GridScale, scale: u32) -> Result<Self, String> {
        super::ProceduralTerrain::validate_world_scale(scale)?;
        let data = Arc::new(Data::load(path)?);
        let datum = (-data.low).max(0.).ceil() + 32.;
        let field = Arc::new(valley::Field::build(&data));
        let summaries = Arc::new(SummaryIndex::build(&data));
        let ceiling = shifted_ceiling(
            summaries.levels.last().unwrap()[0].high + field.max_delta(),
            datum,
            scale,
        )?;
        let spec = WorldSpec {
            grid,
            extent_m: (data.extent / scale as f64).ceil() as u32,
            ceiling_m: ceiling,
        }
        .validate()?;
        Ok(Self {
            data,
            field,
            spec,
            scale,
            datum,
            summaries,
            cache: Arc::new(std::array::from_fn(|_| Mutex::new(Cache::default()))),
            columns: std::array::from_fn(|_| Mutex::new(ColumnCache::default())),
        })
    }
    pub fn at_grid(&self, grid: GridScale) -> Self {
        Self {
            data: self.data.clone(),
            field: self.field.clone(),
            spec: WorldSpec { grid, ..self.spec },
            scale: self.scale,
            datum: self.datum,
            summaries: self.summaries.clone(),
            cache: self.cache.clone(),
            columns: std::array::from_fn(|_| Mutex::new(ColumnCache::default())),
        }
    }
    pub fn world_scale(&self) -> u32 {
        self.scale
    }
    pub fn memory_bytes(&self) -> usize {
        self.data.memory_bytes()
            + self.field.memory_bytes()
            + self.summaries.memory_bytes()
            + self
                .columns
                .iter()
                .map(|c| {
                    let c = c.lock().unwrap();
                    c.values.capacity() * (std::mem::size_of::<((i64, i64), PaintedColumn)>() + 1)
                        + c.fifo.capacity() * std::mem::size_of::<(i64, i64)>()
                })
                .sum::<usize>()
            + self
                .cache
                .iter()
                .map(|c| {
                    c.lock()
                        .unwrap()
                        .patches
                        .values()
                        .map(|p| {
                            std::mem::size_of::<Patch>()
                                + p.channels.capacity() * std::mem::size_of::<refine::Channel>()
                        })
                        .sum::<usize>()
                })
                .sum::<usize>()
    }
    fn patch(&self, id: u32) -> Arc<Patch> {
        let shard = &self.cache[id as usize % 16];
        if let Some(p) = shard.lock().unwrap().patches.get(&id).cloned() {
            return p;
        }
        let p = Arc::new(Patch::build(&self.data, id));
        let mut c = shard.lock().unwrap();
        if let Some(old) = c.patches.get(&id) {
            return old.clone();
        }
        if c.patches.len() >= 128 {
            if let Some(old) = c.fifo.pop_front() {
                c.patches.remove(&old);
            }
        }
        c.fifo.push_back(id);
        c.patches.insert(id, p.clone());
        p
    }
    fn sample(
        &self,
        x: f64,
        z: f64,
        spacing: f64,
    ) -> Option<(refine::Surface, landscape_paint::Context)> {
        let s = self.scale as f64;
        if [x, z]
            .iter()
            .any(|v| !v.is_finite() || *v < 0. || *v >= self.spec.extent_m as f64)
        {
            return None;
        }
        let p = source_point([x, z], s, self.data.extent);
        let (surface, _) = self.source_height(p, spacing * s)?;
        let (id, w) = self.data.locate(p)?;
        let mut context = self.data.context(id, w);
        context.water_depth_m = (surface.water - surface.height).max(0.);
        Some((
            refine::Surface {
                height: (surface.height + self.datum) / s,
                water: (surface.water + self.datum) / s,
            },
            context,
        ))
    }
    // Geometry-only queries never evaluate surface palettes or local tangents.
    fn source_height(
        &self,
        p: [f64; 2],
        footprint: f64,
    ) -> Option<(refine::Surface, valley::Detail)> {
        let (id, w) = self.data.locate(p)?;
        let patch = self.patch(id);
        let mut surface = patch.sample(p, w, self.data.seed, footprint);
        let mut detail = self.field.sample(p);
        let was_wet = surface.water > surface.height;
        let mut protection = if was_wet { 0. } else { 1. };
        // Raster masks alone cannot protect a narrow channel between field
        // nodes. Enforce the same canonical constraints at the exact query.
        if protection > 0. && (detail.delta != 0. || detail.deposit != 0.) {
            for channel in &patch.channels {
                let [lo, hi] = channel.bounds();
                if (0..2).any(|axis| p[axis] < lo[axis] - 160. || p[axis] > hi[axis] + 160.) {
                    continue;
                }
                let (_, distance) = channel.nearest(p);
                protection *= super::noise::smoothstep(
                    channel.width * 0.65,
                    channel.influence() + 160.,
                    distance,
                );
            }
        }
        detail.delta *= protection;
        detail.deposit *= protection;
        surface.height += detail.delta;
        if !was_wet {
            surface.water = surface.height;
        }
        Some((surface, detail))
    }
    fn source_sample(
        &self,
        p: [f64; 2],
        footprint: f64,
    ) -> Option<(refine::Surface, landscape_paint::Context, paint::Local)> {
        let (id, w) = self.data.locate(p)?;
        let patch = self.patch(id);
        let (surface, detail) = self.source_height(p, footprint)?;
        let mut context = self.data.context(id, w);
        // Derive the local tangent from the final carved base, not the atlas
        // node gradients. Neighbors use their own canonical triangle at seams.
        let epsilon = 8.;
        let mut slope = [0.; 2];
        let mut curvature = 0.;
        for axis in 0..2 {
            let mut lo = p;
            let mut hi = p;
            lo[axis] = (p[axis] - epsilon).max(0.);
            hi[axis] = (p[axis] + epsilon).min(self.data.extent);
            let height = |q| {
                self.source_height(q, footprint)
                    .map(|(s, _)| s.height)
                    .unwrap_or(surface.height)
            };
            let (a, b) = (height(lo), height(hi));
            slope[axis] = (b - a) / (hi[axis] - lo[axis]).max(1e-6);
            curvature += (a + b - 2. * surface.height) / (epsilon * epsilon);
        }
        context.gradient = slope;
        context.water_depth_m = (surface.water - surface.height).max(0.);
        let mut local = paint::Local {
            slope,
            curvature,
            deposit: detail.deposit,
            river_distance: f64::INFINITY,
            river_width: 0.,
        };
        for channel in &patch.channels {
            let (_, distance) = channel.nearest(p);
            if distance < local.river_distance {
                local.river_distance = distance;
                local.river_width = channel.width;
            }
        }
        Some((surface, context, local))
    }
    fn local_bounds(&self, lo: [f64; 2], hi: [f64; 2]) -> Option<(Bound, f64)> {
        let mut bounds = Bound::default();
        let mut error = 0f64;
        let mut planes = Vec::new();
        for id in self.data.triangle_ids_in_rect(lo, hi) {
            let patch = self.patch(id);
            let polygon = clip_triangle(patch.points, lo, hi);
            if polygon.is_empty() {
                continue;
            }
            let point = std::array::from_fn(|axis| {
                polygon.iter().map(|p| p[axis]).sum::<f64>() / polygon.len() as f64
            });
            let w = barycentric(patch.points, point);
            let gradients = barycentric_gradients(patch.points);
            let h = patch.height(w);
            let (gradient, curvature) = cubic_derivatives(&patch.controls, gradients, w);
            let radius = std::array::from_fn::<_, 2, _>(|axis| {
                polygon
                    .iter()
                    .map(|p| (p[axis] - point[axis]).abs())
                    .fold(0., f64::max)
            });
            let second = 0.5
                * (curvature[0] * radius[0] * radius[0]
                    + 2. * curvature[1] * radius[0] * radius[1]
                    + curvature[2] * radius[1] * radius[1]);
            let variation = gradient[0].abs() * radius[0] + gradient[1].abs() * radius[1] + second;
            let mut bound = Bound {
                low: h - variation - 0.52,
                high: h + variation + 0.52,
                wet: false,
                width: f64::INFINITY,
                point: None,
            };
            let mut local_error = 2. * second + 1.04;
            // Each compiled wet corner contributes a fixed-level candidate.
            // Tapered barycentric support can only lower it toward the bed.
            for i in 0..3 {
                if patch.wet_depth[i] <= 0. {
                    continue;
                }
                let supported = polygon
                    .iter()
                    .any(|p| barycentric(patch.points, *p)[i] > 0.);
                if supported {
                    let level = patch.water[i];
                    local_error = local_error.max((level - bound.low).max(0.));
                    bound.high = bound.high.max(level);
                    bound.wet = true;
                    bound.width = bound.width.min((hi[0] - lo[0]).max(hi[1] - lo[1]));
                }
            }
            for channel in &patch.channels {
                let influence = channel.influence() + channel.bend.abs();
                let expanded = refine::Channel {
                    width: influence * 2.,
                    ..*channel
                };
                if let Some((start, end)) = channel_interval(&expanded, lo, hi) {
                    let level = |t: f64| channel.position(t)[1];
                    let profile_low = level(start).min(level(end));
                    let bed = if channel.compiled {
                        profile_low.min(bound.low)
                    } else {
                        profile_low
                    } - channel.depth();
                    local_error = local_error.max((bound.high - bed).max(0.));
                    bound.low = bound.low.min(bed);
                    if channel_footprint(channel, lo, hi).is_some() {
                        bound.wet = true;
                        bound.width = bound.width.min(channel.width);
                        bound.high = bound.high.max(level(start).max(level(end)));
                    }
                }
            }
            // Different parent triangles have continuous heights, but different
            // tangent planes; include their affine disagreement explicitly.
            planes.push((point, h, gradient));
            bounds.include(bound);
            error = error.max(local_error);
        }
        if !bounds.low.is_finite() {
            return None;
        }
        if planes.len() > 1 {
            let mut disagreement = 0f64;
            for p in [
                [lo[0], lo[1]],
                [hi[0], lo[1]],
                [lo[0], hi[1]],
                [hi[0], hi[1]],
            ] {
                let mut low = f64::INFINITY;
                let mut high = f64::NEG_INFINITY;
                for (origin, h, g) in &planes {
                    let v = h + g[0] * (p[0] - origin[0]) + g[1] * (p[1] - origin[1]);
                    low = low.min(v);
                    high = high.max(v);
                }
                disagreement = disagreement.max(high - low);
            }
            error += disagreement;
        }
        Some((bounds, error.min(bounds.high - bounds.low)))
    }
    fn source_rect(&self, origin: [f64; 2], edge: f64) -> Option<([f64; 2], [f64; 2])> {
        if !edge.is_finite()
            || edge <= 0.
            || origin
                .iter()
                .any(|v| !v.is_finite() || *v < 0. || *v >= self.spec.extent_m as f64)
        {
            return None;
        }
        let s = self.scale as f64;
        Some((
            origin.map(|v| (v * s).min(self.data.extent)),
            origin.map(|v| ((v + edge) * s).min(self.data.extent)),
        ))
    }
    fn column(&self, x: i64, z: i64, spacing: i64) -> Option<Column> {
        self.painted_column(x, z, spacing).map(|p| p.column)
    }
    fn painted_column(&self, x: i64, z: i64, spacing: i64) -> Option<PaintedColumn> {
        let hash = crate::painting::hash(x as u64 ^ crate::painting::hash(z as u64));
        let shard = &self.columns[hash as usize % self.columns.len()];
        if spacing == 1 {
            if let Some(c) = shard.lock().unwrap().values.get(&(x, z)).copied() {
                return Some(c);
            }
        }
        if !(0..self.spec.extent_cells()).contains(&x)
            || !(0..self.spec.extent_cells()).contains(&z)
        {
            return None;
        }
        let m = self.spec.grid.meters();
        let scale = self.scale as f64;
        let source = source_point([x as f64 * m, z as f64 * m], scale, self.data.extent);
        let footprint = if spacing == 1 {
            0.001
        } else {
            spacing as f64 * m * scale
        };
        let (s, c, local) = self.source_sample(source, footprint)?;
        let ground = ((s.height + self.datum) / (scale * m)).floor() as i64;
        let water = ((s.water + self.datum) / (scale * m)).floor() as i64;
        let paint = paint::sample(
            source,
            if spacing == 1 { m * scale } else { footprint },
            self.data.seed,
            c,
            local,
        );
        let surface = surface_material(paint, c);
        let result = PaintedColumn {
            column: Column {
                ground,
                water: water.max(ground),
                bedrock: ground - (2. / m).ceil() as i64,
                surface,
                subsoil: MaterialId::SOIL,
                rock: MaterialId::ROCK,
            },
            paint,
        };
        if spacing == 1 {
            let mut cache = shard.lock().unwrap();
            if !cache.values.contains_key(&(x, z)) {
                if cache.values.len() >= 1024 {
                    if let Some(old) = cache.fifo.pop_front() {
                        cache.values.remove(&old);
                    }
                }
                cache.fifo.push_back((x, z));
                cache.values.insert((x, z), result);
            }
        }
        Some(result)
    }
    fn color(&self, p: CellPos, material: MaterialId, step: i64) -> Appearance {
        if material == MaterialId::WATER {
            return crate::painting::encode_rgb565([44, 96, 108]);
        }
        let Some(painted) = self.painted_column(p.x, p.z, step.max(1)) else {
            return Appearance(0);
        };
        // Test occupied depth, not filtered material identity: different display
        // footprints must not replace coherent surface paint with a flat palette.
        let color = if p.y >= painted.column.ground - step.max(1) {
            painted.paint.linear_rgb
        } else {
            let rgb = match material {
                MaterialId::ROCK => [143, 139, 125],
                MaterialId::SOIL => [130, 115, 86],
                MaterialId::GRAVEL => [171, 158, 130],
                MaterialId::MUD => [90, 83, 65],
                _ => [130, 115, 86],
            };
            rgb.map(|v| {
                let s = v as f64 / 255.;
                if s <= 0.04045 {
                    (s / 12.92) as f32
                } else {
                    ((s + 0.055) / 1.055).powf(2.4) as f32
                }
            })
        };
        crate::painting::encode_rgb565(color.map(|v| {
            let v = v as f64;
            let s = if v <= 0.0031308 {
                12.92 * v
            } else {
                1.055 * v.powf(1. / 2.4) - 0.055
            };
            (s.clamp(0., 1.) * 255.).round() as u8
        }))
    }
}
impl Generator for AtlasTerrain {
    fn spec(&self) -> WorldSpec {
        self.spec
    }
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: self.data.seed,
            version: 4,
            definition: self.data.fingerprint
                ^ crate::painting::hash(self.scale as u64)
                ^ 0x41544c4153504e32
                ^ crate::painting::hash(0x56414c4c45590008),
        }
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        let material = self.column(p.x, p.z, 1)?.material_at(p.y);
        Some(if material == MaterialId::AIR {
            Voxel::AIR
        } else {
            Voxel::new(material, self.color(p, material, 1))
        })
    }
    fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
        self.column(x, z, 1)
    }
    fn display_column(&self, x: i64, z: i64, step: i64) -> Option<Column> {
        self.column(x, z, step)
    }
    fn appearance_at(&self, p: CellPos, m: MaterialId) -> Appearance {
        self.color(p, m, 1)
    }
    fn display_appearance_at(&self, p: CellPos, m: MaterialId, s: i64) -> Appearance {
        self.color(p, m, s)
    }
    fn runs(&self, x: i64, z: i64, r: Range<i64>) -> Option<Vec<Run>> {
        Some(self.column(x, z, 1)?.runs(r))
    }
    fn water_footprint(&self, o: [f64; 2], edge: f64) -> Option<super::river::WaterFootprint> {
        let (lo, hi) = self.source_rect(o, edge)?;
        let b = self.summaries.query(lo, hi, self.data.extent);
        if !b.wet {
            return None;
        }
        let scale = self.scale as f64;
        if let Some(p) = b
            .point
            .filter(|p| (0..2).all(|i| p[i] >= lo[i] && p[i] <= hi[i]))
        {
            let point = p.map(|v| v / scale);
            if self
                .sample(
                    point[0].min(self.spec.extent_m as f64 - 1e-8),
                    point[1].min(self.spec.extent_m as f64 - 1e-8),
                    0.001 / scale,
                )
                .map_or(false, |(s, _)| s.water > s.height)
            {
                return Some(super::river::WaterFootprint {
                    id: 0x41544c53,
                    point,
                    width_m: b.width / scale,
                    coverage: (b.width / (edge * scale)).clamp(0.000001, 1.),
                });
            }
        }
        // Fine queries resolve indexed global and local centerlines so a narrow
        // channel's representative point remains on the actual feature.
        if edge * scale <= self.data.extent / (self.data.side - 1) as f64 * 2. {
            let mut best: Option<super::river::WaterFootprint> = None;
            for id in self.data.triangle_ids_in_rect(lo, hi) {
                for channel in &self.patch(id).channels {
                    if let Some(mut footprint) = channel_footprint(channel, lo, hi) {
                        footprint.id = id;
                        footprint.point = footprint.point.map(|v| v / scale);
                        footprint.width_m /= scale;
                        let p = footprint.point;
                        if !self
                            .sample(
                                p[0].min(self.spec.extent_m as f64 - 1e-8),
                                p[1].min(self.spec.extent_m as f64 - 1e-8),
                                0.001 / scale,
                            )
                            .map_or(false, |(s, _)| s.water > s.height)
                        {
                            continue;
                        }
                        if best
                            .as_ref()
                            .map_or(true, |b| footprint.coverage > b.coverage)
                        {
                            best = Some(footprint);
                        }
                    }
                }
            }
            if best.is_some() {
                return best;
            }
        }
        // Lake metadata is returned only at a confirmed wet sample. Bounds and
        // feature flags may over-cover; a render footprint must not invent water.
        for f in [[0.5, 0.5], [0., 0.], [1., 0.], [0., 1.], [1., 1.]] {
            let p = std::array::from_fn(|i| (lo[i] + (hi[i] - lo[i]) * f[i]) / scale);
            if let Some((surface, _)) = self.sample(
                p[0].min(self.spec.extent_m as f64 - 1e-8),
                p[1].min(self.spec.extent_m as f64 - 1e-8),
                0.001 / scale,
            ) {
                if surface.water > surface.height {
                    return Some(super::river::WaterFootprint {
                        id: 0x41544c53,
                        point: p,
                        width_m: edge,
                        coverage: 1.,
                    });
                }
            }
        }
        None
    }
    fn terrain_summary(&self, o: [f64; 2], edge: f64) -> Option<summary::TerrainSummary> {
        let (lo, hi) = self.source_rect(o, edge)?;
        let (mut b, mut residual) =
            if edge * self.scale as f64 <= self.data.extent / (self.data.side - 1) as f64 * 2. {
                self.local_bounds(lo, hi)?
            } else {
                let b = self.summaries.query(lo, hi, self.data.extent);
                (b, b.high - b.low)
            };
        let (delta_min, delta_max, delta_error) = self.field.summary(lo, hi);
        // Exact-query channel protection multiplies the bilinear displacement
        // by a smooth mask in [0,1]. Include zero in its extrema and bound mask
        // variation locally, preserving a residual that vanishes with spacing.
        let amplitude = delta_min.abs().max(delta_max.abs());
        let mut mask_lipschitz = 0.;
        if amplitude > 0. {
            for channel in self.data.channels_in_rect(lo, hi) {
                let transition = channel.influence() + 160. - channel.width * 0.65;
                mask_lipschitz += 1.5 / transition.max(1.);
            }
        }
        let diameter = (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
        b.low += delta_min.min(0.);
        b.high += delta_max.max(0.);
        residual += delta_error + 2. * amplitude * (mask_lipschitz * diameter).min(1.);
        let scale = self.scale as f64;
        let min = (b.low + self.datum) / scale;
        let max = (b.high + self.datum) / scale;
        Some(summary::TerrainSummary {
            min_m: min,
            max_m: max,
            error_m: residual / scale,
            normal_variation: (max - min) / edge,
            appearance_variation: 1.,
            water_coverage: if b.wet {
                (b.width / (edge * scale)).clamp(0.000001, 1.)
            } else {
                0.
            },
            features: summary::GEOLOGY
                | if b.wet { summary::WATER } else { 0 }
                | if max - min > edge * 0.55 {
                    summary::CLIFF
                } else {
                    0
                },
            feature_scale_m: if b.wet { b.width / scale } else { 1. / scale },
            bounds_kind: summary::BoundsKind::SampledWithResidual,
        })
    }
}

fn surface_material(paint: landscape_paint::Paint, c: landscape_paint::Context) -> MaterialId {
    let category = paint
        .coverage
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    match category {
        1 => MaterialId::ROCK,
        2 => {
            if c.moisture < 0.35 {
                MaterialId::GRAVEL
            } else {
                MaterialId::SOIL
            }
        }
        3 => MaterialId::SNOW,
        4 => MaterialId::MUD,
        _ => {
            if c.moisture < 0.2 {
                MaterialId::SAND
            } else {
                MaterialId::GRASS
            }
        }
    }
}

/// Convex-hull surface bounds, including every channel's full influence rectangle.
/// Leaves intentionally over-cover triangles; queries never depend on quadrature.
#[derive(Clone, Copy)]
struct Bound {
    low: f64,
    high: f64,
    wet: bool,
    width: f64,
    point: Option<[f64; 2]>,
}
impl Default for Bound {
    fn default() -> Self {
        Self {
            low: f64::INFINITY,
            high: f64::NEG_INFINITY,
            wet: false,
            width: f64::INFINITY,
            point: None,
        }
    }
}
impl Bound {
    fn include(&mut self, other: Self) {
        self.low = self.low.min(other.low);
        self.high = self.high.max(other.high);
        self.wet |= other.wet;
        self.width = self.width.min(other.width);
        if self.point.is_none() {
            self.point = other.point;
        }
    }
}
struct SummaryIndex {
    // Fine to coarse, ending with one root. Fixed leaf resolution caps both
    // build storage and partial-rectangle query work independent of atlas size.
    levels: Vec<Vec<Bound>>,
    side: usize,
}
impl SummaryIndex {
    fn build(data: &Data) -> Self {
        let side = 128;
        let mut leaves = vec![Bound::default(); side * side];
        let mut insert = |lo: [f64; 2], hi: [f64; 2], bound: Bound| {
            let bin =
                |v: f64| ((v / data.extent * side as f64).floor().max(0.) as usize).min(side - 1);
            for z in bin(lo[1])..=bin(hi[1]) {
                for x in bin(lo[0])..=bin(hi[0]) {
                    let mut local = bound;
                    if local.point.map_or(false, |p| {
                        (0..2).any(|a| {
                            p[a] < [x, z][a] as f64 * data.extent / side as f64
                                || p[a] > ([x, z][a] + 1) as f64 * data.extent / side as f64
                        })
                    }) {
                        local.point = None;
                    }
                    leaves[z * side + x].include(local);
                }
            }
        };
        for triangle in &data.triangles {
            let n = triangle.map(|i| data.nodes[i as usize]);
            let lo =
                std::array::from_fn(|a| n.iter().map(|n| n.p[a]).fold(f64::INFINITY, f64::min));
            let hi =
                std::array::from_fn(|a| n.iter().map(|n| n.p[a]).fold(f64::NEG_INFINITY, f64::max));
            let low = n
                .iter()
                .map(|n| n.height.min(n.water))
                .fold(f64::INFINITY, f64::min);
            let high = n
                .iter()
                .map(|n| n.height.max(n.water))
                .fold(f64::NEG_INFINITY, f64::max);
            // Cubic edge and interior controls remain within corner extrema;
            // local branches only lower the surface by at most channel depth.
            let wet = n.iter().any(|n| n.water > n.height || n.discharge >= 0.02);
            insert(
                lo,
                hi,
                Bound {
                    low: low - 30.52,
                    high: high + 0.52,
                    wet,
                    width: if wet { 0.25 } else { f64::INFINITY },
                    point: n.iter().find(|n| n.water > n.height + 0.52).map(|n| n.p),
                },
            );
        }
        for channel in data.channels_in_rect([0.; 2], [data.extent; 2]) {
            let [lo, hi] = channel.bounds();
            let midpoint = channel.position(0.5);
            insert(
                lo,
                hi,
                Bound {
                    low: channel.a[1].min(channel.b[1]) - channel.depth() - 0.52,
                    high: channel.a[1].max(channel.b[1]) + 0.52,
                    wet: true,
                    width: channel.width,
                    point: Some([midpoint[0], midpoint[2]]),
                },
            );
        }
        let mut levels = vec![leaves];
        let mut width = side;
        while width > 1 {
            let next_width = width / 2;
            let mut next = vec![Bound::default(); next_width * next_width];
            for z in 0..next_width {
                for x in 0..next_width {
                    for dz in 0..2 {
                        for dx in 0..2 {
                            next[z * next_width + x]
                                .include(levels.last().unwrap()[(z * 2 + dz) * width + x * 2 + dx]);
                        }
                    }
                }
            }
            levels.push(next);
            width = next_width;
        }
        Self { levels, side }
    }
    fn memory_bytes(&self) -> usize {
        self.levels
            .iter()
            .map(|l| l.capacity() * std::mem::size_of::<Bound>())
            .sum()
    }
    fn query(&self, lo: [f64; 2], hi: [f64; 2], extent: f64) -> Bound {
        fn visit(
            index: &SummaryIndex,
            level: usize,
            x: usize,
            z: usize,
            lo: [f64; 2],
            hi: [f64; 2],
            extent: f64,
            result: &mut Bound,
        ) {
            let side = index.side >> level;
            let edge = extent / side as f64;
            let origin = [x as f64 * edge, z as f64 * edge];
            if (0..2).any(|a| origin[a] > hi[a] || origin[a] + edge < lo[a]) {
                return;
            }
            if level == 0 || (0..2).all(|a| lo[a] <= origin[a] && hi[a] >= origin[a] + edge) {
                result.include(index.levels[level][z * side + x]);
            } else {
                for dz in 0..2 {
                    for dx in 0..2 {
                        visit(
                            index,
                            level - 1,
                            x * 2 + dx,
                            z * 2 + dz,
                            lo,
                            hi,
                            extent,
                            result,
                        );
                    }
                }
            }
        }
        let mut result = Bound::default();
        visit(
            self,
            self.levels.len() - 1,
            0,
            0,
            lo,
            hi,
            extent,
            &mut result,
        );
        result
    }
}

fn source_point(point: [f64; 2], scale: f64, extent: f64) -> [f64; 2] {
    point.map(|v| (v * scale).min(extent))
}
fn shifted_ceiling(high: f64, datum: f64, scale: u32) -> Result<u32, String> {
    let ceiling = ((high + datum + 128.) / scale as f64).ceil().max(1024.);
    if !ceiling.is_finite() || ceiling > 32768. {
        return Err(format!("shifted atlas ceiling {ceiling} m exceeds the supported 32768 m; choose a larger world scale"));
    }
    Ok(ceiling as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires saved atlas and compiled valley"]
    fn saved_valley_final_surface_is_continuous_and_inside_summaries() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../output/atlas-climate/seed42");
        let world = AtlasTerrain::load(&path, GridScale::default(), 10).unwrap();
        let (lo, hi) = world.field.bounds();
        for z in 0..12 {
            for x in 0..12 {
                let p = [
                    lo[0] + (x as f64 + 0.5) * (hi[0] - lo[0]) / 12.,
                    lo[1] + (z as f64 + 0.5) * (hi[1] - lo[1]) / 12.,
                ];
                let (id, _) = world.data.locate(p).unwrap();
                let ids = world.data.triangles[id as usize];
                let a = world.data.nodes[ids[0] as usize].p;
                let b = world.data.nodes[ids[1] as usize].p;
                let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                let d = [b[0] - a[0], b[1] - a[1]];
                let len = d[0].hypot(d[1]);
                let left = [mid[0] - d[1] / len * 0.0001, mid[1] + d[0] / len * 0.0001];
                let right = [mid[0] + d[1] / len * 0.0001, mid[1] - d[0] / len * 0.0001];
                let l = world.source_height(left, 0.001).unwrap().0;
                let r = world.source_height(right, 0.001).unwrap().0;
                assert!(
                    (l.height - r.height).abs() < 0.02,
                    "height seam at {mid:?}: {}",
                    l.height - r.height
                );
                assert!((l.water - r.water).abs() < 0.02, "water seam at {mid:?}");
            }
        }
        for origin in [[97000., 47500.], [97570., 47550.], [97675., 47425.]] {
            for edge in [2., 16., 128.] {
                let b = world.terrain_summary(origin, edge).unwrap();
                for z in 0..9 {
                    for x in 0..9 {
                        let p = [
                            origin[0] + edge * x as f64 / 8.,
                            origin[1] + edge * z as f64 / 8.,
                        ];
                        let (s, _) = world.sample(p[0], p[1], 0.001).unwrap();
                        assert!(
                            s.height >= b.min_m - 0.001 && s.water <= b.max_m + 0.001,
                            "summary missed {p:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "loads saved million-node atlas; release query timing"]
    fn saved_atlas_column_query_timing() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../output/atlas-climate/seed42");
        let terrain = AtlasTerrain::load(&path, GridScale::default(), 10).unwrap();
        let mut protected = 0;
        for channel in terrain
            .data
            .channels_in_rect([960000., 465000.], [980000., 485000.])
        {
            let q = channel.position(0.5);
            let point = [q[0], q[2]];
            let Some((id, w)) = terrain.data.locate(point) else {
                continue;
            };
            let base = terrain.patch(id).sample(point, w, terrain.data.seed, 0.001);
            if base.water > base.height {
                let final_surface = terrain.source_height(point, 0.001).unwrap().0;
                assert_eq!(
                    base.height, final_surface.height,
                    "query protection must preserve actual wet beds"
                );
                assert_eq!(base.water, final_surface.water);
                protected += 1;
            }
        }
        assert!(protected > 0);
        let m = terrain.spec.grid.meters();
        let x = (96700. / m) as i64;
        let z = (47400. / m) as i64;
        let c = terrain.column(x, z, 1).unwrap();
        let top = CellPos::new(x, c.ground - 1, z);
        assert_eq!(
            terrain.color(top, MaterialId::GRAVEL, 1),
            terrain.color(top, c.surface, 1),
            "surface paint depends on occupied depth, never a filtered material mismatch"
        );
        for pass in 0..2 {
            let start = std::time::Instant::now();
            for i in 0..4096 {
                let c = terrain.column(x + i % 64, z + i / 64, 1).unwrap();
                std::hint::black_box(c);
            }
            eprintln!(
                "atlas column pass {pass}: {:.3} us/query",
                start.elapsed().as_secs_f64() * 1e6 / 4096.
            );
        }
    }
    #[test]
    fn rounded_extent_preserves_last_columns_at_every_scale() {
        for scale in [1, 2, 4, 8, 16, 32, 64, 100] {
            let extent = 1000000.;
            let world_extent = (extent / scale as f64).ceil();
            for spacing in [0.01, 0.25, 1., 4.] {
                let last = ((world_extent / spacing).ceil() - 1.) * spacing;
                let p = source_point([last, last], scale as f64, extent);
                assert!(p.iter().all(|v| *v >= 0. && *v <= extent));
            }
        }
        assert_eq!(source_point([333333.75; 2], 3., 1000000.), [1000000.; 2]);
    }
    #[test]
    fn shifted_vertical_range_is_checked_before_integer_conversion() {
        assert!(shifted_ceiling(30000., 10000., 1).is_err());
        assert_eq!(shifted_ceiling(30000., 10000., 2).unwrap(), 20064);
        assert!(shifted_ceiling(f64::INFINITY, 0., 1).is_err());
    }
    #[test]
    fn planar_cubic_has_slope_but_zero_curvature_residual() {
        let points = [[0., 0.], [100., 0.], [0., 100.]];
        let controls = [
            10.,
            110.,
            210.,
            10. + 100. / 3.,
            10. + 200. / 3.,
            110. + 100. / 3.,
            110. + 200. / 3.,
            10. + 400. / 3.,
            10. + 200. / 3.,
            110.,
        ];
        let (gradient, curvature) =
            cubic_derivatives(&controls, barycentric_gradients(points), [0.2, 0.3, 0.5]);
        assert!((gradient[0] - 1.).abs() < 1e-10);
        assert!((gradient[1] - 2.).abs() < 1e-10);
        assert!(curvature.iter().all(|v| v.abs() < 1e-10));
    }
    #[test]
    fn narrow_channel_footprint_is_an_actual_clipped_intersection() {
        let channel = refine::Channel {
            a: [-10., 20., 0.3],
            b: [20., 19., 0.3],
            width: 0.1,
            flow: 1.,
            local: false,
            compiled: false,
            profile: [0.; 17],
            bend: 0.,
        };
        let hit = channel_footprint(&channel, [0., 0.], [1., 1.]).unwrap();
        assert!((hit.point[1] - 0.3).abs() < 1e-12);
        assert!(hit.point[0] >= 0. && hit.point[0] <= 1.);
        assert!(channel_footprint(&channel, [0., 2.], [1., 3.]).is_none());
        assert!(clip_triangle([[0., 0.], [1., 0.], [0., 1.]], [0.8, 0.8], [0.9, 0.9]).is_empty());
    }
    #[test]
    fn hierarchy_retains_unsampled_peak_and_narrow_water() {
        let dry = Bound {
            low: 0.,
            high: 1.,
            wet: false,
            width: f64::INFINITY,
            point: None,
        };
        let feature = Bound {
            low: -4.,
            high: 180.,
            wet: true,
            width: 0.25,
            point: None,
        };
        let mut leaves = vec![dry; 16];
        leaves[6] = feature;
        let mut middle = vec![dry; 4];
        middle[1].include(feature);
        let mut root = dry;
        root.include(feature);
        let index = SummaryIndex {
            levels: vec![leaves, middle, vec![root]],
            side: 4,
        };
        let all = index.query([0., 0.], [4., 4.], 4.);
        assert_eq!(all.high, 180.);
        assert_eq!(all.low, -4.);
        assert!(all.wet);
        let local = index.query([2.2, 1.2], [2.3, 1.3], 4.);
        assert!(local.wet);
        assert_eq!(local.width, 0.25);
        assert_eq!(local.high, 180.);
        assert!(!index.query([0.1, 0.1], [0.2, 0.2], 4.).wet);
    }
}

fn chord_interval(
    a: [f64; 2],
    b: [f64; 2],
    half: f64,
    lo: [f64; 2],
    hi: [f64; 2],
) -> Option<(f64, f64)> {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let (mut start, mut end) = (0f64, 1f64);
    for axis in 0..2 {
        if delta[axis].abs() < 1e-12 {
            if a[axis] < lo[axis] - half || a[axis] > hi[axis] + half {
                return None;
            }
        } else {
            let a0 = (lo[axis] - half - a[axis]) / delta[axis];
            let a1 = (hi[axis] + half - a[axis]) / delta[axis];
            start = start.max(a0.min(a1));
            end = end.min(a0.max(a1));
        }
    }
    if end < start {
        None
    } else {
        Some((start, end))
    }
}
fn channel_interval(channel: &refine::Channel, lo: [f64; 2], hi: [f64; 2]) -> Option<(f64, f64)> {
    let steps = 16;
    let mut result: Option<(f64, f64)> = None;
    for i in 0..steps {
        let a = channel.position(i as f64 / steps as f64);
        let b = channel.position((i + 1) as f64 / steps as f64);
        let sagitta = channel.bend.abs() / (steps * steps) as f64;
        if let Some((start, end)) = chord_interval(
            [a[0], a[2]],
            [b[0], b[2]],
            channel.width * 0.5 + sagitta,
            lo,
            hi,
        ) {
            let interval = (
                (i as f64 + start) / steps as f64,
                (i as f64 + end) / steps as f64,
            );
            result = Some(result.map_or(interval, |old| {
                (old.0.min(interval.0), old.1.max(interval.1))
            }));
        }
    }
    result
}
fn channel_footprint(
    channel: &refine::Channel,
    lo: [f64; 2],
    hi: [f64; 2],
) -> Option<super::river::WaterFootprint> {
    let (start, end) = channel_interval(channel, lo, hi)?;
    // Find a curve point inside the rectangle when possible; otherwise choose
    // the closest curve point and clamp into the intersecting wet corridor.
    let mut t = (start + end) * 0.5;
    for i in 0..=16 {
        let q = channel.position(start + (end - start) * i as f64 / 16.);
        if q[0] >= lo[0] && q[0] <= hi[0] && q[2] >= lo[1] && q[2] <= hi[1] {
            t = start + (end - start) * i as f64 / 16.;
            break;
        }
    }
    let q = channel.position(t);
    let a = channel.position(start);
    let b = channel.position(end);
    Some(super::river::WaterFootprint {
        id: 0,
        point: [q[0].clamp(lo[0], hi[0]), q[2].clamp(lo[1], hi[1])],
        width_m: channel.width,
        coverage: (((b[0] - a[0]).hypot(b[2] - a[2]) * channel.width)
            / ((hi[0] - lo[0]) * (hi[1] - lo[1])).max(1e-12))
        .clamp(0.000001, 1.),
    })
}

fn barycentric_gradients(p: [[f64; 2]; 3]) -> [[f64; 2]; 3] {
    let dx = p[1][0] - p[0][0];
    let dz = p[1][1] - p[0][1];
    let ex = p[2][0] - p[0][0];
    let ez = p[2][1] - p[0][1];
    let determinant = dx * ez - dz * ex;
    let b = [ez / determinant, -ex / determinant];
    let c = [-dz / determinant, dx / determinant];
    [[-b[0] - c[0], -b[1] - c[1]], b, c]
}
fn barycentric(p: [[f64; 2]; 3], point: [f64; 2]) -> [f64; 3] {
    let g = barycentric_gradients(p);
    let b = g[1][0] * (point[0] - p[0][0]) + g[1][1] * (point[1] - p[0][1]);
    let c = g[2][0] * (point[0] - p[0][0]) + g[2][1] * (point[1] - p[0][1]);
    [1. - b - c, b, c]
}
fn clip_triangle(points: [[f64; 2]; 3], lo: [f64; 2], hi: [f64; 2]) -> Vec<[f64; 2]> {
    let mut polygon = points.to_vec();
    for (axis, limit, sign) in [
        (0, lo[0], 1.),
        (0, hi[0], -1.),
        (1, lo[1], 1.),
        (1, hi[1], -1.),
    ] {
        if polygon.is_empty() {
            break;
        }
        let mut next = Vec::new();
        let mut a = *polygon.last().unwrap();
        for &b in &polygon {
            let inside_a = (a[axis] - limit) * sign >= 0.;
            let inside_b = (b[axis] - limit) * sign >= 0.;
            if inside_a != inside_b {
                let t = (limit - a[axis]) / (b[axis] - a[axis]);
                next.push(std::array::from_fn(|i| a[i] + t * (b[i] - a[i])));
            }
            if inside_b {
                next.push(b);
            }
            a = b;
        }
        polygon = next;
    }
    polygon
}
fn cubic_derivatives(controls: &[f64; 10], g: [[f64; 2]; 3], w: [f64; 3]) -> ([f64; 2], [f64; 3]) {
    let terms = [
        ([3, 0, 0], 1.),
        ([0, 3, 0], 1.),
        ([0, 0, 3], 1.),
        ([2, 1, 0], 3.),
        ([1, 2, 0], 3.),
        ([0, 2, 1], 3.),
        ([0, 1, 2], 3.),
        ([1, 0, 2], 3.),
        ([2, 0, 1], 3.),
        ([1, 1, 1], 6.),
    ];
    let derivative = |w: [f64; 3], first: usize, second: Option<usize>| -> f64 {
        terms
            .iter()
            .zip(controls)
            .map(|((powers, coefficient), height)| {
                let mut exponents = *powers;
                let mut value = coefficient * height;
                if exponents[first] == 0 {
                    return 0.;
                }
                value *= exponents[first] as f64;
                exponents[first] -= 1;
                if let Some(second) = second {
                    if exponents[second] == 0 {
                        return 0.;
                    }
                    value *= exponents[second] as f64;
                    exponents[second] -= 1;
                }
                for i in 0..3 {
                    value *= w[i].powi(exponents[i]);
                }
                value
            })
            .sum()
    };
    let gradient =
        std::array::from_fn(|axis| (0..3).map(|i| derivative(w, i, None) * g[i][axis]).sum());
    let mut curvature = [0f64; 3];
    // A cubic's physical second derivatives are affine: extrema occur at
    // parent triangle corners, so these bounds hold throughout the clipped patch.
    for corner in [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]] {
        for (k, (a, b)) in [(0, 0), (0, 1), (1, 1)].into_iter().enumerate() {
            let mut value = 0.;
            for i in 0..3 {
                for j in 0..3 {
                    value += derivative(corner, i, Some(j)) * g[i][a] * g[j][b];
                }
            }
            curvature[k] = curvature[k].max(value.abs());
        }
    }
    (gradient, curvature)
}
