//! Sparse physical geological voids. Most columns never visit a descriptor.
use super::{
    noise::mix,
    terrain::{self, Region},
    Geology,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Cave,
    Arch,
}
#[derive(Clone, Copy, Debug)]
pub struct GeologicalFeature {
    pub kind: FeatureKind,
    pub center_m: [f64; 3],
    pub axis: [f64; 2],
    pub radius_m: f64,
    pub length_m: f64,
}
impl GeologicalFeature {
    pub fn bounds(self) -> [[f64; 3]; 2] {
        let pad = if self.kind == FeatureKind::Arch {
            self.radius_m * 2.
        } else {
            self.radius_m
        };
        let extent = [
            self.length_m * self.axis[0].abs() + pad,
            if self.kind == FeatureKind::Arch {
                self.radius_m * 1.8
            } else {
                self.radius_m
            },
            self.length_m * self.axis[1].abs() + pad,
        ];
        [
            std::array::from_fn(|i| self.center_m[i] - extent[i]),
            std::array::from_fn(|i| self.center_m[i] + extent[i]),
        ]
    }
    pub fn void_interval(self, x: f64, z: f64) -> Option<[f64; 2]> {
        let dx = x - self.center_m[0];
        let dz = z - self.center_m[2];
        let along = dx * self.axis[0] + dz * self.axis[1];
        let cross = -dx * self.axis[1] + dz * self.axis[0];
        let q = if self.kind == FeatureKind::Arch {
            (along / (self.length_m * 0.7)).powi(2) + (cross / (self.radius_m * 2.)).powi(2)
        } else {
            (along / self.length_m).powi(2) + (cross / self.radius_m).powi(2)
        };
        if q >= 1. {
            return None;
        }
        let half = self.radius_m * (1. - q).sqrt();
        Some([self.center_m[1] - half, self.center_m[1] + half])
    }
    pub fn rock_top(self, x: f64, z: f64) -> Option<f64> {
        if self.kind != FeatureKind::Arch {
            return None;
        }
        let dx = x - self.center_m[0];
        let dz = z - self.center_m[2];
        let along = dx * self.axis[0] + dz * self.axis[1];
        let cross = -dx * self.axis[1] + dz * self.axis[0];
        let q = (along / self.length_m).powi(2) + (cross / (self.radius_m * 0.7)).powi(2);
        (q < 1.).then(|| self.center_m[1] + self.radius_m * 1.8 * (1. - q).sqrt())
    }
}
#[derive(Default)]
pub(super) struct FeatureIndex {
    pub descriptors: Vec<GeologicalFeature>,
    bins: HashMap<(i32, i32), Vec<usize>>,
}
impl FeatureIndex {
    pub fn generate(region: &Region, seed: u64) -> Self {
        let mut index = Self::default();
        for z in 0..39 {
            for x in 0..39 {
                let h = mix(seed ^ mix(x as u64) ^ mix(z as u64).rotate_left(29) ^ 1719);
                if h % 7 != 0 {
                    continue;
                }
                let px = 128. + x as f64 * 256. + (h % 81) as f64;
                let pz = 128. + z as f64 * 256. + ((h >> 16) % 81) as f64;
                let s = terrain::terrain_surface(region, px, pz, 0.001, seed);
                let g = region.gradient_at(px, pz);
                let length = g[0].hypot(g[1]);
                if length < 0.2 || s.water > s.height || s.environment.soil_depth_m > 2. {
                    continue;
                }
                let kind = match s.environment.geology {
                    Geology::Limestone => FeatureKind::Cave,
                    Geology::Sandstone if length > 0.4 => FeatureKind::Arch,
                    _ => continue,
                };
                let feature = GeologicalFeature {
                    kind,
                    center_m: [
                        px,
                        s.height + if kind == FeatureKind::Arch { 1. } else { -3. },
                        pz,
                    ],
                    axis: if kind == FeatureKind::Arch {
                        [-g[1] / length, g[0] / length]
                    } else {
                        [g[0] / length, g[1] / length]
                    },
                    radius_m: 2.5 + (h % 4) as f64 * 0.4,
                    length_m: 12.,
                };
                let id = index.descriptors.len();
                let [low, high] = feature.bounds();
                for bz in (low[2] / 64.).floor() as i32..=(high[2] / 64.).floor() as i32 {
                    for bx in (low[0] / 64.).floor() as i32..=(high[0] / 64.).floor() as i32 {
                        index.bins.entry((bx, bz)).or_default().push(id);
                    }
                }
                index.descriptors.push(feature);
            }
        }
        index
    }
    /// Rescale sparse descriptors once, then rebuild their fixed game-meter bins.
    pub fn rescaled(&self, factor: f64) -> Self {
        let mut result = Self::default();
        for feature in &self.descriptors {
            let f = GeologicalFeature {
                center_m: feature.center_m.map(|v| v * factor),
                radius_m: feature.radius_m * factor,
                length_m: feature.length_m * factor,
                ..*feature
            };
            let id = result.descriptors.len();
            let [low, high] = f.bounds();
            for z in (low[2] / 64.).floor() as i32..=(high[2] / 64.).floor() as i32 {
                for x in (low[0] / 64.).floor() as i32..=(high[0] / 64.).floor() as i32 {
                    result.bins.entry((x, z)).or_default().push(id);
                }
            }
            result.descriptors.push(f);
        }
        result
    }
    pub fn intervals(&self, x: f64, z: f64) -> Vec<[f64; 2]> {
        self.bins
            .get(&((x / 64.).floor() as i32, (z / 64.).floor() as i32))
            .into_iter()
            .flatten()
            .filter_map(|&i| self.descriptors[i].void_interval(x, z))
            .collect()
    }
    pub fn overlaps(&self, origin: [f64; 2], edge: f64) -> bool {
        self.descriptors.iter().any(|f| {
            let [a, b] = f.bounds();
            a[0] < origin[0] + edge
                && b[0] > origin[0]
                && a[2] < origin[1] + edge
                && b[2] > origin[1]
        })
    }
    pub fn rock_top(&self, x: f64, z: f64) -> Option<f64> {
        self.bins
            .get(&((x / 64.).floor() as i32, (z / 64.).floor() as i32))
            .into_iter()
            .flatten()
            .filter_map(|&i| self.descriptors[i].rock_top(x, z))
            .max_by(f64::total_cmp)
    }
}

pub(crate) fn add_solids(runs: Vec<super::Run>, solids: &[super::Run]) -> Vec<super::Run> {
    let mut result = runs;
    for solid in solids {
        let mut next = Vec::with_capacity(result.len() + 2);
        for r in result {
            if r.end <= solid.start
                || r.start >= solid.end
                || r.material == crate::MaterialId::WATER
            {
                super::push_run(&mut next, r);
                continue;
            }
            super::push_run(
                &mut next,
                super::Run {
                    end: r.end.min(solid.start),
                    ..r
                },
            );
            super::push_run(
                &mut next,
                super::Run {
                    start: r.start.max(solid.start),
                    end: r.end.min(solid.end),
                    material: solid.material,
                },
            );
            super::push_run(
                &mut next,
                super::Run {
                    start: r.start.max(solid.end),
                    ..r
                },
            );
        }
        result = next;
    }
    result
}

/// Subtract an air interval from solid runs while preserving material boundaries.
pub(crate) fn carve(runs: Vec<super::Run>, voids: &[[i64; 2]]) -> Vec<super::Run> {
    let mut result = runs;
    for &[low, high] in voids {
        let mut next = Vec::with_capacity(result.len() + 2);
        for r in result {
            if r.material == crate::MaterialId::AIR
                || r.material == crate::MaterialId::WATER
                || r.end <= low
                || r.start >= high
            {
                super::push_run(&mut next, r);
                continue;
            }
            super::push_run(
                &mut next,
                super::Run {
                    end: r.end.min(low),
                    ..r
                },
            );
            super::push_run(
                &mut next,
                super::Run {
                    start: r.start.max(low),
                    end: r.end.min(high),
                    material: crate::MaterialId::AIR,
                },
            );
            super::push_run(
                &mut next,
                super::Run {
                    start: r.start.max(high),
                    ..r
                },
            );
        }
        result = next;
    }
    result
}
