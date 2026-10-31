//! Generation is a baseline provider, never a camera-dependent world model.
mod climate;
pub mod atlas;
pub mod atlas_climate;
pub mod atlas_hydrology;
pub mod geographic_atlas;
pub mod landscape_experiment;
pub mod landscape_paint;
mod landscape_mesh;
pub mod debug;
mod environment;
pub mod features;
mod geomorph;
mod landforms;
mod noise;
mod provinces;
pub mod river;
mod source;
pub mod summary;
pub mod survey;
pub use source::Terrain;
mod terrain;
use crate::{
    grid::CellPos,
    materials::{MaterialId, Voxel},
    painting,
};
pub use environment::{
    Biome, ClimatePreset, EnvironmentSample, Geology, Landform, Refinement, TerrainConfig,
};
use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    sync::{Arc, Mutex},
};

pub const GENERATOR_VERSION: u32 = 6;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratorIdentity {
    pub seed: u64,
    pub version: u32,
    pub definition: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: i64,
    pub end: i64,
    pub material: MaterialId,
}

/// A terrain optimization. The world API also supports arbitrary 3D generators.
#[derive(Clone, Copy, Debug)]
pub struct Column {
    pub ground: i64,
    pub water: i64,
    pub bedrock: i64,
    pub surface: MaterialId,
    pub subsoil: MaterialId,
    pub rock: MaterialId,
}
impl Column {
    pub fn material_at(self, y: i64) -> MaterialId {
        if y < 0 || y >= self.water.max(self.ground) {
            MaterialId::AIR
        } else if y >= self.ground {
            MaterialId::WATER
        } else if y == self.ground - 1 {
            self.surface
        } else if y < self.bedrock {
            self.rock
        } else {
            self.subsoil
        }
    }
    pub fn runs(self, range: Range<i64>) -> Vec<Run> {
        let mut cuts = vec![range.start, range.end];
        for y in [
            0,
            self.bedrock.min(self.ground - 1),
            self.ground - 1,
            self.ground,
            self.water,
        ] {
            if y > range.start && y < range.end {
                cuts.push(y);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut result = Vec::new();
        for pair in cuts.windows(2) {
            push_run(
                &mut result,
                Run {
                    start: pair[0],
                    end: pair[1],
                    material: self.material_at(pair[0]),
                },
            );
        }
        result
    }
}
pub fn push_run(runs: &mut Vec<Run>, run: Run) {
    if run.start >= run.end {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.end == run.start && last.material == run.material {
            last.end = run.end;
            return;
        }
    }
    runs.push(run);
}

pub trait Generator: Send + Sync {
    fn water_footprint(&self, _origin: [f64; 2], _edge: f64) -> Option<river::WaterFootprint> {
        None
    }
    fn display_solids(&self, _x: i64, _z: i64, _step: i64) -> Vec<Run> {
        Vec::new()
    }
    fn has_volumetric_features(&self, _origin: [f64; 2], _edge: f64) -> bool {
        false
    }
    /// Render-only feature significance. Custom generators conservatively keep
    /// their full volumetric path unless they provide a size-aware override.
    fn has_display_volumetric_features(&self, origin: [f64; 2], edge: f64, _cell_m: f64) -> bool {
        self.has_volumetric_features(origin, edge)
    }
    fn spec(&self) -> crate::WorldSpec {
        crate::WorldSpec::default()
    }
    fn display_voids(&self, _x: i64, _z: i64, _step: i64) -> Vec<[i64; 2]> {
        Vec::new()
    }
    fn terrain_summary(&self, _origin: [f64; 2], _edge: f64) -> Option<summary::TerrainSummary> {
        None
    }
    fn identity(&self) -> GeneratorIdentity;
    /// None means unknown/unprovided world, never implicit air.
    fn voxel(&self, pos: CellPos) -> Option<Voxel>;
    /// Long-term baseline surface environment. Y does not select a live air parcel.
    fn environment_at(&self, _pos: CellPos) -> Option<EnvironmentSample> {
        None
    }
    fn appearance_at(&self, pos: CellPos, _material: MaterialId) -> crate::materials::Appearance {
        painting::appearance(pos, self.identity().seed)
    }
    fn display_appearance_at(
        &self,
        pos: CellPos,
        material: MaterialId,
        _step: i64,
    ) -> crate::materials::Appearance {
        self.appearance_at(pos, material)
    }
    fn column_hint(&self, _x: i64, _z: i64) -> Option<Column> {
        None
    }
    /// Render-only band-limited column; authoritative queries never call this.
    fn display_column(&self, x: i64, z: i64, _spacing: i64) -> Option<Column> {
        self.column_hint(x, z)
    }
    /// Column and its already band-limited appearance inputs, evaluated together.
    fn display_sample(
        &self,
        x: i64,
        z: i64,
        spacing: i64,
    ) -> Option<(Column, Option<EnvironmentSample>)> {
        Some((self.display_column(x, z, spacing)?, None))
    }
    /// Analytic procedural coverage for distant color filtering; edits bypass it.
    fn surface_cover_at(&self, _pos: CellPos) -> Option<[(MaterialId, f32); 5]> {
        None
    }
    fn display_cover_at(&self, _pos: CellPos, _spacing: i64) -> Option<[(MaterialId, f32); 5]> {
        None
    }
    fn runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<Run>> {
        if range.end.checked_sub(range.start)? > 1_000_000 {
            return None;
        }
        let mut result = Vec::new();
        for y in range {
            push_run(
                &mut result,
                Run {
                    start: y,
                    end: y + 1,
                    material: self.voxel(CellPos::new(x, y, z))?.material,
                },
            );
        }
        Some(result)
    }
}

#[derive(Clone, Copy)]
struct GeneratedColumn {
    column: Column,
    environment: EnvironmentSample,
}
#[derive(Default)]
struct ColumnCache {
    values: HashMap<(i64, i64), GeneratedColumn>,
    fifo: VecDeque<(i64, i64)>,
}
/// Sixteen independent small FIFO caches avoid one global lock on hot queries.
pub struct ProceduralTerrain {
    spec: crate::WorldSpec,
    source_extent_m: u32,
    world_scale: u32,
    seed: u64,
    config: TerrainConfig,
    region: Arc<terrain::Region>,
    features: Arc<features::FeatureIndex>,
    columns: [Mutex<ColumnCache>; 16],
}
impl ProceduralTerrain {
    pub fn new(seed: u64) -> Self {
        Self::with_config(seed, TerrainConfig::default()).expect("valid default climate")
    }
    pub fn with_config(seed: u64, config: TerrainConfig) -> Result<Self, String> {
        Self::with_spec(seed, config, crate::WorldSpec::default())
    }
    pub fn with_spec(
        seed: u64,
        mut config: TerrainConfig,
        spec: crate::WorldSpec,
    ) -> Result<Self, String> {
        let spec = spec.validate()?;
        // Legacy regional indexes were sized for this domain. The larger world
        // envelope is supported by the saved atlas, not those dense indexes.
        if spec.extent_m > 100_000 || spec.ceiling_m > 2048 {
            return Err("legacy procedural terrain supports at most 100 km and a 2048 m ceiling; use the atlas source for larger worlds".into());
        }
        config.validate()?;
        config.wind_degrees = config.wind_degrees.rem_euclid(360.);
        if config.wind_degrees == 0. {
            config.wind_degrees = 0.;
        }
        let region = Arc::new(terrain::Region::generate(
            seed,
            config,
            spec.extent_m,
            spec.ceiling_m,
        ));
        let features = Arc::new(features::FeatureIndex::generate(&region, seed));
        Ok(Self {
            spec,
            source_extent_m: spec.extent_m,
            world_scale: 1,
            seed,
            config,
            region,
            features,
            columns: std::array::from_fn(|_| Mutex::new(ColumnCache::default())),
        })
    }
    /// Compress the same source landscape uniformly in X/Y/Z. Voxel size and
    /// player dimensions stay in game meters. Regional data is shared.
    pub fn at_world_scale(&self, world_scale: u32) -> Result<Self, String> {
        Self::validate_world_scale(world_scale)?;
        let mut result = self.at_grid(self.spec.grid);
        result.spec.extent_m = (self.source_extent_m + world_scale - 1) / world_scale;
        result.world_scale = world_scale;
        if world_scale != self.world_scale {
            result.features = Arc::new(
                self.features
                    .rescaled(self.world_scale as f64 / world_scale as f64),
            );
        }
        Ok(result)
    }
    pub fn validate_world_scale(world_scale: u32) -> Result<(), String> {
        if !(1..=100).contains(&world_scale) {
            return Err("world scale must be 1..100 terrain meters per game meter".into());
        }
        Ok(())
    }
    pub fn with_world_scale(
        seed: u64,
        config: TerrainConfig,
        spec: crate::WorldSpec,
        world_scale: u32,
    ) -> Result<Self, String> {
        Self::validate_world_scale(world_scale)?;
        Self::with_spec(seed, config, spec)?.at_world_scale(world_scale)
    }
    pub fn world_scale(&self) -> u32 {
        self.world_scale
    }
    fn surface_m(&self, x: f64, z: f64, spacing: f64) -> terrain::Surface {
        let scale = self.world_scale as f64;
        let mut s = terrain::terrain_surface(
            &self.region,
            x * scale,
            z * scale,
            spacing * scale,
            self.seed,
        );
        s.height /= scale;
        s.water /= scale;
        s.bedrock /= scale;
        // Internal climate/material suitability retains source-world units.
        // Public geometry/environment APIs convert lengths and areas below.
        s
    }
    fn game_environment(&self, mut e: EnvironmentSample) -> EnvironmentSample {
        let scale = self.world_scale as f32;
        e.elevation_m /= scale;
        e.soil_depth_m /= scale;
        e.drainage_area_m2 /= scale * scale;
        e
    }
    pub fn config(&self) -> TerrainConfig {
        self.config
    }
    pub fn at_grid(&self, grid: crate::GridScale) -> Self {
        Self {
            seed: self.seed,
            config: self.config,
            source_extent_m: self.source_extent_m,
            world_scale: self.world_scale,
            spec: crate::WorldSpec { grid, ..self.spec },
            region: Arc::clone(&self.region),
            features: Arc::clone(&self.features),
            columns: std::array::from_fn(|_| Mutex::new(ColumnCache::default())),
        }
    }
    pub fn with_refinement(&self, refinement: Refinement) -> Self {
        let mut result = self.at_grid(self.spec.grid);
        result.config.refinement = refinement;
        let mut region = (*self.region).clone();
        region.set_refinement(refinement);
        result.region = Arc::new(region);
        result.features = Arc::new(
            features::FeatureIndex::generate(&result.region, self.seed)
                .rescaled(1. / self.world_scale as f64),
        );
        result
    }
    pub fn geological_features(&self) -> &[features::GeologicalFeature] {
        &self.features.descriptors
    }
    pub fn sample_m(&self, x: f64, z: f64) -> Option<SurfaceSample> {
        if !x.is_finite()
            || !z.is_finite()
            || x < 0.
            || z < 0.
            || x >= self.spec.extent_m as f64
            || z >= self.spec.extent_m as f64
        {
            return None;
        }
        let s = self.surface_m(x, z, 0.001 / self.world_scale as f64);
        Some(SurfaceSample {
            height_m: s.height,
            water_m: s.water,
            bedrock_m: s.bedrock,
            material: s.surface,
            environment: self.game_environment(s.environment),
        })
    }
    /// Counts bounded drainage corrections and routes left unresolved.
    pub fn river_diagnostics(&self) -> river::RiverDiagnostics {
        self.region.river_diagnostics()
    }
    /// Persistent regional arrays and channel bins; excludes allocator overhead,
    /// temporary generation work and the separately bounded hot column caches.
    pub fn regional_memory_bytes(&self) -> usize {
        self.region.memory_bytes()
    }
    pub fn cached_columns(&self) -> usize {
        self.columns
            .iter()
            .map(|c| c.lock().unwrap().values.len())
            .sum()
    }
    pub fn debug_image(
        &self,
        layer: debug::DebugLayer,
        size: usize,
    ) -> Result<debug::DebugImage, String> {
        debug::image(size, self.source_extent_m as f64, layer, |x, z| {
            self.region.environment_at(x, z)
        })
    }
    pub fn environment_statistics(&self) -> debug::EnvironmentStats {
        debug::statistics(128, self.source_extent_m as f64, |x, z| {
            self.region.environment_at(x, z)
        })
    }
    pub fn extent_cells(&self) -> i64 {
        self.spec.extent_cells()
    }
    fn generated(&self, x: i64, z: i64) -> Option<GeneratedColumn> {
        if !(0..self.spec.extent_cells()).contains(&x)
            || !(0..self.spec.extent_cells()).contains(&z)
        {
            return None;
        }
        let shard = (painting::hash(x as u64 ^ (z as u64).rotate_left(31)) & 15) as usize;
        if let Some(column) = self.columns[shard]
            .lock()
            .unwrap()
            .values
            .get(&(x, z))
            .copied()
        {
            return Some(column);
        }
        // Always sample the native world resolution. LOD only changes which
        // columns the renderer requests, never the contents of those columns.
        let sample = self.surface_m(
            x as f64 * self.spec.grid.meters(),
            z as f64 * self.spec.grid.meters(),
            0.001 / self.world_scale as f64,
        );
        let ground = (sample.height / self.spec.grid.meters()).floor() as i64;
        let water = if sample.water > 0.0 {
            (sample.water / self.spec.grid.meters()).floor() as i64
        } else {
            ground
        };
        let column = Column {
            ground,
            water: water.max(ground),
            bedrock: (sample.bedrock / self.spec.grid.meters()).floor() as i64,
            surface: sample.surface,
            subsoil: sample.subsoil,
            rock: sample.rock,
        };
        let generated = GeneratedColumn {
            column,
            environment: sample.environment,
        };
        let mut cache = self.columns[shard].lock().unwrap();
        if !cache.values.contains_key(&(x, z)) {
            if cache.values.len() >= 2048 {
                if let Some(old) = cache.fifo.pop_front() {
                    cache.values.remove(&old);
                }
            }
            cache.fifo.push_back((x, z));
            cache.values.insert((x, z), generated);
        }
        Some(generated)
    }
}
impl Generator for ProceduralTerrain {
    fn water_footprint(&self, origin: [f64; 2], edge: f64) -> Option<river::WaterFootprint> {
        let scale = self.world_scale as f64;
        let mut hit = self
            .region
            .water_footprint(origin.map(|v| v * scale), edge * scale)?;
        hit.point = hit.point.map(|v| v / scale);
        hit.width_m /= scale;
        Some(hit)
    }
    fn has_volumetric_features(&self, origin: [f64; 2], edge: f64) -> bool {
        self.features.overlaps(origin, edge)
    }
    fn has_display_volumetric_features(&self, origin: [f64; 2], edge: f64, cell_m: f64) -> bool {
        self.features.overlaps_resolved(origin, edge, cell_m)
    }
    fn display_cover_at(&self, pos: CellPos, spacing: i64) -> Option<[(MaterialId, f32); 5]> {
        let size = spacing as f64 * self.spec.grid.meters();
        if size * (self.world_scale as f64) < 4. {
            return None;
        }
        let center = self.spec.grid.point(pos);
        let mut result = [(MaterialId::AIR, 0f32); 5];
        for z in 0..3 {
            for x in 0..3 {
                let px = (center[0] + (x as f64 - 1.) * size / 3.)
                    .clamp(0., self.spec.extent_m as f64 - 0.001);
                let pz = (center[2] + (z as f64 - 1.) * size / 3.)
                    .clamp(0., self.spec.extent_m as f64 - 0.001);
                let s = self.surface_m(px, pz, size / 3.);
                let slot = result
                    .iter()
                    .position(|v| v.0 == s.surface)
                    .or_else(|| result.iter().position(|v| v.1 == 0.))
                    .unwrap_or(4);
                result[slot].0 = s.surface;
                result[slot].1 += 1. / 9.;
            }
        }
        Some(result)
    }
    fn terrain_summary(&self, origin: [f64; 2], edge: f64) -> Option<summary::TerrainSummary> {
        self.summarize_m(origin, edge)
    }
    fn spec(&self) -> crate::WorldSpec {
        self.spec
    }
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: self.seed,
            version: GENERATOR_VERSION,
            definition: 0x5445525241494e32
                ^ painting::hash(self.source_extent_m as u64 ^ 0x534f55524345)
                ^ self.config.fingerprint()
                ^ if self.world_scale == 1 {
                    0
                } else {
                    painting::hash(self.world_scale as u64 ^ 0x574f524c44534341)
                },
        }
    }
    fn voxel(&self, pos: CellPos) -> Option<Voxel> {
        let generated = self.generated(pos.x, pos.z)?;
        let mut material = generated.column.material_at(pos.y);
        if self
            .display_solids(pos.x, pos.z, 1)
            .iter()
            .any(|r| r.start <= pos.y && pos.y < r.end)
        {
            material = MaterialId::SANDSTONE;
        }
        if material != MaterialId::AIR
            && material != MaterialId::WATER
            && self
                .display_voids(pos.x, pos.z, 1)
                .iter()
                .any(|r| r[0] <= pos.y && pos.y < r[1])
        {
            material = MaterialId::AIR;
        }
        Some(if material == MaterialId::AIR {
            Voxel::AIR
        } else {
            Voxel::new(
                material,
                painting::environment_appearance_m(
                    self.spec.grid.point(pos),
                    self.seed,
                    material,
                    generated.environment,
                ),
            )
        })
    }
    fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
        let mut c = self.generated(x, z)?.column;
        if let Some(top) = self.display_solids(x, z, 1).iter().map(|r| r.end).max() {
            if top > c.ground {
                c.ground = top;
                c.water = c.water.max(top);
                c.surface = MaterialId::SANDSTONE;
            }
        }
        Some(c)
    }
    fn display_column(&self, x: i64, z: i64, spacing: i64) -> Option<Column> {
        Some(self.display_sample(x, z, spacing)?.0)
    }
    fn display_sample(
        &self,
        x: i64,
        z: i64,
        spacing: i64,
    ) -> Option<(Column, Option<EnvironmentSample>)> {
        if !(0..self.spec.extent_cells()).contains(&x)
            || !(0..self.spec.extent_cells()).contains(&z)
        {
            return None;
        }
        let s = self.surface_m(
            x as f64 * self.spec.grid.meters(),
            z as f64 * self.spec.grid.meters(),
            spacing.max(1) as f64 * self.spec.grid.meters(),
        );
        let ground = (s.height / self.spec.grid.meters()).floor() as i64;
        Some((
            Column {
                ground,
                water: if s.water > 0. {
                    (s.water / self.spec.grid.meters()).floor() as i64
                } else {
                    ground
                }
                .max(ground),
                bedrock: (s.bedrock / self.spec.grid.meters()).floor() as i64,
                surface: s.surface,
                subsoil: s.subsoil,
                rock: s.rock,
            },
            Some(self.game_environment(s.environment)),
        ))
    }
    fn surface_cover_at(&self, pos: CellPos) -> Option<[(MaterialId, f32); 5]> {
        let generated = self.generated(pos.x, pos.z)?;
        // Keep explicitly carved channel banks out of climate blending.
        if matches!(
            generated.column.surface,
            MaterialId::GRAVEL | MaterialId::SILT
        ) {
            return None;
        }
        Some(generated.environment.surface_cover(generated.column.rock))
    }
    fn environment_at(&self, pos: CellPos) -> Option<EnvironmentSample> {
        if !(0..self.spec.extent_cells()).contains(&pos.x)
            || !(0..self.spec.extent_cells()).contains(&pos.z)
        {
            return None;
        }
        let scale = self.spec.grid.meters() * self.world_scale as f64;
        Some(
            self.game_environment(
                self.region
                    .environment_at(pos.x as f64 * scale, pos.z as f64 * scale),
            ),
        )
    }
    fn appearance_at(&self, pos: CellPos, material: MaterialId) -> crate::materials::Appearance {
        self.generated(pos.x, pos.z).map_or_else(
            || painting::appearance(pos, self.seed),
            |g| {
                painting::environment_appearance_m(
                    self.spec.grid.point(pos),
                    self.seed,
                    material,
                    g.environment,
                )
            },
        )
    }
    fn runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<Run>> {
        Some(features::carve(
            features::add_solids(
                self.generated(x, z)?.column.runs(range),
                &self.display_solids(x, z, 1),
            ),
            &self.display_voids(x, z, 1),
        ))
    }
    fn display_voids(&self, x: i64, z: i64, step: i64) -> Vec<[i64; 2]> {
        let scale = self.spec.grid.meters();
        self.features
            .intervals(
                (x as f64 + step as f64 * 0.5) * scale,
                (z as f64 + step as f64 * 0.5) * scale,
            )
            .into_iter()
            .filter_map(|r| {
                let low = (r[0] / scale - 0.5).ceil() as i64;
                let high = (r[1] / scale - 0.5).ceil() as i64;
                (high > low).then_some([low, high])
            })
            .collect()
    }
    fn display_solids(&self, x: i64, z: i64, step: i64) -> Vec<Run> {
        let scale = self.spec.grid.meters();
        let px = (x as f64 + step as f64 * 0.5) * scale;
        let pz = (z as f64 + step as f64 * 0.5) * scale;
        let Some(top) = self.features.rock_top(px, pz) else {
            return Vec::new();
        };
        let ground = if step == 1 {
            self.generated(x, z)
                .map_or(top, |c| c.column.ground as f64 * scale)
        } else {
            self.surface_m(px, pz, step as f64 * scale).height
        };
        let start = (ground / scale).floor() as i64;
        let end = (top / scale - 0.5).ceil() as i64;
        if end > start {
            vec![Run {
                start,
                end,
                material: MaterialId::SANDSTONE,
            }]
        } else {
            Vec::new()
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceSample {
    pub height_m: f64,
    pub water_m: f64,
    pub bedrock_m: f64,
    pub material: MaterialId,
    pub environment: EnvironmentSample,
}

/// Cheap deterministic generator for tests and small simulation experiments.
#[derive(Clone, Copy, Debug)]
pub struct FlatTerrain {
    pub height: i64,
    pub water: i64,
    pub seed: u64,
}
impl Generator for FlatTerrain {
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: self.seed,
            version: GENERATOR_VERSION,
            definition: painting::hash(self.height as u64)
                ^ painting::hash(self.water as u64).rotate_left(23)
                ^ 0x464c4154,
        }
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        let material = self.column_hint(p.x, p.z)?.material_at(p.y);
        Some(if material == MaterialId::AIR {
            Voxel::AIR
        } else {
            Voxel::new(material, painting::appearance(p, self.seed))
        })
    }
    fn column_hint(&self, _x: i64, _z: i64) -> Option<Column> {
        Some(Column {
            ground: self.height,
            water: self.water.max(self.height),
            bedrock: (self.height - 3).max(0),
            surface: MaterialId::GRASS,
            subsoil: MaterialId::SOIL,
            rock: MaterialId::ROCK,
        })
    }
    fn runs(&self, x: i64, z: i64, range: Range<i64>) -> Option<Vec<Run>> {
        Some(self.column_hint(x, z)?.runs(range))
    }
}
