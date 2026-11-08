//! Bounded detailed appearance maps for an unchanged coarse geometry patch.
//!
//! Texels contain canonical native-face radiance/area profiles, not invented
//! texture or subvoxel noise. A quarter-metre target spacing is independent of
//! geometry LOD until the bounded map size forces a larger power-of-two spacing.
//! Point sampling remains the reference default. The opt-in bounded native box
//! prefilter integrates directional color-area profiles before reconstruction;
//! Footprint mode adds four deterministic strata across each texel. These are
//! bounded estimators, not full pixel-footprint AA or hierarchy mip restrictions.

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

/// Bounded native-cell support, independent of coarse patch extent. Filtering
/// averages face color-area integrals, never normalized RGB or fitted normals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MapPrefilter {
    Off,
    /// Four deterministic strata across the texel, each native-microfiltered.
    Footprint,
    NativeBox {
        radius_cells: i64,
    },
}

impl MapPrefilter {
    /// Include this value in a node memo key: nodes at the same world position
    /// can have different footprints when their map spacing differs.
    pub(crate) fn footprint_radius(self, spacing_cells: i64) -> i64 {
        match self {
            Self::Off => 0,
            Self::Footprint => 1,
            Self::NativeBox { radius_cells } => radius_cells.min(spacing_cells / 2),
        }
    }

    /// Point/native keys are negative; footprint keys are positive actual
    /// spacings. Equivalent small footprints share the native radius-one key.
    /// These keys are for the appearance-map memo namespace only.
    pub(crate) fn cache_key(self, spacing_cells: i64) -> i64 {
        match self {
            Self::Off => -1,
            Self::Footprint if spacing_cells > 4 => spacing_cells,
            _ => -1 - self.footprint_radius(spacing_cells),
        }
    }

    /// Upper bound on distinct canonical column queries per node. A square
    /// native support adds four one-cell-wide halo strips, with no corners.
    pub(crate) fn max_probe_columns(self, spacing_cells: i64) -> usize {
        if self == Self::Off {
            return 5;
        }
        let width = 2 * self.footprint_radius(spacing_cells) as usize + 1;
        let strata = if self == Self::Footprint && spacing_cells > 4 {
            4
        } else {
            1
        };
        strata * (width * width + 4 * width) + usize::from(strata > 1)
    }
}

/// FRONTIER_MAP_PREFILTER=off|on|footprint (default off). Native `on`
/// accepts FRONTIER_MAP_PREFILTER_RADIUS_CELLS=1|2|4, default 1. Footprint
/// uses four radius-one microfilters, bounded to 85 source columns per node, including the center policy query.
static PREFILTER: std::sync::OnceLock<MapPrefilter> = std::sync::OnceLock::new();

/// Viewer meadow policy. Explicit environment settings remain authoritative;
/// tests and file-format fixtures retain point sampling unless initialized here.
/// Radius two integrates a full four-cell staircase phase without increasing
/// map density or applying a texel-sized blur to canonical rock detail.
pub(crate) fn initialize_meadow_application() {
    PREFILTER.get_or_init(|| {
        let mode = std::env::var("FRONTIER_MAP_PREFILTER").ok();
        let radius = std::env::var("FRONTIER_MAP_PREFILTER_RADIUS_CELLS").ok();
        meadow_prefilter(mode.as_deref(), radius.as_deref())
    });
}
fn meadow_prefilter(mode: Option<&str>, radius: Option<&str>) -> MapPrefilter {
    match mode {
        None => parse_prefilter(Some("on"), radius.or(Some("2"))),
        Some(_) => parse_prefilter(mode, radius),
    }
}

pub(crate) fn map_prefilter() -> MapPrefilter {
    *PREFILTER.get_or_init(|| {
        parse_prefilter(
            std::env::var("FRONTIER_MAP_PREFILTER").ok().as_deref(),
            std::env::var("FRONTIER_MAP_PREFILTER_RADIUS_CELLS")
                .ok()
                .as_deref(),
        )
    })
}

/// Native microfilter support only. Use effective_filter_cache_key for memo
/// identity: Footprint also depends on the full texel spacing.
pub(crate) fn effective_filter_radius(spacing_cells: i64) -> i64 {
    map_prefilter().footprint_radius(spacing_cells)
}

/// Active configuration discriminator in the appearance-map memo namespace.
pub(crate) fn effective_filter_cache_key(spacing_cells: i64) -> i64 {
    map_prefilter().cache_key(spacing_cells)
}

fn parse_prefilter(mode: Option<&str>, radius: Option<&str>) -> MapPrefilter {
    match mode {
        Some("footprint") => MapPrefilter::Footprint,
        Some("on") => MapPrefilter::NativeBox {
            radius_cells: match radius {
                Some("4") => 4,
                Some("2") => 2,
                _ => 1,
            },
        },
        _ => MapPrefilter::Off,
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
/// Point nodes cost at most five canonical queries. Filtered nodes use at most
/// (2r+1)^2 native profiles (r <= 4), with source overlap reused locally;
/// the caller may additionally reuse its immutable world/revision source cache.
pub(crate) fn build_map(
    origin: [i64; 2],
    edge: i64,
    sample: impl FnMut(i64, i64) -> Result<AppearanceColumn, String>,
) -> Result<AppearanceMap, String> {
    build_map_with_config(origin, edge, map_intervals(), map_prefilter(), sample)
}

/// Explicit-density variant for bounded experiments and deterministic tests.
/// Aligned patches have at most intervals+1 nodes per axis; an unaligned patch
/// can need one extra node to preserve global alignment and full coverage.
pub(crate) fn build_map_with_intervals(
    origin: [i64; 2],
    edge: i64,
    intervals: usize,
    sample: impl FnMut(i64, i64) -> Result<AppearanceColumn, String>,
) -> Result<AppearanceMap, String> {
    build_map_with_config(origin, edge, intervals, MapPrefilter::Off, sample)
}

/// Deterministic explicit configuration; the density-only variant retains the
/// original point reference regardless of process environment.
pub(crate) fn build_map_with_config(
    origin: [i64; 2],
    edge: i64,
    intervals: usize,
    prefilter: MapPrefilter,
    sample: impl FnMut(i64, i64) -> Result<AppearanceColumn, String>,
) -> Result<AppearanceMap, String> {
    build_map_with_config_and_predicate(origin, edge, intervals, prefilter, |_| true, sample)
}

/// Optional display policy keyed solely to the canonical center top color.
/// Excluded nodes retain the bit-identical point profile, including all native
/// side strata. Filtering at allowed nodes still integrates canonical colors
/// throughout its support; this policy does not relabel or modify the world.
pub(crate) fn build_map_with_config_and_predicate(
    origin: [i64; 2],
    edge: i64,
    intervals: usize,
    prefilter: MapPrefilter,
    allowed: impl Fn([f32; 3]) -> bool,
    mut sample: impl FnMut(i64, i64) -> Result<AppearanceColumn, String>,
) -> Result<AppearanceMap, String> {
    if !matches!(
        prefilter,
        MapPrefilter::Off
            | MapPrefilter::Footprint
            | MapPrefilter::NativeBox {
                radius_cells: 1 | 2 | 4
            }
    ) {
        return Err("appearance map prefilter radius must be 1, 2 or 4 native cells".into());
    }
    let map = layout(origin, edge, intervals)?;
    let map_origin = map.origin_cells;
    let spacing = map.spacing_cells;
    let dimension = map.dimension;
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
            let profile = node_profile(point, spacing, prefilter, &allowed, &mut column)?;
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

/// Globally aligned layout, available before any expensive source queries.
pub(crate) fn layout(
    origin: [i64; 2],
    edge: i64,
    intervals: usize,
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
    Ok(AppearanceMap {
        origin_cells: map_origin,
        spacing_cells: spacing,
        dimension,
        profiles: Vec::new(),
    })
}

/// Evaluate exactly one node. Callers may memoize the result using world,
/// position, effective footprint and surface-filter policy identity.
pub(crate) fn node_profile(
    point: [i64; 2],
    spacing: i64,
    prefilter: MapPrefilter,
    allowed: &impl Fn([f32; 3]) -> bool,
    column: &mut impl FnMut([i64; 2]) -> Result<(i64, [[f32; 3]; 3]), String>,
) -> Result<Profile, String> {
    let node_prefilter = if prefilter != MapPrefilter::Off && !allowed(column(point)?.1[0]) {
        MapPrefilter::Off
    } else {
        prefilter
    };
    let profile = if node_prefilter == MapPrefilter::Footprint && spacing > 4 {
        let mut filtered = [[0.; 4]; 5];
        for probe in footprint_probes(point, spacing)? {
            add_weighted(&mut filtered, box_profile(probe, 1, column)?, 0.25);
        }
        filtered
    } else {
        box_profile(point, node_prefilter.footprint_radius(spacing), column)?
    };
    Ok(profile)
}

fn add_weighted(target: &mut Profile, source: Profile, weight: f32) {
    for direction in 0..5 {
        for channel in 0..4 {
            target[direction][channel] += source[direction][channel] * weight;
        }
    }
}

fn box_profile(
    point: [i64; 2],
    radius: i64,
    column: &mut impl FnMut([i64; 2]) -> Result<(i64, [[f32; 3]; 3]), String>,
) -> Result<Profile, String> {
    if radius == 0 {
        return native_profile(point, column);
    }
    // Centered width-2r box; boundary cells cover half a cell each.
    let mut filtered = [[0.; 4]; 5];
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let probe = [
                point[0]
                    .checked_add(dx)
                    .ok_or("appearance map footprint overflow")?,
                point[1]
                    .checked_add(dz)
                    .ok_or("appearance map footprint overflow")?,
            ];
            let weight = (if dx.abs() == radius { 0.5 } else { 1. })
                * (if dz.abs() == radius { 0.5 } else { 1. })
                / (4 * radius * radius) as f32;
            add_weighted(&mut filtered, native_profile(probe, column)?, weight);
        }
    }
    Ok(filtered)
}

/// Four strata cover a spacing-sized square centered on this global node.
/// Hash jitter breaks the regular native-height phase without changing across
/// patches, frames or world seeds. This bounded quadrature estimates a texel
/// footprint; four samples cannot guarantee removal of every source frequency.
fn footprint_probes(point: [i64; 2], spacing: i64) -> Result<[[i64; 2]; 4], String> {
    fn mix(mut value: u64) -> u64 {
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }
    let seed = mix(point[0] as u64)
        ^ mix((point[1] as u64).wrapping_add(0x9e3779b97f4a7c15))
        ^ mix(spacing as u64);
    let half = spacing / 2;
    let mut probes = [[0; 2]; 4];
    for (stratum, probe) in probes.iter_mut().enumerate() {
        for axis in 0..2 {
            let jitter = mix(seed.wrapping_add((2 * stratum + axis) as u64)) % half as u64;
            let offset = -i128::from(half)
                + ((stratum >> axis) & 1) as i128 * i128::from(half)
                + i128::from(jitter);
            probe[axis] = i64::try_from(i128::from(point[axis]) + offset)
                .map_err(|_| "appearance map footprint overflow")?;
        }
    }
    Ok(probes)
}

/// Canonical one-native-cell top and directional exposed side integrals.
fn native_profile(
    point: [i64; 2],
    column: &mut impl FnMut([i64; 2]) -> Result<(i64, [[f32; 3]; 3]), String>,
) -> Result<Profile, String> {
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
    Ok(profile)
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
    fn quantized_plane_phase_variation_is_reduced_without_inventing_side_direction() {
        for (radius, expected_range) in [(1, 0.5_f32), (2, 0.), (4, 0.)] {
            let mut point_areas = Vec::new();
            let mut filtered_areas = Vec::new();
            for phase in 0..4 {
                let plane = |x: i64, _: i64| {
                    Ok(AppearanceColumn {
                        height: (x + phase).div_euclid(4),
                        colors: [[0.2, 0.6, 0.1]; 3],
                    })
                };
                let point = build_map_with_intervals([0, 0], 16, 2, plane).unwrap();
                let filtered = build_map_with_config(
                    [0, 0],
                    16,
                    2,
                    MapPrefilter::NativeBox {
                        radius_cells: radius,
                    },
                    plane,
                )
                .unwrap();
                point_areas.push(point.profiles[0][2][3]);
                filtered_areas.push(filtered.profiles[0][2][3]);
                assert_eq!(filtered.profiles[0][1], [0.; 4]);
                assert_eq!(filtered.profiles[0][3], [0.; 4]);
                assert_eq!(filtered.profiles[0][4], [0.; 4]);
                // Color-area stays proportional to actual canonical face area.
                assert!(
                    (filtered.profiles[0][2][1] - 0.6 * filtered.profiles[0][2][3]).abs() < 1e-6
                );
            }
            let range = |values: &[f32]| {
                values.iter().copied().fold(f32::NEG_INFINITY, f32::max)
                    - values.iter().copied().fold(f32::INFINITY, f32::min)
            };
            assert_eq!(range(&point_areas), 1.);
            assert_eq!(range(&filtered_areas), expected_range);
            assert!((filtered_areas.iter().sum::<f32>() / 4. - 0.25).abs() < 1e-6);
        }
    }

    #[test]
    fn constant_native_slope_preserves_directional_strata_integrals() {
        let plane = |x: i64, z: i64| {
            Ok(AppearanceColumn {
                height: 16 * x - 2 * z,
                colors: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            })
        };
        let point = build_map_with_intervals([-32, -32], 32, 8, plane).unwrap();
        for radius in [1, 2, 4] {
            let filtered = build_map_with_config(
                [-32, -32],
                32,
                8,
                MapPrefilter::NativeBox {
                    radius_cells: radius,
                },
                plane,
            )
            .unwrap();
            assert_eq!(filtered.profiles, point.profiles);
            assert_eq!(filtered.profiles[0][2], [1., 11., 4., 16.]);
            assert_eq!(filtered.profiles[0][3], [1., 1., 0., 2.]);
        }
    }

    #[test]
    fn material_and_cliff_changes_are_confined_to_bounded_native_support() {
        let map = build_map_with_config(
            [-8, 0],
            16,
            4,
            MapPrefilter::NativeBox { radius_cells: 2 },
            |x, _| {
                let (height, color) = if x < 0 {
                    (20, [1., 0., 0.])
                } else {
                    (0, [0., 1., 0.])
                };
                Ok(AppearanceColumn {
                    height,
                    colors: [color; 3],
                })
            },
        )
        .unwrap();
        // Nodes four cells from the boundary retain pure canonical materials.
        assert_eq!(map.profiles[1][0], [1., 0., 0., 1.]);
        assert_eq!(map.profiles[3][0], [0., 1., 0., 1.]);
        assert_eq!(map.profiles[1][1], [0.; 4]);
        assert_eq!(map.profiles[3][1], [0.; 4]);
        // At the boundary, side RGB*area belongs to the red upper cliff;
        // averaging RGB first would incorrectly introduce green side radiance.
        assert_eq!(map.profiles[2][1], [5., 0., 0., 5.]);
    }

    #[test]
    fn filtered_negative_shared_nodes_match_and_source_work_stays_bounded() {
        for radius in [1, 2, 4] {
            let config = MapPrefilter::NativeBox {
                radius_cells: radius,
            };
            let left = build_map_with_config([-32, -32], 32, 8, config, source).unwrap();
            let right = build_map_with_config([0, -32], 32, 8, config, source).unwrap();
            for z in 0..left.dimension {
                assert_eq!(
                    left.profiles[z * left.dimension + left.dimension - 1],
                    right.profiles[z * right.dimension]
                );
            }
            for edge in [1, 33, 1024, 1 << 30] {
                let mut queries = std::collections::HashSet::new();
                let map = build_map_with_config([-33, -35], edge, 8, config, |x, z| {
                    assert!(queries.insert([x, z]));
                    source(x, z)
                })
                .unwrap();
                let r = config.footprint_radius(map.spacing_cells) as usize;
                // Square native support plus four one-cell halo strips.
                let per_node = (2 * r + 1).pow(2) + 4 * (2 * r + 1);
                assert!(queries.len() <= map.dimension.pow(2) * per_node);
                assert!(queries.len() <= 11_700);
            }
        }
    }

    #[test]
    fn meadow_policy_filters_full_staircase_phase_with_bounded_work_and_overrides() {
        let candidate = MapPrefilter::NativeBox { radius_cells: 2 };
        assert_eq!(meadow_prefilter(None, None), candidate);
        assert_eq!(candidate.max_probe_columns(1024), 45);
        assert_eq!(meadow_prefilter(Some("off"), None), MapPrefilter::Off);
        assert_eq!(
            meadow_prefilter(Some("footprint"), None),
            MapPrefilter::Footprint
        );
        assert_eq!(
            meadow_prefilter(None, Some("1")),
            MapPrefilter::NativeBox { radius_cells: 1 }
        );
        assert_eq!(
            meadow_prefilter(Some("on"), None),
            MapPrefilter::NativeBox { radius_cells: 1 }
        );
        // The canonical quarter-slope can alias to a constant wrong side area
        // at arbitrarily large map spacings. Integrate native phase first.
        for edge in [16, 128, 1024] {
            let mut areas = Vec::new();
            for phase in 0..4 {
                let map = build_map_with_config([0, 0], edge, 2, candidate, |x, _| {
                    Ok(AppearanceColumn {
                        height: (x + phase).div_euclid(4),
                        colors: [[0.2, 0.6, 0.1]; 3],
                    })
                })
                .unwrap();
                areas.push(map.profiles[0][2][3]);
            }
            assert_eq!(areas, vec![0.25; 4]);
        }
    }
    #[test]
    fn prefilter_configuration_is_opt_in_and_radius_is_native_bounded() {
        for mode in [None, Some("off"), Some("ON"), Some(""), Some("true")] {
            assert_eq!(parse_prefilter(mode, Some("4")), MapPrefilter::Off);
        }
        for (value, expected) in [
            (None, 1),
            (Some("1"), 1),
            (Some("2"), 2),
            (Some("4"), 4),
            (Some("8"), 1),
        ] {
            let config = parse_prefilter(Some("on"), value);
            assert_eq!(
                config,
                MapPrefilter::NativeBox {
                    radius_cells: expected
                }
            );
            assert_eq!(config.footprint_radius(1 << 30), expected);
            assert_eq!(config.footprint_radius(4), expected.min(2));
        }
        assert!(build_map_with_config(
            [0, 0],
            16,
            2,
            MapPrefilter::NativeBox { radius_cells: 3 },
            source
        )
        .is_err());
    }

    #[test]
    fn footprint_softens_quantized_relief_over_several_native_periods() {
        for period in [12, 20, 32, 64] {
            let mut point_error = 0.;
            let mut footprint_error = 0.;
            for phase in 0..period {
                let field = |x: i64, _: i64| {
                    let wave = ((x + phase) as f64 * std::f64::consts::TAU / period as f64).sin();
                    let height = (100. + 8. * wave).floor() as i64;
                    let color = (height - 92) as f32 / 16.;
                    Ok(AppearanceColumn {
                        height,
                        colors: [[color; 3]; 3],
                    })
                };
                let point = build_map_with_intervals([-128, -128], 128, 2, field).unwrap();
                let footprint =
                    build_map_with_config([-128, -128], 128, 2, MapPrefilter::Footprint, field)
                        .unwrap();
                for (a, b) in point.profiles.iter().zip(&footprint.profiles) {
                    point_error += (a[0][0] - 0.5).powi(2);
                    footprint_error += (b[0][0] - 0.5).powi(2);
                }
            }
            assert!(
                footprint_error < point_error * 0.6,
                "period {period}: footprint error {footprint_error}, point error {point_error}"
            );
        }
    }

    #[test]
    fn footprint_shared_nodes_and_flat_integrals_are_stable() {
        let config = MapPrefilter::Footprint;
        let left = build_map_with_config([-128, -128], 128, 2, config, source).unwrap();
        let right = build_map_with_config([0, -128], 128, 2, config, source).unwrap();
        for z in 0..left.dimension {
            assert_eq!(
                left.profiles[z * left.dimension + left.dimension - 1],
                right.profiles[z * right.dimension]
            );
        }
        let flat = |_: i64, _: i64| {
            Ok(AppearanceColumn {
                height: 12,
                colors: [[0.25, 0.5, 1.]; 3],
            })
        };
        let filtered = build_map_with_config([-128, -128], 128, 2, config, flat).unwrap();
        let point = build_map_with_intervals([-128, -128], 128, 2, flat).unwrap();
        assert_eq!(filtered.profiles, point.profiles);
        let small = build_map_with_config([-8, -8], 8, 2, config, source).unwrap();
        let native = build_map_with_config(
            [-8, -8],
            8,
            2,
            MapPrefilter::NativeBox { radius_cells: 1 },
            source,
        )
        .unwrap();
        assert_eq!(small.profiles, native.profiles);
    }

    #[test]
    fn footprint_cache_keys_and_query_budget_include_actual_spacing() {
        let config = MapPrefilter::Footprint;
        assert_eq!(parse_prefilter(Some("footprint"), None), config);
        assert_eq!(
            config.cache_key(4),
            MapPrefilter::NativeBox { radius_cells: 1 }.cache_key(4)
        );
        assert_ne!(config.cache_key(8), config.cache_key(16));
        for spacing in [8, 64, 1 << 30, 1 << 60] {
            assert!(config.cache_key(spacing) > 0);
            assert_eq!(config.max_probe_columns(spacing), 85);
        }
        for edge in [8, 128, 1024, 1 << 30] {
            let mut queries = std::collections::HashSet::new();
            let map = build_map_with_config([-133, -137], edge, 2, config, |x, z| {
                assert!(queries.insert([x, z]), "source cache must reuse overlap");
                source(x, z)
            })
            .unwrap();
            assert!(
                queries.len() <= map.dimension.pow(2) * config.max_probe_columns(map.spacing_cells)
            );
            assert!(config.max_probe_columns(map.spacing_cells) <= 100);
        }
        for spacing in [8, 64, 1024] {
            let probes = footprint_probes([-128, -256], spacing).unwrap();
            assert_eq!(probes, footprint_probes([-128, -256], spacing).unwrap());
            for (stratum, probe) in probes.into_iter().enumerate() {
                for axis in 0..2 {
                    let lower = [-128, -256][axis] - spacing / 2
                        + ((stratum >> axis) & 1) as i64 * (spacing / 2);
                    assert!((lower..lower + spacing / 2).contains(&probe[axis]));
                }
            }
        }
    }

    #[test]
    fn surface_policy_keeps_excluded_stone_point_profiles_and_filters_grass_phase() {
        let grass = [0.2, 0.6, 0.1];
        let stone = [0.5, 0.5, 0.5];
        for config in [
            MapPrefilter::NativeBox { radius_cells: 2 },
            MapPrefilter::Footprint,
        ] {
            for phase in 0..4 {
                let field = |x: i64, z: i64| {
                    Ok(AppearanceColumn {
                        height: (x + phase).div_euclid(4) + z.rem_euclid(3),
                        colors: [if x < 0 { stone } else { grass }; 3],
                    })
                };
                let point = build_map_with_intervals([-16, -16], 32, 8, field).unwrap();
                let filtered = build_map_with_config([-16, -16], 32, 8, config, field).unwrap();
                let policy = build_map_with_config_and_predicate(
                    [-16, -16],
                    32,
                    8,
                    config,
                    |rgb| rgb == grass,
                    field,
                )
                .unwrap();
                for z in 0..policy.dimension {
                    for x in 0..policy.dimension {
                        let index = z * policy.dimension + x;
                        assert_eq!(
                            policy.profiles[index],
                            if policy.origin_cells[0] + x as i64 * policy.spacing_cells < 0 {
                                point.profiles[index]
                            } else {
                                filtered.profiles[index]
                            }
                        );
                    }
                }
                // At x=4, radius-two filtering reduces the quantized grass
                // phase to the correct quarter-native-cell side coverage.
                if config == (MapPrefilter::NativeBox { radius_cells: 2 }) {
                    assert_eq!(policy.profiles[5][2][3], 0.25);
                }
                let neighbor = build_map_with_config_and_predicate(
                    [16, -16],
                    32,
                    8,
                    config,
                    |rgb| rgb == grass,
                    field,
                )
                .unwrap();
                for z in 0..policy.dimension {
                    assert_eq!(
                        policy.profiles[z * policy.dimension + policy.dimension - 1],
                        neighbor.profiles[z * neighbor.dimension]
                    );
                }
            }
            let mut queries = std::collections::HashSet::new();
            let map = build_map_with_config_and_predicate(
                [-128, -128],
                128,
                2,
                config,
                |rgb| rgb == grass,
                |x, z| {
                    assert!(queries.insert([x, z]));
                    Ok(AppearanceColumn {
                        height: x.div_euclid(4) + z.rem_euclid(3),
                        colors: [if x < 0 { stone } else { grass }; 3],
                    })
                },
            )
            .unwrap();
            assert!(
                queries.len() <= map.dimension.pow(2) * config.max_probe_columns(map.spacing_cells)
            );
        }
    }

    #[test]
    fn anisotropic_reconstruction_is_continuous_across_shared_map_edges() {
        // A non-affine signal makes asymmetric edge clamping and losing a
        // tangent footprint observable; constant/linear nodes would hide both.
        let field = |x: i64, z: i64| {
            let color = ((x * x + 3 * z * z).rem_euclid(97)) as f32 / 97.;
            Ok(AppearanceColumn {
                height: 32,
                colors: [[color; 3]; 3],
            })
        };
        let left = build_map_with_intervals([-32, -32], 32, 4, field).unwrap();
        let right = build_map_with_intervals([0, -32], 32, 4, field).unwrap();
        let bilinear = |map: &AppearanceMap, uv: [f64; 2]| {
            let extent = (map.dimension - 1) as f64;
            let uv = uv.map(|v| v.clamp(0., extent));
            let cell = uv.map(|v| (v.floor() as usize).min(map.dimension - 2));
            let fraction = [uv[0] - cell[0] as f64, uv[1] - cell[1] as f64];
            let mut value = 0.;
            for dz in 0..2 {
                for dx in 0..2 {
                    let weight = if dx == 0 {
                        1. - fraction[0]
                    } else {
                        fraction[0]
                    } * if dz == 0 {
                        1. - fraction[1]
                    } else {
                        fraction[1]
                    };
                    value += map.profiles[(cell[1] + dz) * map.dimension + cell[0] + dx][0][0]
                        as f64
                        * weight;
                }
            }
            value
        };
        // CPU reconstruction of the shader's symmetric footprint contraction.
        let reconstruct = |map: &AppearanceMap, uv: [f64; 2], mut axis: [f64; 2], taps: usize| {
            let length = axis[0].hypot(axis[1]);
            for a in &mut axis {
                *a *= (4. / length).min(1.);
            }
            let extent = (map.dimension - 1) as f64;
            let mut support = 1_f64;
            for a in 0..2 {
                if axis[a].abs() > 0.000001 {
                    let center = uv[a].clamp(0., extent);
                    let room = center.min(extent - center).max(0.);
                    support = support.min((2. * room / axis[a].abs()).min(1.));
                }
            }
            (0..taps)
                .map(|i| {
                    let offset = (i as f64 + 0.5) / taps as f64 - 0.5;
                    bilinear(
                        map,
                        [
                            uv[0] + axis[0] * support * offset,
                            uv[1] + axis[1] * support * offset,
                        ],
                    )
                })
                .sum::<f64>()
                / taps as f64
        };
        let edge = (left.dimension - 1) as f64;
        for taps in [2, 4] {
            for axis in [[2., 0.], [2., 1.], [-1., 2.], [0., 2.]] {
                for y in [0.9, 1.8, 2.9] {
                    let a = reconstruct(&left, [edge, y], axis, taps);
                    let b = reconstruct(&right, [0., y], axis, taps);
                    assert!((a - b).abs() < 1e-12);
                    for epsilon in [1e-4, 1e-7] {
                        let l = reconstruct(&left, [edge - epsilon, y], axis, taps);
                        let r = reconstruct(&right, [epsilon, y], axis, taps);
                        assert!((l - a).abs() < 4. * epsilon);
                        assert!((r - b).abs() < 4. * epsilon);
                    }
                }
            }
            // Filtering parallel to the boundary must remain active there.
            let tangent = reconstruct(&left, [edge, 0.9], [0., 2.], taps);
            assert!((tangent - bilinear(&left, [edge, 0.9])).abs() > 0.001);
        }
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
