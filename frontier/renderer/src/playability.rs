//! Bounded frame observations: no file I/O, GPU waits or strings in the hot path.
use std::collections::VecDeque;
/// Report-time only: never read procfs in a presented-frame hot path.
pub(crate) fn process_memory_json() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let bytes = |name: &str| {
        status
            .lines()
            .find_map(|line| {
                line.strip_prefix(name)?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
                    .and_then(|v| v.checked_mul(1024))
            })
            .map_or("null".to_string(), |v| v.to_string())
    };
    format!("{{\"rss_bytes\":{},\"peak_rss_bytes\":{},\"scope\":\"Linux whole-process RSS and high-water RSS including startup and active workers; not GPU allocation accounting or a hard memory bound\"}}", bytes("VmRSS:"), bytes("VmHWM:"))
}
#[derive(Clone, Copy)]
pub(crate) struct Observation {
    pub seconds: f64,
    pub frame_ms: f64,
    pub prepare_ms: f64,
    pub terrain_ms: f64,
    pub vegetation_ms: f64,
    pub yaw: f32,
    pub eye: [f64; 3],
    pub physical_eye: Option<[f64; 3]>,
    pub terrain_pending: usize,
    pub terrain_ready: bool,
    pub native_clusters: usize,
    pub plant_pending: usize,
    pub plant_triangles: usize,
    pub plant_tree_lods: [usize; 4],
    pub plant_lod_changes: usize,
    pub plant_coarsenings: usize,
    pub plant_demand: [usize; 3],
    pub plant_ready: [usize; 3],
    pub shadow_admitted: usize,
    pub shadow_removed: usize,
    pub shadow_triangles: usize,
    pub shadow_lod_changes: usize,
    pub shadow_lods: [usize; 4],
    pub plant_selected_mesh_evictions: usize,
    pub plant_cpu_bytes: usize,
}
pub(crate) struct FrameTrace(VecDeque<Observation>);
impl Default for FrameTrace {
    fn default() -> Self {
        Self(VecDeque::with_capacity(3600))
    }
}
impl FrameTrace {
    pub fn record(&mut self, x: Observation) {
        if self.0.len() == 3600 {
            self.0.pop_front();
        }
        self.0.push_back(x);
    }
    pub fn json(&self) -> String {
        let rows = self.0.iter().map(|x| format!(
            "{{\"seconds\":{},\"frame_ms\":{},\"prepare_ms\":{},\"terrain_ms\":{},\"vegetation_ms\":{},\"yaw\":{},\"eye\":{:?},\"physical_eye\":{},\"terrain_pending\":{},\"terrain_ready\":{},\"native_clusters\":{},\"plant_pending\":{},\"plant_triangles\":{},\"plant_tree_lods\":{:?},\"plant_lod_changes\":{},\"plant_coarsenings\":{},\"plant_demand\":{:?},\"plant_ready\":{:?},\"shadow_admitted\":{},\"shadow_removed\":{},\"shadow_triangles\":{},\"shadow_lod_changes\":{},\"shadow_lods\":{:?},\"plant_selected_mesh_evictions\":{},\"plant_cpu_bytes\":{}}}",
            x.seconds,x.frame_ms,x.prepare_ms,x.terrain_ms,x.vegetation_ms,x.yaw,x.eye,x.physical_eye.map_or("null".into(),|p|format!("{p:?}")),
            x.terrain_pending,x.terrain_ready,x.native_clusters,x.plant_pending,x.plant_triangles,x.plant_tree_lods,x.plant_lod_changes,x.plant_coarsenings,x.plant_demand,x.plant_ready,x.shadow_admitted,x.shadow_removed,x.shadow_triangles,x.shadow_lod_changes,x.shadow_lods,x.plant_selected_mesh_evictions,x.plant_cpu_bytes)).collect::<Vec<_>>().join(",");
        format!("{{\"capacity\":3600,\"scope\":\"last presented frames, including startup/warmup; prepare_ms is main-thread wall time before surface acquisition; GPU timestamps are separate asynchronous samples\",\"frames\":[{rows}]}}")
    }
}
