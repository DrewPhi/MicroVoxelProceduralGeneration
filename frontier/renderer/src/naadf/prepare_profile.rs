//! Optional coarse CPU preparation instrumentation. No clocks run per voxel.
use std::{fmt::Write, sync::OnceLock, time::Instant};

pub(super) const STAGES: usize = 18;
const NAMES: [&str; STAGES] = [
    "initialization",
    "vegetation_enumeration",
    "vegetation_models",
    "vegetation_assembly",
    "vegetation_overlap",
    "edit_index",
    "column_sampling",
    "chunk_dispatch_classification",
    "native_cell_assembly",
    "leaf_hierarchy_dedup",
    "block_proofs",
    "root_proofs",
    "ownership",
    "leaf_encoding",
    "sparse_roots",
    "parallel_regions_wall",
    "stream_leaf_remap_dedup",
    "stream_block_remap_writes",
];

#[repr(usize)]
#[derive(Clone, Copy)]
pub(super) enum Stage {
    Initialization,
    VegetationEnumeration,
    VegetationModels,
    VegetationAssembly,
    VegetationOverlap,
    EditIndex,
    ColumnSampling,
    ChunkDispatchClassification,
    NativeCellAssembly,
    LeafHierarchyDedup,
    BlockProofs,
    RootProofs,
    Ownership,
    LeafEncoding,
    SparseRoots,
    ParallelRegionsWall,
    StreamLeafRemapDedup,
    StreamBlockRemapWrites,
}

pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var_os("FRONTIER_NAADF_CPU_PROFILE").is_some_and(|v| v == "1"))
}
pub(super) fn stack_blocks() -> bool {
    static STACK: OnceLock<bool> = OnceLock::new();
    *STACK.get_or_init(|| {
        std::env::var_os("FRONTIER_NAADF_STACK_BLOCK_BOUNDS").is_some_and(|v| v == "1")
    })
}

pub(super) fn column_extrema() -> bool {
    static EXTREMA: OnceLock<bool> = OnceLock::new();
    *EXTREMA
        .get_or_init(|| std::env::var_os("FRONTIER_NAADF_COLUMN_EXTREMA").is_some_and(|v| v == "1"))
}

#[derive(Clone, Debug, Default)]
pub(super) struct Metrics {
    pub ms: [f64; STAGES],
    pub regions: u64,
    pub region_worker_wall_ms: f64,
    pub region_unclassified_wall_ms: f64,
    pub sampled_columns: u64,
    pub mixed_chunks: u64,
    pub requested_leaves: u64,
    pub unique_leaves: u64,
    pub column_vec_allocations: u64,
    pub block_vec_allocations: u64,
    pub column_vec_bytes: u64,
    pub block_vec_bytes: u64,
    pub stream_write_vec_allocations: u64,
    pub stream_write_vec_bytes: u64,
    pub region_workspace_estimate_peak_bytes: usize,
}
impl Metrics {
    pub fn merge(&mut self, other: &Self) {
        for (a, b) in self.ms.iter_mut().zip(other.ms) {
            *a += b;
        }
        self.regions += other.regions;
        self.region_worker_wall_ms += other.region_worker_wall_ms;
        self.region_unclassified_wall_ms += other.region_unclassified_wall_ms;
        self.sampled_columns += other.sampled_columns;
        self.mixed_chunks += other.mixed_chunks;
        self.requested_leaves += other.requested_leaves;
        self.unique_leaves += other.unique_leaves;
        self.column_vec_allocations += other.column_vec_allocations;
        self.block_vec_allocations += other.block_vec_allocations;
        self.column_vec_bytes += other.column_vec_bytes;
        self.block_vec_bytes += other.block_vec_bytes;
        self.stream_write_vec_allocations += other.stream_write_vec_allocations;
        self.stream_write_vec_bytes += other.stream_write_vec_bytes;
        self.region_workspace_estimate_peak_bytes = self
            .region_workspace_estimate_peak_bytes
            .max(other.region_workspace_estimate_peak_bytes);
    }
    pub fn json(&self) -> String {
        let mut json = String::from("{\"stage_ms\":{");
        for (i, (name, ms)) in NAMES.iter().zip(self.ms).enumerate() {
            write!(json, "{}{name:?}:{ms}", if i == 0 { "" } else { "," }).unwrap();
        }
        write!(
            json,
            concat!(
                "}},\"regions\":{},\"region_worker_wall_ms\":{},\"region_unclassified_wall_ms\":{},",
                "\"sampled_columns\":{},\"mixed_chunks\":{},",
                "\"requested_leaves\":{},\"unique_leaves\":{},",
                "\"explicit_column_vec_allocations\":{},\"explicit_block_vec_allocations\":{},",
                "\"explicit_column_vec_requested_bytes\":{},\"explicit_block_vec_requested_bytes\":{},",
                "\"explicit_stream_write_vec_allocations\":{},\"explicit_stream_write_vec_requested_bytes\":{},",
                "\"region_workspace_estimate_peak_bytes\":{},",
                "\"region_stages_are_summed_worker_wall_time\":true,",
                "\"parallel_regions_wall_is_not_additive\":true,",
                "\"workspace_peak_is_largest_individual_region\":true}}"
            ),
            self.regions,
            self.region_worker_wall_ms,
            self.region_unclassified_wall_ms,
            self.sampled_columns,
            self.mixed_chunks,
            self.requested_leaves,
            self.unique_leaves,
            self.column_vec_allocations,
            self.block_vec_allocations,
            self.column_vec_bytes,
            self.block_vec_bytes,
            self.stream_write_vec_allocations,
            self.stream_write_vec_bytes,
            self.region_workspace_estimate_peak_bytes
        )
        .unwrap();
        json
    }
}

pub(super) struct Collector {
    pub metrics: Option<Metrics>,
}
impl Collector {
    pub fn new() -> Self {
        Self::with_enabled(enabled())
    }
    pub fn with_enabled(enabled: bool) -> Self {
        Self {
            metrics: enabled.then(Metrics::default),
        }
    }
    #[inline]
    pub fn start(&self) -> Option<Instant> {
        self.metrics.as_ref().map(|_| Instant::now())
    }
    #[inline]
    pub fn end(&mut self, stage: Stage, started: Option<Instant>) {
        if let (Some(metrics), Some(started)) = (&mut self.metrics, started) {
            metrics.ms[stage as usize] += started.elapsed().as_secs_f64() * 1000.;
        }
    }
    pub fn exclusive_start(&self, children: &[Stage]) -> Option<(Instant, f64)> {
        self.metrics.as_ref().map(|metrics| {
            (
                Instant::now(),
                children.iter().map(|s| metrics.ms[*s as usize]).sum(),
            )
        })
    }
    pub fn exclusive_end(
        &mut self,
        stage: Stage,
        start: Option<(Instant, f64)>,
        children: &[Stage],
    ) {
        if let (Some(metrics), Some((start, before))) = (&mut self.metrics, start) {
            let nested: f64 = children.iter().map(|s| metrics.ms[*s as usize]).sum();
            metrics.ms[stage as usize] +=
                (start.elapsed().as_secs_f64() * 1000. - (nested - before)).max(0.);
        }
    }
    pub fn workspace(&mut self, bytes: usize) {
        if let Some(metrics) = &mut self.metrics {
            metrics.region_workspace_estimate_peak_bytes =
                metrics.region_workspace_estimate_peak_bytes.max(bytes);
        }
    }
    pub fn merge(&mut self, other: Option<&Metrics>) {
        if let (Some(metrics), Some(other)) = (&mut self.metrics, other) {
            metrics.merge(other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_profile_does_not_start_clocks_or_materialize_metrics() {
        let mut profile = Collector::with_enabled(false);
        assert!(profile.start().is_none());
        profile.end(Stage::Initialization, None);
        profile.workspace(1024);
        assert!(profile.metrics.is_none());
    }
    #[test]
    fn merged_profiles_keep_parallel_wall_distinct_from_worker_totals() {
        let mut profile = Collector::with_enabled(true);
        let mut first = Metrics::default();
        first.regions = 1;
        first.ms[Stage::ColumnSampling as usize] = 10.;
        first.region_worker_wall_ms = 12.;
        first.region_workspace_estimate_peak_bytes = 2048;
        profile.merge(Some(&first));
        profile.merge(Some(&first));
        let metrics = profile.metrics.unwrap();
        assert_eq!(metrics.regions, 2);
        assert_eq!(metrics.ms[Stage::ColumnSampling as usize], 20.);
        assert_eq!(metrics.region_worker_wall_ms, 24.);
        assert_eq!(metrics.ms[Stage::ParallelRegionsWall as usize], 0.);
        assert_eq!(metrics.region_workspace_estimate_peak_bytes, 2048);
        assert!(metrics
            .json()
            .contains("\"region_stages_are_summed_worker_wall_time\":true"));
    }
}
