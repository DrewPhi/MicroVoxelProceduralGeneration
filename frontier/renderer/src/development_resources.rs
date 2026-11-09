//! Lazy working-set budgets, not allocations or a strict whole-process cap.
//! GPU residency is charged to the same envelope as CPU memory: integrated GPUs
//! share system RAM. Driver allocations, world state and render targets need slack.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResourceProfile {
    Development,
    #[default]
    Constrained,
    Custom,
}
impl ResourceProfile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Development => "Development",
            Self::Constrained => "Constrained",
            Self::Custom => "Custom",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub total_mib: usize,
    pub available_mib: usize,
    pub measured: bool,
}
impl MemorySnapshot {
    pub fn detect() -> Self {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| Self::from_meminfo(&text))
            .unwrap_or(Self {
                total_mib: 4096,
                available_mib: 2048,
                measured: false,
            })
    }
    fn from_meminfo(text: &str) -> Option<Self> {
        let value = |key| {
            text.lines().find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next()? == key)
                    .then(|| words.next()?.parse::<usize>().ok())
                    .flatten()
            })
        };
        let total_mib = value("MemTotal:")? / 1024;
        let available_mib = value("MemAvailable:")? / 1024;
        (total_mib > 0).then_some(Self {
            total_mib,
            available_mib: available_mib.min(total_mib),
            measured: true,
        })
    }
    /// Leave 30% of currently available RAM (at least 1 GiB) for the OS,
    /// untracked engine/driver use and replacements; never budget over half total RAM.
    pub fn working_mib(self) -> usize {
        self.available_mib
            .saturating_sub((self.available_mib * 30 / 100).max(1024))
            .min(self.total_mib / 2)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceBudgets {
    pub terrain_cpu_mib: usize,
    pub build_mib: usize,
    pub terrain_gpu_mib: usize,
    pub plant_cpu_mib: usize,
    pub plant_gpu_mib: usize,
}
impl ResourceBudgets {
    pub fn constrained(alpine: bool) -> Self {
        Self {
            terrain_cpu_mib: if alpine { 640 } else { 512 },
            build_mib: 512,
            terrain_gpu_mib: 512,
            plant_cpu_mib: 160,
            plant_gpu_mib: 256,
        }
    }
    pub fn development(memory: MemorySnapshot) -> Self {
        // Fractions sum below 100%; integer rounding can only leave more slack.
        let n = memory.working_mib();
        Self {
            terrain_cpu_mib: n * 30 / 100,
            build_mib: n * 30 / 100,
            terrain_gpu_mib: n * 20 / 100,
            plant_cpu_mib: n * 8 / 100,
            plant_gpu_mib: n * 12 / 100,
        }
    }
    pub fn total_mib(self) -> usize {
        self.terrain_cpu_mib
            .saturating_add(self.build_mib)
            .saturating_add(self.terrain_gpu_mib)
            .saturating_add(self.plant_cpu_mib)
            .saturating_add(self.plant_gpu_mib)
    }
}
/// Changes scheduling throughput only; never changes quality, density or fog.
/// A slow-frame streak backs off promptly; recovery needs 30 healthy frames.
#[derive(Clone, Debug)]
pub struct ThroughputGovernor {
    scale: f64,
    slow: u32,
    healthy: u32,
}
impl Default for ThroughputGovernor {
    fn default() -> Self {
        Self {
            scale: 1.,
            slow: 0,
            healthy: 0,
        }
    }
}
impl ThroughputGovernor {
    pub fn observe(&mut self, frame_ms: f64) {
        if !frame_ms.is_finite() || frame_ms <= 0. {
            return;
        }
        if frame_ms > 1000. / 30. {
            self.slow += 1;
            self.healthy = 0;
        } else if frame_ms < (1000. / 30.) * 0.8 {
            self.healthy += 1;
            self.slow = 0;
        } else {
            self.slow = 0;
            self.healthy = 0;
        }
        if self.slow >= 3 {
            self.scale = (self.scale * 0.75).max(0.125);
            self.slow = 0;
        }
        if self.healthy >= 30 {
            self.scale = (self.scale + 0.125).min(1.);
            self.healthy = 0;
        }
    }
    pub fn budget(&self, configured: usize) -> usize {
        if configured == 0 {
            0
        } else {
            ((configured as f64 * self.scale).floor() as usize).clamp(1, configured)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linux_available_is_not_free_and_fallback_is_explicit() {
        let m = MemorySnapshot::from_meminfo(
            "MemTotal: 8388608 kB\nMemFree: 1 kB\nMemAvailable: 6291456 kB\n",
        )
        .unwrap();
        assert_eq!(
            (m.total_mib, m.available_mib, m.working_mib()),
            (8192, 6144, 4096)
        );
        assert!(MemorySnapshot::from_meminfo("MemTotal: 8192 kB").is_none());
    }
    #[test]
    fn combined_budgets_respect_headroom_even_under_pressure() {
        for total in [1024, 4096, 8192, 32768, 131072] {
            for available in [0, total / 8, total / 2, total] {
                let m = MemorySnapshot {
                    total_mib: total,
                    available_mib: available,
                    measured: true,
                };
                assert!(ResourceBudgets::development(m).total_mib() <= m.working_mib());
            }
        }
        assert_eq!(ResourceBudgets::constrained(false).total_mib(), 1952);
        assert_eq!(ResourceBudgets::constrained(true).terrain_cpu_mib, 640);
    }
    #[test]
    fn pacing_has_hysteresis_and_bounds() {
        let mut g = ThroughputGovernor::default();
        for _ in 0..120 {
            g.observe(50.);
        }
        assert_eq!(g.budget(64), 8);
        assert_eq!(g.budget(0), 0);
        for _ in 0..29 {
            g.observe(12.);
        }
        assert_eq!(g.budget(64), 8);
        g.observe(12.);
        assert_eq!(g.budget(64), 16);
        for _ in 0..30 {
            g.observe(20.);
        }
        assert_eq!(g.budget(64), 24);
        for _ in 0..1000 {
            g.observe(10.);
        }
        assert_eq!(g.budget(64), 64);
    }
}
