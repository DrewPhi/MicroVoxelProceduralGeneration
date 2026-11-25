//! Measured travel lead and explicit destination presentation gating.
use std::collections::VecDeque;

/// Bounded latency history. Distances derive from velocity and measured latency;
/// this policy imposes no fixed-distance prefetch limit.
pub struct TravelPredictor {
    latencies: VecDeque<f64>,
    previous: Option<([f64; 2], f64)>,
    velocity: [f64; 2],
}
impl Default for TravelPredictor {
    fn default() -> Self {
        Self {
            latencies: VecDeque::new(),
            previous: None,
            velocity: [0.; 2],
        }
    }
}
impl TravelPredictor {
    pub fn observe(&mut self, position: [f64; 2], seconds: f64) {
        if !seconds.is_finite() || position.iter().any(|v| !v.is_finite()) {
            return;
        }
        if let Some((old, time)) = self.previous {
            let dt = seconds - time;
            if dt > 0. {
                self.velocity = std::array::from_fn(|i| (position[i] - old[i]) / dt);
            }
        }
        self.previous = Some((position, seconds));
    }
    pub fn record_latency(&mut self, seconds: f64) {
        if seconds.is_finite() && seconds >= 0. {
            if self.latencies.len() == 32 {
                self.latencies.pop_front();
            }
            self.latencies.push_back(seconds);
        }
    }
    pub fn horizon_seconds(&self) -> f64 {
        let mut sorted: Vec<_> = self.latencies.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        // Bootstrap is replaced with actual generation+join+upload+publish data.
        let p95 = if sorted.is_empty() {
            0.25
        } else {
            sorted[(sorted.len() * 95).div_ceil(100) - 1]
        };
        p95 * 1.25 + 1. / 60.
    }
    pub fn target(&self, position: [f64; 2]) -> [f64; 2] {
        let horizon = self.horizon_seconds();
        std::array::from_fn(|i| position[i] + self.velocity[i] * horizon)
    }
    pub fn reset_motion(&mut self) {
        self.previous = None;
        self.velocity = [0.; 2];
    }
}

/// A teleport is ready only when its exact latest selection has reached GPU
/// publication. CPU completion alone must never move the presented camera.
#[derive(Default)]
pub struct TeleportGate {
    serial: u64,
    pending: Option<(u64, [f64; 2])>,
    ready: Option<u64>,
}
impl TeleportGate {
    pub fn request(&mut self, destination: [f64; 2]) -> u64 {
        self.serial += 1;
        self.pending = Some((self.serial, destination));
        self.ready = None;
        self.serial
    }
    pub fn destination(&self) -> Option<[f64; 2]> {
        self.pending.map(|(_, p)| p)
    }
    pub fn published(&mut self, destination: [f64; 2]) {
        if let Some((id, wanted)) = self.pending {
            if destination == wanted {
                self.ready = Some(id);
            }
        }
    }
    pub fn is_ready(&self, id: u64) -> bool {
        self.ready == Some(id)
    }
    pub fn finish(&mut self, id: u64) -> bool {
        if !self.is_ready(id) {
            return false;
        }
        self.pending = None;
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lead_tracks_latency_speed_stops_and_turns_without_distance_cap() {
        let mut p = TravelPredictor::default();
        p.record_latency(2.);
        p.observe([0., 0.], 0.);
        p.observe([15., 0.], 1.);
        assert!(p.target([15., 0.])[0] > 50.);
        p.observe([15., 0.], 2.);
        assert_eq!(p.target([15., 0.]), [15., 0.]);
        p.observe([0., 0.], 3.);
        assert!(p.target([0., 0.])[0] < -30.);
        for _ in 0..40 {
            p.record_latency(0.1);
        }
        assert!(p.horizon_seconds() < 0.2);
        assert_eq!(p.latencies.len(), 32);
    }
    #[test]
    fn changed_destination_and_cold_cpu_ready_cannot_release_camera() {
        let mut t = TeleportGate::default();
        let old = t.request([800., 0.]);
        assert!(!t.is_ready(old));
        let latest = t.request([-800., 0.]);
        t.published([800., 0.]);
        assert!(!t.is_ready(old));
        assert!(!t.is_ready(latest));
        t.published([-800., 0.]);
        assert!(t.finish(latest));
        let warm = t.request([-800., 0.]);
        assert!(!t.is_ready(warm));
        t.published([-800., 0.]);
        assert!(t.is_ready(warm));
    }
}
