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
    pub terrain_owner: u64,
    pub watched_tile_cells: Option<u32>,
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
            "{{\"seconds\":{},\"frame_ms\":{},\"prepare_ms\":{},\"terrain_ms\":{},\"vegetation_ms\":{},\"yaw\":{},\"eye\":{:?},\"physical_eye\":{},\"terrain_pending\":{},\"terrain_ready\":{},\"native_clusters\":{},\"terrain_owner\":{},\"watched_tile_cells\":{},\"plant_pending\":{},\"plant_triangles\":{},\"plant_tree_lods\":{:?},\"plant_lod_changes\":{},\"plant_coarsenings\":{},\"plant_demand\":{:?},\"plant_ready\":{:?},\"shadow_admitted\":{},\"shadow_removed\":{},\"shadow_triangles\":{},\"shadow_lod_changes\":{},\"shadow_lods\":{:?},\"plant_selected_mesh_evictions\":{},\"plant_cpu_bytes\":{}}}",
            x.seconds,x.frame_ms,x.prepare_ms,x.terrain_ms,x.vegetation_ms,x.yaw,x.eye,x.physical_eye.map_or("null".into(),|p|format!("{p:?}")),
            x.terrain_pending,x.terrain_ready,x.native_clusters,x.terrain_owner,x.watched_tile_cells.map_or("null".into(),|v|v.to_string()),x.plant_pending,x.plant_triangles,x.plant_tree_lods,x.plant_lod_changes,x.plant_coarsenings,x.plant_demand,x.plant_ready,x.shadow_admitted,x.shadow_removed,x.shadow_triangles,x.shadow_lod_changes,x.shadow_lods,x.plant_selected_mesh_evictions,x.plant_cpu_bytes)).collect::<Vec<_>>().join(",");
        format!("{{\"capacity\":3600,\"scope\":\"last presented frames, including startup/warmup; prepare_ms is main-thread wall time before surface acquisition; GPU timestamps are separate asynchronous samples\",\"frames\":[{rows}]}}")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ShadowState {
    selected: Option<crate::vegetation::MeshKey>,
    requested: crate::vegetation::MeshKey,
    reason: crate::vegetation::ShadowReason,
}
struct ShadowEvent {
    seconds: f64,
    root: [i64; 3],
    from: Option<u8>,
    to: Option<u8>,
    reason: &'static str,
    gpu: bool,
}
/// Last 4096 actual stream/admission changes, rather than a log of every frame.
#[derive(Default)]
pub(crate) struct ShadowTrace {
    previous: std::collections::BTreeMap<[i64; 3], ShadowState>,
    gpu: std::collections::BTreeSet<[i64; 3]>,
    events: VecDeque<ShadowEvent>,
    dropped: u64,
}
impl ShadowTrace {
    fn push(&mut self, e: ShadowEvent) {
        if self.events.len() == 4096 {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(e);
    }
    pub fn observe(
        &mut self,
        seconds: f64,
        inspections: &[crate::vegetation::ShadowInspection],
        gpu: &[[i64; 3]],
    ) {
        let next: std::collections::BTreeMap<_, _> = inspections
            .iter()
            .map(|x| {
                (
                    x.root_cell,
                    ShadowState {
                        selected: x.selected,
                        requested: x.requested,
                        reason: x.reason,
                    },
                )
            })
            .collect();
        for (root, state) in &next {
            let old = self.previous.get(root).copied();
            if old != Some(*state) {
                self.push(ShadowEvent {
                    seconds,
                    root: *root,
                    from: old.and_then(|s| s.selected.map(|k| k.lod)),
                    to: state.selected.map(|k| k.lod),
                    reason: state.reason.as_str(),
                    gpu: false,
                });
            }
        }
        let removed: Vec<_> = self
            .previous
            .iter()
            .filter(|(root, _)| !next.contains_key(*root))
            .map(|(r, s)| (*r, s.selected.map(|k| k.lod)))
            .collect();
        for (root, from) in removed {
            self.push(ShadowEvent {
                seconds,
                root,
                from,
                to: None,
                reason: "left_stream_demand",
                gpu: false,
            });
        }
        self.previous = next;
        let next: std::collections::BTreeSet<_> = gpu.iter().copied().collect();
        let entered: Vec<_> = next.difference(&self.gpu).copied().collect();
        let removed: Vec<_> = self.gpu.difference(&next).copied().collect();
        for root in entered {
            self.push(ShadowEvent {
                seconds,
                root,
                from: None,
                to: None,
                reason: "gpu_admitted",
                gpu: true,
            });
        }
        for root in removed {
            self.push(ShadowEvent {
                seconds,
                root,
                from: None,
                to: None,
                reason: "gpu_removed",
                gpu: true,
            });
        }
        self.gpu = next;
    }
    pub fn json(&self) -> String {
        let lod = |x: Option<u8>| x.map_or("null".into(), |x| x.to_string());
        let events=self.events.iter().map(|e|format!("{{\"seconds\":{},\"root\":{:?},\"from_lod\":{},\"to_lod\":{},\"reason\":\"{}\",\"gpu_admission\":{}}}",e.seconds,e.root,lod(e.from),lod(e.to),e.reason,e.gpu)).collect::<Vec<_>>().join(",");
        format!("{{\"capacity\":4096,\"dropped\":{},\"scope\":\"monotonic seconds since viewer visual start; stream reason/LOD changes and actual GPU caster membership; does not infer shadow texel visibility\",\"events\":[{events}]}}",self.dropped)
    }
}

#[cfg(test)]
mod shadow_trace_tests {
    use super::*;
    fn inspection(lod: u8) -> crate::vegetation::ShadowInspection {
        let key = crate::vegetation::MeshKey {
            species: 2,
            variant: 0,
            seed: 7,
            tile: [0, 0],
            lod,
            edit_id: 0,
        };
        crate::vegetation::ShadowInspection {
            root_cell: [1, 2, 3],
            selected: Some(key),
            requested: key,
            reason: crate::vegetation::ShadowReason::Admitted,
            triangles: 100,
        }
    }
    #[test]
    fn records_changes_and_actual_admission_without_per_frame_duplicates() {
        let mut trace = ShadowTrace::default();
        let root = [1, 2, 3];
        trace.observe(0., &[inspection(2)], &[root]);
        assert_eq!(trace.events.len(), 2);
        trace.observe(1., &[inspection(2)], &[root]);
        assert_eq!(trace.events.len(), 2);
        trace.observe(2., &[inspection(4)], &[root]);
        assert_eq!(trace.events.len(), 3);
        trace.observe(3., &[], &[]);
        assert_eq!(trace.events.len(), 5);
        assert!(trace.json().contains("gpu_removed"));
    }
    #[test]
    fn event_ring_is_bounded() {
        let mut trace = ShadowTrace::default();
        for i in 0..5000 {
            trace.observe(i as f64, &[inspection(if i % 2 == 0 { 2 } else { 4 })], &[]);
        }
        assert_eq!(trace.events.len(), 4096);
        assert_eq!(trace.dropped, 904);
    }
}
