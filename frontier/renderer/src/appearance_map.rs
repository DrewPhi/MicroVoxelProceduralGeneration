//! Bounded detailed appearance maps for an unchanged coarse geometry patch.
//!
//! Texels contain canonical native-face radiance/area profiles, not invented
//! texture or subvoxel noise. A quarter-metre target spacing is independent of
//! geometry LOD until the bounded map size forces a larger power-of-two spacing.
//! Unlike the appearance hierarchy these are detailed point samples, not mip
//! restrictions. Bilinear reconstruction alone is not a footprint AA guarantee.

use crate::appearance_hierarchy::AppearanceColumn;
use std::collections::HashMap;

pub(crate) type Profile = [[f32; 4]; 5];
const TARGET_SPACING_CELLS: i64 = 4;
/// Number of intervals per patch axis. The memory-bounded default produces a
/// 3x3 map on aligned coarse patches; opt in to denser 5x5 or 9x9 maps.
/// Only exact values 2, 4 and 8 are accepted; invalid settings use the default.
pub(crate) fn map_intervals() -> usize {
    static INTERVALS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *INTERVALS.get_or_init(|| {
        parse_intervals(
            std::env::var("FRONTIER_APPEARANCE_MAP_INTERVALS")
                .ok()
                .as_deref(),
        )
    })
}

fn parse_intervals(value: Option<&str>) -> usize {
    match value {
        Some("4") => 4,
        Some("8") => 8,
        _ => 2,
    }
}

#[derive(Debug)]
pub(crate) struct AppearanceMap {
    /// Global native-cell coordinate of the first texel (including the margin).
    pub origin_cells: [i64; 2],
    pub spacing_cells: i64,
    pub dimension: usize,
    /// Row-major (+X fastest), standard +Y,+X,-X,+Z,-Z profiles.
    pub profiles: Vec<Profile>,
}

/// Cover a square patch with a globally aligned bounded map. Neighbor probes
/// always use one native cell, regardless of geometry size or texel spacing.
/// Each node costs at most five canonical queries, with overlap reused locally;
/// the caller may additionally reuse its immutable world/revision source cache.
pub(crate) fn build_map(
    origin: [i64; 2],
    edge: i64,
    sample: impl FnMut(i64, i64) -> Result<AppearanceColumn, String>,
) -> Result<AppearanceMap, String> {
    build_map_with_intervals(origin, edge, map_intervals(), sample)
}

/// Explicit-density variant for bounded experiments and deterministic tests.
/// Aligned patches have at most intervals+1 nodes per axis; an unaligned patch
/// can need one extra node to preserve global alignment and full coverage.
pub(crate) fn build_map_with_intervals(
    origin: [i64; 2],
    edge: i64,
    intervals: usize,
    mut sample: impl FnMut(i64, i64) -> Result<AppearanceColumn, String>,
) -> Result<AppearanceMap, String> {
    if !matches!(intervals, 2 | 4 | 8) {
        return Err("appearance map intervals must be 2, 4 or 8".into());
    }
    if edge <= 0 {
        return Err("appearance map requires a positive native-cell edge".into());
    }
    let mut spacing = TARGET_SPACING_CELLS;
    while i128::from(spacing) * (intervals as i128) < i128::from(edge) {
        spacing = spacing
            .checked_mul(2)
            .ok_or("appearance map spacing overflow")?;
    }
    let mut map_origin = [0; 2];
    let mut dimension = 0;
    for axis in 0..2 {
        // i128 intermediates also make negative floor division explicit.
        let start = i128::from(origin[axis]).div_euclid(i128::from(spacing)) * i128::from(spacing);
        map_origin[axis] = i64::try_from(start).map_err(|_| "appearance map origin overflow")?;
        let end = i128::from(origin[axis]) + i128::from(edge);
        let intervals = (end - start + i128::from(spacing) - 1) / i128::from(spacing);
        dimension = dimension.max(intervals as usize + 1);
    }
    if dimension > intervals + 2 {
        return Err("appearance map exceeds bounded dimensions".into());
    }
    // Cache only the native source payload, keeping this independent of the
    // hierarchy's representation or any mip/geometry-dependent cache key.
    let mut columns = HashMap::<[i64; 2], (i64, [[f32; 3]; 3])>::new();
    let mut column = |point: [i64; 2]| -> Result<(i64, [[f32; 3]; 3]), String> {
        if let Some(value) = columns.get(&point) {
            return Ok(*value);
        }
        let value = sample(point[0], point[1])?;
        if value
            .colors
            .iter()
            .flatten()
            .any(|c| !c.is_finite() || *c < 0.)
        {
            return Err("appearance map requires finite nonnegative canonical colors".into());
        }
        let value = (value.height, value.colors);
        columns.insert(point, value);
        Ok(value)
    };
    let mut profiles = Vec::with_capacity(dimension * dimension);
    for z in 0..dimension {
        for x in 0..dimension {
            let coordinate = |axis: usize, index: usize| {
                i64::try_from(i128::from(map_origin[axis]) + index as i128 * i128::from(spacing))
                    .map_err(|_| "appearance map node overflow".to_string())
            };
            let point = [coordinate(0, x)?, coordinate(1, z)?];
            let (height, colors) = column(point)?;
            let mut profile = [[0.; 4]; 5];
            profile[0] = [colors[0][0], colors[0][1], colors[0][2], 1.];
            for (axis, delta) in [[1_i64, 0_i64], [-1, 0], [0, 1], [0, -1]]
                .into_iter()
                .enumerate()
            {
                let neighbor = [
                    point[0]
                        .checked_add(delta[0])
                        .ok_or("appearance map halo overflow")?,
                    point[1]
                        .checked_add(delta[1])
                        .ok_or("appearance map halo overflow")?,
                ];
                let (neighbor_height, _) = column(neighbor)?;
                let drop = (i128::from(height) - i128::from(neighbor_height)).max(0);
                // Canonical top / soil / deep strata are exactly one, eleven,
                // then remaining native layers. No coarse slope invents faces.
                let areas = [drop.min(1), (drop - 1).clamp(0, 11), (drop - 12).max(0)];
                for (area, color) in areas.into_iter().zip(colors) {
                    let area = area as f32;
                    for (channel, value) in color.into_iter().enumerate() {
                        profile[axis + 1][channel] += value * area;
                    }
                    profile[axis + 1][3] += area;
                }
            }
            profiles.push(profile);
        }
    }
    Ok(AppearanceMap {
        origin_cells: map_origin,
        spacing_cells: spacing,
        dimension,
        profiles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(x: i64, z: i64) -> Result<AppearanceColumn, String> {
        Ok(AppearanceColumn {
            height: 32 + x.rem_euclid(7) + z.rem_euclid(3),
            colors: [[0.2, 0.6, 0.1], [0.4, 0.3, 0.2], [0.5, 0.5, 0.5]],
        })
    }

    #[test]
    fn narrow_canonical_material_stripe_survives_interior_map_nodes() {
        let map = build_map_with_intervals([0, 0], 32, 8, |x, _| {
            let color = if x == 12 { [1., 0., 0.] } else { [0., 1., 0.] };
            Ok(AppearanceColumn {
                height: 10,
                colors: [color; 3],
            })
        })
        .unwrap();
        assert_eq!(map.spacing_cells, 4);
        assert_eq!(map.dimension, 9);
        // Four patch corners all miss this one-native-voxel-wide stripe.
        assert_eq!(map.profiles[3][0], [1., 0., 0., 1.]);
        assert_eq!(map.profiles[0][0], [0., 1., 0., 1.]);
        assert_eq!(map.profiles[8][0], [0., 1., 0., 1.]);
        assert_eq!(map.profiles[2][0], [0., 1., 0., 1.]);
    }

    #[test]
    fn global_tile_edges_have_identical_canonical_profiles() {
        let a = build_map_with_intervals([-32, -32], 32, 8, source).unwrap();
        let b = build_map_with_intervals([0, -32], 32, 8, source).unwrap();
        for z in 0..a.dimension {
            assert_eq!(
                a.profiles[z * a.dimension + a.dimension - 1],
                b.profiles[z * b.dimension]
            );
        }
    }

    #[test]
    fn negative_unaligned_origins_floor_and_cover_patch() {
        let map = build_map_with_intervals([-33, -35], 32, 8, source).unwrap();
        assert_eq!(map.origin_cells, [-36, -36]);
        assert_eq!(map.dimension, 10);
        assert!(map.origin_cells[0] + (map.dimension as i64 - 1) * map.spacing_cells >= -1);
        let aligned = build_map_with_intervals([-36, -36], 32, 8, source).unwrap();
        for z in 0..aligned.dimension {
            for x in 0..aligned.dimension {
                assert_eq!(
                    map.profiles[z * map.dimension + x],
                    aligned.profiles[z * aligned.dimension + x]
                );
            }
        }
    }

    #[test]
    fn work_is_bounded_at_all_supported_densities_and_reuses_native_halo() {
        for intervals in [2, 4, 8] {
            let aligned = build_map_with_intervals([0, 0], 1024, intervals, source).unwrap();
            assert_eq!(aligned.dimension, intervals + 1);
            for edge in [1, 16, 32, 33, 1024, 1 << 30] {
                let mut queries = std::collections::HashSet::new();
                let map = build_map_with_intervals([-33, -35], edge, intervals, |x, z| {
                    assert!(queries.insert([x, z]), "duplicate source query");
                    source(x, z)
                })
                .unwrap();
                assert!(map.dimension <= intervals + 2);
                assert!(queries.len() <= 5 * (intervals + 2).pow(2));
                assert!(queries.len() <= 500);
                assert_eq!(map.spacing_cells.count_ones(), 1);
            }
        }
    }

    #[test]
    fn density_configuration_is_strict_and_defaults_to_two() {
        for input in [
            None,
            Some(""),
            Some("3"),
            Some("16"),
            Some(" 4"),
            Some("04"),
        ] {
            assert_eq!(parse_intervals(input), 2);
        }
        for intervals in [2, 4, 8] {
            assert_eq!(parse_intervals(Some(&intervals.to_string())), intervals);
        }
        assert!(build_map_with_intervals([0, 0], 32, 3, source).is_err());
        let map = build_map([0, 0], 1024, source).unwrap();
        assert_eq!(map.dimension, map_intervals() + 1);
    }

    #[test]
    fn native_side_profiles_preserve_material_strata_and_direction() {
        let map = build_map_with_intervals([0, 0], 1, 8, |x, z| {
            Ok(AppearanceColumn {
                height: if x == 1 && z == 0 { 4 } else { 20 },
                colors: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            })
        })
        .unwrap();
        assert_eq!(map.profiles[0][1], [1., 11., 4., 16.]);
        assert_eq!(map.profiles[0][2], [0.; 4]);
        assert_eq!(map.profiles[0][3], [0.; 4]);
        assert_eq!(map.profiles[0][4], [0.; 4]);
    }

    #[test]
    fn invalid_ranges_and_source_failures_are_reported() {
        assert!(build_map_with_intervals([0, 0], 0, 8, source).is_err());
        assert!(build_map_with_intervals([i64::MAX, 0], 32, 8, source).is_err());
        assert!(build_map_with_intervals([i64::MIN, 0], 32, 8, source).is_err());
        assert!(
            build_map_with_intervals([0, 0], 32, 8, |_, _| Err("cancelled".into()))
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(
            build_map_with_intervals([0, 0], 32, 8, |_, _| Ok(AppearanceColumn {
                height: 0,
                colors: [[f32::NAN; 3]; 3]
            }))
            .is_err()
        );
    }
}
