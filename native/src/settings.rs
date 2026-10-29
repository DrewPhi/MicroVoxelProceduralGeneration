//! Small, versioned local display preferences. No world/save data lives here.
use std::{collections::VecDeque, path::PathBuf};
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub ray: bool,
    pub quality: f64,
    pub native_radius: f64,
    pub memory_mib: usize,
    pub haze: f64,
    pub ambient: bool,
    pub shadows: bool,
    pub antialias: bool,
    pub transitions: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            ray: false,
            quality: 75.,
            native_radius: 6.,
            memory_mib: 128,
            haze: 0.7,
            ambient: true,
            shadows: true,
            antialias: true,
            transitions: true,
        }
    }
}
impl Settings {
    /// Maximum quality targets one projected display cell per pixel. This is
    /// a sampling target, not a claim that all silhouettes have <=1 px error.
    pub fn pixels(self) -> f64 {
        1. + 15. * (1. - self.quality / 100.).powi(2)
    }
    pub fn tile_budget(self) -> usize {
        (self.memory_mib * 8).clamp(256, 8192)
    }
    pub fn validate(&mut self) {
        let finite = |v: f64, default: f64, lo: f64, hi: f64| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                default
            }
        };
        self.quality = finite(self.quality, 75., 0., 100.);
        self.native_radius = finite(self.native_radius, 6., 2., 32.);
        self.haze = finite(self.haze, 0.7, 0., 2.);
        self.memory_mib = self.memory_mib.clamp(64, 1024);
    }
    pub fn preset(index: usize) -> Self {
        match index {
            0 => Self {
                quality: 45.,
                native_radius: 4.,
                memory_mib: 96,
                shadows: false,
                ..Self::default()
            },
            2 => Self {
                quality: 100.,
                native_radius: 12.,
                memory_mib: 256,
                ..Self::default()
            },
            _ => Self::default(),
        }
    }
    pub fn path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("settings.conf")
    }
    pub fn load() -> Self {
        let mut s = Self::default();
        if let Ok(text) = std::fs::read_to_string(Self::path()) {
            for line in text.lines() {
                let Some((key, v)) = line.split_once('=') else {
                    continue;
                };
                match key {
                    "renderer" => s.ray = v == "ray",
                    "quality" => s.quality = v.parse().unwrap_or(s.quality),
                    "native_radius" => s.native_radius = v.parse().unwrap_or(s.native_radius),
                    "memory_mib" => s.memory_mib = v.parse().unwrap_or(s.memory_mib),
                    "haze" => s.haze = v.parse().unwrap_or(s.haze),
                    "ambient" => s.ambient = v == "true",
                    "shadows" => s.shadows = v == "true",
                    "antialias" => s.antialias = v == "true",
                    "transitions" => s.transitions = v == "true",
                    _ => {}
                }
            }
        }
        s.validate();
        s
    }
    pub fn save(self) -> std::io::Result<()> {
        let path = Self::path();
        let temporary = path.with_extension("conf.tmp");
        std::fs::write(&temporary,format!("# Display settings v1; world data is independent\nrenderer={}\nquality={}\nnative_radius={}\nmemory_mib={}\nhaze={}\nambient={}\nshadows={}\nantialias={}\ntransitions={}\n",
            if self.ray {"ray"}else{"mesh"},self.quality,self.native_radius,self.memory_mib,self.haze,self.ambient,self.shadows,self.antialias,self.transitions))?;
        std::fs::rename(temporary, path)
    }
}
#[derive(Default)]
pub struct FrameTimes {
    samples: VecDeque<f64>,
}
impl FrameTimes {
    pub fn push(&mut self, seconds: f64) {
        if seconds.is_finite() && seconds > 0. {
            if self.samples.len() == 512 {
                self.samples.pop_front();
            }
            self.samples.push_back(seconds * 1000.);
        }
    }
    pub fn percentiles(&self) -> [f64; 3] {
        if self.samples.is_empty() {
            return [0.; 3];
        }
        let mut sorted: Vec<_> = self.samples.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        [0.5, 0.95, 0.99].map(|p| sorted[((sorted.len() - 1) as f64 * p).round() as usize])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_reject_nonfinite_and_quality_is_monotonic() {
        let mut s = Settings {
            quality: f64::NAN,
            native_radius: f64::INFINITY,
            memory_mib: usize::MAX,
            haze: -2.,
            ..Settings::default()
        };
        s.validate();
        assert_eq!(s.quality, 75.);
        assert_eq!(s.memory_mib, 1024);
        assert_eq!(s.haze, 0.);
        let mut previous = 17.;
        for quality in 0..=100 {
            s.quality = quality as f64;
            assert!(s.pixels() <= previous);
            previous = s.pixels();
        }
        assert_eq!(s.pixels(), 1.);
    }
    #[test]
    fn frame_percentiles_include_stalls() {
        let mut f = FrameTimes::default();
        for _ in 0..90 {
            f.push(0.01);
        }
        for _ in 0..10 {
            f.push(0.04);
        }
        assert_eq!(f.percentiles(), [10., 40., 40.]);
        for _ in 0..10 {
            f.push(2.);
        }
        f.push(f64::NAN);
        f.push(f64::INFINITY);
        assert_eq!(f.percentiles(), [10., 2000., 2000.]);
    }
}
