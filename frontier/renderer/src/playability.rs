//! Bounded frame observations: no file I/O, GPU waits or strings in the hot path.
use std::collections::VecDeque;
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
            "{{\"seconds\":{},\"frame_ms\":{},\"prepare_ms\":{},\"terrain_ms\":{},\"vegetation_ms\":{},\"yaw\":{},\"eye\":{:?},\"physical_eye\":{},\"terrain_pending\":{},\"terrain_ready\":{},\"native_clusters\":{},\"plant_pending\":{},\"plant_triangles\":{}}}",
            x.seconds,x.frame_ms,x.prepare_ms,x.terrain_ms,x.vegetation_ms,x.yaw,x.eye,x.physical_eye.map_or("null".into(),|p|format!("{p:?}")),
            x.terrain_pending,x.terrain_ready,x.native_clusters,x.plant_pending,x.plant_triangles)).collect::<Vec<_>>().join(",");
        format!("{{\"capacity\":3600,\"scope\":\"last presented frames, including startup/warmup; prepare_ms is main-thread wall time before surface acquisition; GPU timestamps are separate asynchronous samples\",\"frames\":[{rows}]}}")
    }
}
