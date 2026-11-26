//! Local adjustments to the automatic screen-space LOD policy.

pub(crate) const DISTANCES: [f32; 27] = [
    0., 4., 8., 12., 16., 24., 32., 48., 64., 80., 100., 125., 150., 175., 200., 225., 250., 300.,
    350., 400., 500., 600., 750., 900., 1050., 1200., 1536.,
];
pub(crate) const LAST_KNOT: f32 = (DISTANCES.len() - 1) as f32;
pub(crate) const TERRAIN_DISTANCES: [f32; DISTANCES.len()] = [
    0., 4., 8., 12., 16., 24., 32., 48., 64., 80., 100., 125., 150., 175., 200., 250., 300., 400.,
    500., 650., 800., 1000., 1250., 1500., 1900., 2400., 3072.,
];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LodCurve {
    knots: [f32; DISTANCES.len()],
}

impl Default for LodCurve {
    fn default() -> Self {
        Self {
            knots: [0.; DISTANCES.len()],
        }
    }
}

fn smooth(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}

pub(crate) fn distance_at(knot: f32, distances: &[f32; DISTANCES.len()]) -> f32 {
    let left = (knot as usize).min(DISTANCES.len() - 2);
    let t = knot - left as f32;
    distances[left] + (distances[left + 1] - distances[left]) * t
}

impl LodCurve {
    pub(crate) fn sample(&self, knot: f32) -> f32 {
        let left = (knot as usize).min(self.knots.len() - 2);
        let t = smooth(knot - left as f32);
        self.knots[left] + (self.knots[left + 1] - self.knots[left]) * t
    }

    pub(crate) fn at_distance(&self, distance: f64) -> f32 {
        self.in_profile(distance as f32, &DISTANCES)
    }

    pub(crate) fn in_profile(&self, distance: f32, distances: &[f32; DISTANCES.len()]) -> f32 {
        let distance = distance.clamp(0., distances[distances.len() - 1]);
        let right = distances
            .partition_point(|&point| point <= distance)
            .clamp(1, DISTANCES.len() - 1);
        let left = right - 1;
        let t = smooth((distance - distances[left]) / (distances[right] - distances[left]));
        self.knots[left] + (self.knots[right] - self.knots[left]) * t
    }

    /// Keep the clicked segment at the pointer height and ease back into the
    /// previous profile over neighboring segments.
    pub(crate) fn bend(&mut self, knot: f32, value: f32) {
        let first = knot.floor();
        let last = knot.ceil();
        for (i, old) in self.knots.iter_mut().enumerate() {
            let i = i as f32;
            let offset = (first - i).max(i - last).max(0.);
            let weight = 1. - smooth((offset / 3.).min(1.));
            *old += (value - *old) * weight;
        }
    }
}

pub(crate) fn adjusted_stride(automatic: u8, pixels_per_voxel: f64, coarsest: u8, bias: f32) -> u8 {
    if bias == 0. {
        return automatic;
    }
    let endpoint = if bias > 0. {
        // Use the finest tier that preserves projected detail to roughly one
        // pixel. A tree's stride-16 proxy is not individual branch geometry.
        let mut screen_stride = 1;
        while screen_stride < 8 && pixels_per_voxel * f64::from(screen_stride * 2) <= 1. {
            screen_stride *= 2;
        }
        automatic.min(screen_stride)
    } else {
        coarsest
    };
    let base = automatic.ilog2() as f32;
    let target = endpoint.ilog2() as f32;
    1 << (base + (target - base) * bias.abs()).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_drag_reaches_the_pointer_and_falls_off_locally() {
        let mut curve = LodCurve::default();
        let knot = 14.35;
        curve.bend(knot, 1.);
        assert_eq!(curve.sample(knot), 1.);
        assert_eq!(
            curve.at_distance(f64::from(distance_at(knot, &DISTANCES))),
            1.
        );
        assert_eq!(
            curve.in_profile(distance_at(knot, &TERRAIN_DISTANCES), &TERRAIN_DISTANCES),
            1.
        );
        assert!(curve.at_distance(175.) > 0. && curve.at_distance(175.) < 1.);
        assert_eq!(curve.at_distance(350.), 0.);
        assert_eq!(curve.at_distance(64.), 0.);
        curve.bend(knot, -1.);
        assert_eq!(curve.sample(knot), -1.);
    }

    #[test]
    fn neutral_preserves_policy_and_upper_limit_tracks_projected_pixel_size() {
        assert_eq!(adjusted_stride(16, 0.30, 16, 0.), 16);
        assert_eq!(adjusted_stride(16, 0.30, 16, 1.), 2);
        assert_eq!(adjusted_stride(16, 0.13, 16, 1.), 4);
        assert_eq!(adjusted_stride(16, 0.05, 16, 1.), 8);
        assert_eq!(adjusted_stride(1, 0.05, 16, 1.), 1);
        assert_eq!(adjusted_stride(2, 0.30, 16, -1.), 16);
        assert_eq!(adjusted_stride(2, 0.30, 8, -1.), 8);
    }
}
