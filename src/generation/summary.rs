//! Bounded field summaries: sampled error is explicitly not a certified bound.
use super::ProceduralTerrain;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundsKind {
    SampledWithResidual,
}
pub const WATER: u8 = 1;
pub const CLIFF: u8 = 2;
pub const GEOLOGY: u8 = 4;
pub const VOID: u8 = 8;
#[derive(Clone, Copy, Debug)]
pub struct TerrainSummary {
    pub min_m: f64,
    pub max_m: f64,
    pub error_m: f64,
    pub normal_variation: f64,
    pub appearance_variation: f64,
    pub water_coverage: f64,
    pub features: u8,
    pub feature_scale_m: f64,
    pub bounds_kind: BoundsKind,
}
impl ProceduralTerrain {
    pub fn summarize_m(&self, origin: [f64; 2], edge: f64) -> Option<TerrainSummary> {
        if !edge.is_finite()
            || edge <= 0.
            || origin
                .iter()
                .any(|v| !v.is_finite() || *v < 0. || *v >= self.spec.extent_m as f64)
        {
            return None;
        }
        let mut heights = [0.; 9];
        let mut materials = [crate::MaterialId::AIR; 9];
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        let mut wet = 0.;
        for j in 0..3 {
            for i in 0..3 {
                let x = (origin[0] + edge * i as f64 * 0.5).min(self.spec.extent_m as f64 - 0.001);
                let z = (origin[1] + edge * j as f64 * 0.5).min(self.spec.extent_m as f64 - 0.001);
                let s = self.sample_m(x, z)?;
                let k = j * 3 + i;
                heights[k] = s.height_m.max(s.water_m);
                materials[k] = s.material;
                low = low.min(s.height_m);
                high = high.max(heights[k]);
                wet += f64::from(s.water_m > s.height_m);
            }
        }
        let mut error = 0f64;
        for j in 0..3 {
            for i in 0..3 {
                let tx = i as f64 * 0.5;
                let tz = j as f64 * 0.5;
                let h = heights[0] * (1. - tx) * (1. - tz)
                    + heights[2] * tx * (1. - tz)
                    + heights[6] * (1. - tx) * tz
                    + heights[8] * tx * tz;
                error = error.max((heights[j * 3 + i] - h).abs());
            }
        }
        let gx = [
            heights[2] - heights[0],
            heights[5] - heights[3],
            heights[8] - heights[6],
        ];
        let gz = [
            heights[6] - heights[0],
            heights[7] - heights[1],
            heights[8] - heights[2],
        ];
        let variation = (gx[0] - gx[2]).abs().max((gz[0] - gz[2]).abs()) / edge;
        let mut features = 0;
        let mut feature_scale = f64::INFINITY;
        if wet > 0. {
            features |= WATER;
        }
        if (high - low) / edge > 0.55 {
            features |= CLIFF;
        }
        let appearance = materials.iter().filter(|m| **m != materials[4]).count() as f64 / 9.;
        if appearance > 0. {
            features |= GEOLOGY;
        }
        if self.features.overlaps(origin, edge) {
            features |= VOID;
            feature_scale = 1.25 / self.world_scale() as f64;
            error = error.max(5. / self.world_scale() as f64);
            for feature in &self.features.descriptors {
                let [a, b] = feature.bounds();
                if a[0] < origin[0] + edge
                    && b[0] > origin[0]
                    && a[2] < origin[1] + edge
                    && b[2] > origin[1]
                {
                    low = low.min(a[1]);
                    high = high.max(b[1]);
                }
            }
        }
        let scale = self.world_scale() as f64;
        if let Some(width) = self
            .region
            .channel_footprint(origin.map(|v| v * scale), edge * scale)
        {
            let width = width / scale;
            features |= WATER;
            feature_scale = feature_scale.min(width);
            if edge > width {
                error = error.max(width.min(12. / scale));
            }
        }
        // Unsampled small features remain possible. This residual is an error
        // allowance, never a proof that the sampled min/max enclose all terrain.
        let residual = (edge * 0.12).min(7. / scale);
        Some(TerrainSummary {
            min_m: (low - residual).max(0.),
            max_m: high + residual,
            error_m: error.max(residual),
            normal_variation: variation,
            appearance_variation: appearance,
            water_coverage: wet / 9.,
            features,
            feature_scale_m: feature_scale,
            bounds_kind: BoundsKind::SampledWithResidual,
        })
    }
}
