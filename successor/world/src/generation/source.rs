//! Runtime-selectable baselines. Renderers, collision and edits share one source.
use super::*;
use std::{path::Path, sync::Arc};
#[path = "atlas/cache.rs"]
mod atlas_cache;

#[derive(Clone)]
pub enum Terrain {
    Procedural(Arc<ProceduralTerrain>),
    Survey(Arc<survey::Survey>),
    Atlas(Arc<atlas::AtlasTerrain>),
}
impl Terrain {
    pub fn from_atlas(path: &Path, grid: crate::GridScale, scale: u32) -> Result<Self, String> {
        Ok(Self::Atlas(Arc::new(atlas::AtlasTerrain::load(
            path, grid, scale,
        )?)))
    }
    pub fn prepare_atlas(seed: u64, grid: crate::GridScale, scale: u32) -> Result<Self, String> {
        atlas_cache::prepare_atlas(seed, grid, scale)
    }
    pub fn is_atlas(&self) -> bool {
        matches!(self, Self::Atlas(_))
    }
    pub fn new(seed: u64) -> Self {
        Self::Procedural(Arc::new(ProceduralTerrain::new(seed)))
    }
    pub fn with_config(seed: u64, config: TerrainConfig) -> Result<Self, String> {
        Ok(Self::Procedural(Arc::new(ProceduralTerrain::with_config(
            seed, config,
        )?)))
    }
    pub fn with_spec(
        seed: u64,
        config: TerrainConfig,
        spec: crate::WorldSpec,
    ) -> Result<Self, String> {
        Ok(Self::Procedural(Arc::new(ProceduralTerrain::with_spec(
            seed, config, spec,
        )?)))
    }
    pub fn with_world_scale(
        seed: u64,
        config: TerrainConfig,
        spec: crate::WorldSpec,
        world_scale: u32,
    ) -> Result<Self, String> {
        Ok(Self::Procedural(Arc::new(
            ProceduralTerrain::with_world_scale(seed, config, spec, world_scale)?,
        )))
    }
    pub fn world_scale(&self) -> u32 {
        match self {
            Self::Procedural(p) => p.world_scale(),
            Self::Survey(_) => 1,
            Self::Atlas(a) => a.world_scale(),
        }
    }
    pub fn from_survey(path: &Path) -> crate::Result<Self> {
        Ok(Self::Survey(Arc::new(survey::Survey::load(path)?)))
    }
    pub fn survey(&self) -> Option<&survey::Survey> {
        match self {
            Self::Survey(s) => Some(s),
            _ => None,
        }
    }
    fn generator(&self) -> &dyn Generator {
        match self {
            Self::Procedural(p) => p.as_ref(),
            Self::Survey(s) => s.as_ref(),
            Self::Atlas(a) => a.as_ref(),
        }
    }
    pub fn config(&self) -> Option<TerrainConfig> {
        match self {
            Self::Procedural(p) => Some(p.config()),
            _ => None,
        }
    }
    pub fn regional_memory_bytes(&self) -> usize {
        match self {
            Self::Procedural(p) => p.regional_memory_bytes(),
            Self::Survey(s) => s.memory_bytes(),
            Self::Atlas(a) => a.memory_bytes(),
        }
    }
    pub fn environment_statistics(&self) -> String {
        match self {
            Self::Procedural(p) => p.environment_statistics().to_string(),
            Self::Atlas(_) => {
                "Compiled ocean-transport climate, hydrology and geographic materials.".into()
            }
            _ => "Measured surface and aerial colors; climate/material classes are not surveyed."
                .into(),
        }
    }
    pub fn debug_image(
        &self,
        layer: debug::DebugLayer,
        size: usize,
    ) -> Result<debug::DebugImage, String> {
        match self {
            Self::Procedural(p) => p.debug_image(layer, size),
            _ => Err("Climate maps require --world procedural".into()),
        }
    }
    pub fn sample_m(&self, x: f64, z: f64) -> Option<SurfaceSample> {
        match self {
            Self::Procedural(p) => p.sample_m(x, z),
            _ => None,
        }
    }
    pub fn at_grid(&self, grid: crate::GridScale) -> Self {
        match self {
            Self::Procedural(p) => Self::Procedural(Arc::new(p.at_grid(grid))),
            Self::Atlas(a) => Self::Atlas(Arc::new(a.at_grid(grid))),
            Self::Survey(s) => {
                assert_eq!(
                    grid,
                    s.spec().grid,
                    "Survey retains its measured native spacing"
                );
                self.clone()
            }
        }
    }
    pub fn with_refinement(&self, r: Refinement) -> Self {
        match self {
            Self::Procedural(p) => Self::Procedural(Arc::new(p.with_refinement(r))),
            _ => self.clone(),
        }
    }
    pub fn geological_features(&self) -> &[features::GeologicalFeature] {
        match self {
            Self::Procedural(p) => p.geological_features(),
            _ => &[],
        }
    }
}
impl Generator for Terrain {
    fn water_footprint(&self, o: [f64; 2], e: f64) -> Option<river::WaterFootprint> {
        self.generator().water_footprint(o, e)
    }
    fn spec(&self) -> crate::WorldSpec {
        self.generator().spec()
    }
    fn identity(&self) -> GeneratorIdentity {
        self.generator().identity()
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        self.generator().voxel(p)
    }
    fn column_hint(&self, x: i64, z: i64) -> Option<Column> {
        self.generator().column_hint(x, z)
    }
    fn display_column(&self, x: i64, z: i64, step: i64) -> Option<Column> {
        self.generator().display_column(x, z, step)
    }
    fn display_sample(
        &self,
        x: i64,
        z: i64,
        step: i64,
    ) -> Option<(Column, Option<EnvironmentSample>)> {
        self.generator().display_sample(x, z, step)
    }
    fn appearance_at(&self, p: CellPos, m: MaterialId) -> crate::materials::Appearance {
        self.generator().appearance_at(p, m)
    }
    fn display_appearance_at(
        &self,
        p: CellPos,
        m: MaterialId,
        step: i64,
    ) -> crate::materials::Appearance {
        self.generator().display_appearance_at(p, m, step)
    }
    fn environment_at(&self, p: CellPos) -> Option<EnvironmentSample> {
        self.generator().environment_at(p)
    }
    fn runs(&self, x: i64, z: i64, r: Range<i64>) -> Option<Vec<Run>> {
        self.generator().runs(x, z, r)
    }
    fn terrain_summary(&self, o: [f64; 2], e: f64) -> Option<summary::TerrainSummary> {
        self.generator().terrain_summary(o, e)
    }
    fn has_volumetric_features(&self, o: [f64; 2], e: f64) -> bool {
        self.generator().has_volumetric_features(o, e)
    }
    fn has_display_volumetric_features(&self, o: [f64; 2], e: f64, cell_m: f64) -> bool {
        self.generator()
            .has_display_volumetric_features(o, e, cell_m)
    }
    fn display_solids(&self, x: i64, z: i64, s: i64) -> Vec<Run> {
        self.generator().display_solids(x, z, s)
    }
    fn display_voids(&self, x: i64, z: i64, s: i64) -> Vec<[i64; 2]> {
        self.generator().display_voids(x, z, s)
    }
    fn surface_cover_at(&self, p: CellPos) -> Option<[(MaterialId, f32); 5]> {
        self.generator().surface_cover_at(p)
    }
    fn display_cover_at(&self, p: CellPos, s: i64) -> Option<[(MaterialId, f32); 5]> {
        self.generator().display_cover_at(p, s)
    }
}
