//! Bounded opt-in CPU contract for GPU appearance construction. Canonical source
//! values remain CPU authoritative; only palette/face integration is deferred.
use crate::{
    adaptive_geometry::TileKey,
    gpu_ordered_profiles::{CachedPage, ProfileJob, WIDTH},
};
use frontier_world::World;

pub(crate) fn enabled() -> bool {
    std::env::var_os("FRONTIER_GPU_PROFILES").is_some_and(|v| v == "1")
}

pub(crate) fn eligible(world: &World, key: TileKey) -> bool {
    world.fbm_terrain().is_some()
        && world.topology_base_cells().is_none()
        && world.revision() == 0
        && key.source_revision == 0
        && world.edit_cells().next().is_none()
        && key.width_cells == WIDTH as u32
        && key.step_cells >= 8
        && key.step_cells <= key.width_cells
        && key.width_cells % key.step_cells == 0
}

/// Owned immutable source snapshot. Keeping this on the tile preserves lifetime
/// across cache eviction, asynchronous publication and source-cache replacement.
/// Profile zero is reserved for exact faces; job n produces profile n+1.
pub struct DeferredProfiles {
    pub page: CachedPage,
    pub jobs: Vec<ProfileJob>,
    pub source_revision: u64,
}
impl DeferredProfiles {
    pub fn payload_bytes(&self) -> usize {
        self.page.payload_bytes() + self.jobs.capacity() * std::mem::size_of::<ProfileJob>()
    }
    pub fn profile_count(&self) -> usize {
        self.jobs.len() + 1
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GpuProfilePreparationStats {
    pub enabled: bool,
    pub eligible_tiles: u64,
    pub deferred_tiles: u64,
    pub deferred_jobs: u64,
    pub fallback_tiles: u64,
    pub snapshot_generated_columns: u64,
    pub snapshot_ms: f64,
}
impl GpuProfilePreparationStats {
    pub(crate) fn merge(&mut self, other: Self) {
        self.eligible_tiles += other.eligible_tiles;
        self.deferred_tiles += other.deferred_tiles;
        self.deferred_jobs += other.deferred_jobs;
        self.fallback_tiles += other.fallback_tiles;
        self.snapshot_generated_columns += other.snapshot_generated_columns;
        self.snapshot_ms += other.snapshot_ms;
    }
}
