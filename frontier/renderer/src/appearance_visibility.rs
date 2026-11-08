//! Bounded, view-conditioned height-column appearance experiment (METHOD2).
//!
//! Inspired by directional material/normal distributions and separate visibility
//! in HPG 2023 "Voxel-based Representations for Improved Filtered Appearance"
//! (https://cjsb.github.io/hpg2023/voxel-filtered-appearance.pdf), and visibility-conditioned aggregation
//! in "Efficient Scene Appearance Aggregation for Level-of-Detail Rendering"
//! (https://arxiv.org/abs/2409.03761). This is not either paper's implementation:
//! cardinal height-grid horizons replace scene-wide visibility integration.
//! Geometry, camera and light never enter the cached canonical source identity.

use crate::appearance_hierarchy::AppearanceColumn;
use std::collections::HashMap;

/// V2 keeps spatial interpolation near the native handoff. The raw V1 path
/// remains selectable only for reproducing the initial experiment.
pub(crate) const SPATIAL_MAX_EDGE_CELLS: i64 = 64;
pub(crate) fn spatial_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FRONTIER_VISIBILITY_SPATIAL").as_deref() != Ok("0"))
}

pub(crate) const VIEW_COUNT: usize = 8;
/// Surface-to-camera direction: +X, +Z, -X, -Z, repeated at each elevation.
/// Concentrate the bounded bins on the primary player-height/grazing view:
/// approximately 1.8 and 14 degrees above horizontal. Runtime interpolation
/// clamps below/above these elevations; higher aerial views remain approximate.
pub(crate) const ELEVATION_TANGENTS: [f32; 2] = [0.03125, 0.25];
const MAX_CORE_SIDE: usize = 10;
pub(crate) const MAX_SAMPLED_COLUMNS: usize =
    (MAX_CORE_SIDE + 2) * (MAX_CORE_SIDE + 2) + 4 * MAX_CORE_SIDE * MAX_CORE_SIDE;
type Profile = [[f32; 4]; 5];

/// Bounded cost/quality experiment. Six is the default (at most 208 source
/// columns); ten retains the original denser estimator (at most 544).
pub(crate) fn grid_side() -> usize {
    static GRID: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *GRID.get_or_init(|| parse_grid(std::env::var("FRONTIER_VISIBILITY_GRID").ok().as_deref()))
}

fn parse_grid(value: Option<&str>) -> usize {
    match value {
        Some("10") => 10,
        _ => 6,
    }
}

pub(crate) struct VisibleAppearance {
    /// Index = elevation * 4 + azimuth. Faces: +Y,+X,-X,+Z,-Z.
    /// Every face stores [integral R, integral G, integral B, visible area].
    /// No projection cosine or lighting is baked; apply those during shading.
    pub profiles: [Profile; VIEW_COUNT],
    pub sampled_columns: usize,
}

/// Default grid: at most 208 unique columns (64 grid/halo + 4*36 native
/// neighbors). The opt-in ten-grid uses at most 544. Probes are memoized.
/// The grid integrates the whole footprint, but cannot discover subgrid ridges,
/// caves or distant occluders beyond its halo. These are empirical profiles,
/// never geometric/error certificates. Three sampled material depths approximate
/// wall stratigraphy: depth [0,1), [1,12), and [12,infinity) native cells.
pub(crate) fn build_visible_appearance<F>(
    origin: [i64; 2],
    edge: i64,
    sample: F,
) -> Result<VisibleAppearance, String>
where
    F: FnMut(i64, i64) -> Result<AppearanceColumn, String>,
{
    build_with_grid(origin, edge, grid_side(), sample)
}

/// Explicit grid density for reproducible tests and matched experiments.
/// Only the two supported bounded estimators (6 and 10) are accepted.
pub(crate) fn build_with_grid<F>(
    origin: [i64; 2],
    edge: i64,
    grid: usize,
    mut sample: F,
) -> Result<VisibleAppearance, String>
where
    F: FnMut(i64, i64) -> Result<AppearanceColumn, String>,
{
    if !matches!(grid, 6 | 10) {
        return Err("visibility grid must be 6 or 10".into());
    }
    if edge <= 0 {
        return Err("visibility appearance requires a positive edge".into());
    }
    let n = edge.min(grid as i64) as usize;
    // Integer partitions keep negative world coordinates and tiny patches exact.
    let mut widths = vec![0.; n + 2];
    let mut centers = vec![0_i64; n + 2];
    for i in 0..n {
        let lo = (edge as i128 * i as i128 / n as i128) as i64;
        let hi = (edge as i128 * (i + 1) as i128 / n as i128) as i64;
        widths[i + 1] = (hi - lo) as f64;
        centers[i + 1] = lo + (hi - lo) / 2;
    }
    widths[0] = widths[1];
    widths[n + 1] = widths[n];
    centers[0] = -(widths[0] as i64) + centers[1];
    centers[n + 1] = centers[n]
        .checked_add(widths[n + 1] as i64)
        .ok_or("visibility appearance coordinate overflow")?;
    let side = n + 2;
    let mut cached = HashMap::with_capacity((n + 2) * (n + 2) + 4 * n * n);
    let mut fetch = |x: i64, z: i64| -> Result<AppearanceColumn, String> {
        if !cached.contains_key(&(x, z)) {
            cached.insert((x, z), sample(x, z)?);
        }
        let col: &AppearanceColumn = &cached[&(x, z)];
        Ok(AppearanceColumn {
            height: col.height,
            colors: col.colors,
        })
    };
    let mut columns = Vec::with_capacity(side * side);
    for &z in &centers {
        for &x in &centers {
            columns.push(fetch(
                origin[0]
                    .checked_add(x)
                    .ok_or("visibility appearance coordinate overflow")?,
                origin[1]
                    .checked_add(z)
                    .ok_or("visibility appearance coordinate overflow")?,
            )?);
        }
    }
    let directions: [(isize, isize, usize); 4] = [(1, 0, 1), (0, 1, 3), (-1, 0, 2), (0, -1, 4)];
    // Native exposed-face integrals represent local area density, not the
    // macrogrid's intersample height change. Stretching the latter into material
    // depths would turn a gently sloping grassy staircase into a deep soil wall.
    let mut native_walls = vec![[[0_f64; 4]; 5]; n * n];
    for z in 1..=n {
        for x in 1..=n {
            let column = &columns[z * side + x];
            let wall = &mut native_walls[(z - 1) * n + x - 1];
            for &(dx, dz, face) in &directions {
                let sx = origin[0]
                    .checked_add(centers[x])
                    .and_then(|v| v.checked_add(dx as i64))
                    .ok_or("visibility appearance coordinate overflow")?;
                let sz = origin[1]
                    .checked_add(centers[z])
                    .and_then(|v| v.checked_add(dz as i64))
                    .ok_or("visibility appearance coordinate overflow")?;
                let neighbor = fetch(sx, sz)?;
                let drop = ((column.height as i128 - neighbor.height as i128) as f64).max(0.);
                for (band, lo, hi) in [(0, 0., 1.), (1, 1., 12.), (2, 12., f64::INFINITY)] {
                    let depth = (drop.min(hi) - lo).max(0.);
                    add(
                        wall,
                        face,
                        column.colors[band],
                        depth * widths[x] * widths[z],
                    );
                }
            }
        }
    }
    let mut integrals = [[[0_f64; 4]; 5]; VIEW_COUNT];
    for (elevation, &tangent) in ELEVATION_TANGENTS.iter().enumerate() {
        let tangent = tangent as f64;
        for (azimuth, &(dx, dz, _)) in directions.iter().enumerate() {
            let profile = &mut integrals[elevation * 4 + azimuth];
            for z in 1..=n {
                for x in 1..=n {
                    let column = &columns[z * side + x];
                    let along = if dx != 0 { x } else { z };
                    // Maximum blocking height, relative to this surface, at its
                    // viewer-facing edge. Include each block's nearest boundary,
                    // not its center: a near-flat/grazing ray must not leak through.
                    let mut horizon = f64::NEG_INFINITY;
                    let mut distance = 0.;
                    let mut rx = x as isize + dx;
                    let mut rz = z as isize + dz;
                    while rx >= 0 && rz >= 0 && rx < side as isize && rz < side as isize {
                        let blocker = &columns[rz as usize * side + rx as usize];
                        let relative_h = (blocker.height as i128 - column.height as i128) as f64;
                        horizon = horizon.max(relative_h - tangent * distance);
                        let k = if dx != 0 { rx as usize } else { rz as usize };
                        distance += widths[k];
                        rx += dx;
                        rz += dz;
                    }
                    // Analytically integrate visible top length across the grid
                    // prism; sunlight is deliberately absent from this horizon.
                    let visible_top = (1. - horizon / (tangent * widths[along])).clamp(0., 1.);
                    add(
                        profile,
                        0,
                        column.colors[0],
                        visible_top * widths[x] * widths[z],
                    );
                    let nx = (x as isize + dx) as usize;
                    let nz = (z as isize + dz) as usize;
                    let neighbor = &columns[nz * side + nx];
                    let drop = (column.height as i128 - neighbor.height as i128) as f64;
                    // Common cardinal visibility estimate for ALL normal axes.
                    // Projecting/excluding faces here would attenuate twice when
                    // bins are blended and then projected at a diagonal camera.
                    // This scalar preserves native material-band proportions;
                    // it does not pretend to resolve per-face diagonal occlusion.
                    let wall_visibility = if drop > 0. {
                        (-horizon / drop).clamp(0., 1.)
                    } else {
                        visible_top
                    };
                    let wall = &native_walls[(z - 1) * n + x - 1];
                    for face in 1..5 {
                        for channel in 0..4 {
                            profile[face][channel] += wall[face][channel] * wall_visibility;
                        }
                    }
                }
            }
        }
    }
    Ok(VisibleAppearance {
        profiles: integrals.map(|profile| profile.map(|face| face.map(|value| value as f32))),
        sampled_columns: cached.len(),
    })
}

fn add(profile: &mut [[f64; 4]; 5], face: usize, color: [f32; 3], area: f64) {
    for (channel, value) in color.iter().enumerate() {
        profile[face][channel] += f64::from(*value) * area;
    }
    profile[face][3] += area;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_ten<F>(origin: [i64; 2], edge: i64, sample: F) -> Result<VisibleAppearance, String>
    where
        F: FnMut(i64, i64) -> Result<AppearanceColumn, String>,
    {
        build_with_grid(origin, edge, 10, sample)
    }

    fn column(height: i64, color: [f32; 3]) -> AppearanceColumn {
        AppearanceColumn {
            height,
            colors: [color; 3],
        }
    }

    #[test]
    fn flat_negative_patch_has_identical_top_integrals_at_all_views() {
        let mut coordinates = Vec::new();
        let out = build_ten([-30, -60], 10, |x, z| {
            coordinates.push([x, z]);
            Ok(column(7, [0.25, 0.5, 1.]))
        })
        .unwrap();
        assert_eq!(out.sampled_columns, 144);
        assert!(coordinates.iter().all(|p| p[0] < 0 && p[1] < 0));
        for profile in out.profiles {
            assert_eq!(profile[0], [25., 50., 100., 100.]);
            assert_eq!(&profile[1..], &[[0.; 4]; 4]);
        }
    }

    #[test]
    fn slope_is_directional_and_grazing_view_loses_hidden_top() {
        let out = build_ten([0, 0], 10, |x, _| Ok(column(x, [1.; 3]))).unwrap();
        assert_eq!(out.profiles[0][0][3], 0.); // uphill, tan(view) < tan(slope)
        assert_eq!(out.profiles[2][0][3], 100.); // downhill
        assert_eq!(out.profiles[2][2][3], 100.); // downhill stairs are fully exposed
        assert_eq!(out.profiles[6][2][3], 100.);
        let shallow = build_ten([0, 0], 10, |x, _| Ok(column(x.div_euclid(8), [1.; 3]))).unwrap();
        assert!(shallow.profiles[0][0][3] < shallow.profiles[4][0][3]);
    }

    #[test]
    fn foreground_step_hides_distant_colored_top_and_wall() {
        // Red rear shelf's +X face is locally exposed onto a low trough. A
        // distant foreground ridge hides it despite identical local geometry.
        let build = |ridge| {
            build_ten([0, 0], 10, |x, _| {
                let h = if x < 4 {
                    8
                } else if x >= 8 {
                    ridge
                } else {
                    0
                };
                Ok(column(h, if x < 4 { [1., 0., 0.] } else { [0., 0., 1.] }))
            })
            .unwrap()
        };
        let open = build(0);
        let hidden = build(20);
        assert!(open.profiles[0][1][0] > 0.);
        assert_eq!(hidden.profiles[0][1][0], 0.);
        assert!(hidden.profiles[0][0][0] < open.profiles[0][0][0]);
        assert!(hidden.profiles[2][0][0] > hidden.profiles[0][0][0]);
    }

    #[test]
    fn low_grazing_bin_retains_foreground_occlusion_missing_at_fourteen_degrees() {
        let build = |ridge| {
            build_ten([0, 0], 10, |x, _| {
                let h = if x < 4 {
                    8
                } else if x >= 8 {
                    ridge
                } else {
                    0
                };
                Ok(column(h, if x < 4 { [1., 0., 0.] } else { [0., 0., 1.] }))
            })
            .unwrap()
        };
        let higher_ridge = build(9);
        assert_eq!(higher_ridge.profiles[0][0][0], 0.);
        assert!(higher_ridge.profiles[4][0][0] > 0.);
        let level_ridge = build(8);
        assert!(level_ridge.profiles[0][1][0] > 0.);
        assert!(level_ridge.profiles[0][1][0] < level_ridge.profiles[4][1][0]);
    }

    #[test]
    fn native_wall_bands_are_not_stretched_by_macrogrid_spacing() {
        for rise in [1, 3] {
            let out = build_ten([0, 0], 100, |x, _| {
                Ok(AppearanceColumn {
                    height: rise * x,
                    colors: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                })
            })
            .unwrap();
            assert_eq!(out.sampled_columns, MAX_SAMPLED_COLUMNS);
            let wall = out.profiles[2][2];
            assert_eq!(
                wall,
                [
                    10_000.,
                    (rise - 1) as f32 * 10_000.,
                    0.,
                    rise as f32 * 10_000.
                ]
            );
        }
    }

    #[test]
    fn diagonal_bin_interpolation_preserves_unobstructed_wall_density() {
        let out = build_ten([0, 0], 100, |x, _| Ok(column(x, [1.; 3]))).unwrap();
        // A -X-facing native staircase is unobstructed toward -X and +Z.
        // Keep its full density in both profiles; the shader projects it once
        // against the actual diagonal view, after interpolating these bins.
        let toward_x = out.profiles[2][2][3];
        let toward_z = out.profiles[1][2][3];
        assert_eq!(toward_x, 10_000.);
        assert_eq!(toward_z, toward_x);
        let diagonal_projected = (toward_x + toward_z) * 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(
            diagonal_projected,
            toward_x * std::f32::consts::FRAC_1_SQRT_2
        );
    }

    #[test]
    fn sample_budget_does_not_scale_with_footprint_and_tiny_patch_is_exact() {
        for edge in [1, 2, 10, 1_000_000] {
            let out = build_ten([-1_000_000, -1_000_000], edge, |_, _| {
                Ok(column(i64::MAX, [1.; 3]))
            })
            .unwrap();
            assert!(out.sampled_columns <= MAX_SAMPLED_COLUMNS);
            assert_eq!(out.profiles[0][0][3], (edge as f64 * edge as f64) as f32);
            assert!(out
                .profiles
                .iter()
                .flatten()
                .flatten()
                .all(|v| v.is_finite() && *v >= 0.));
        }
    }

    #[test]
    fn six_grid_preserves_flat_integrals_native_bands_and_work_bound() {
        for edge in [1, 6, 60, 1_000_000] {
            let mut queries = std::collections::HashSet::new();
            let out = build_with_grid([-100, -100], edge, 6, |x, z| {
                assert!(queries.insert([x, z]), "duplicate canonical query");
                Ok(column(100, [0.25, 0.5, 1.]))
            })
            .unwrap();
            assert_eq!(queries.len(), out.sampled_columns);
            assert!(out.sampled_columns <= 208);
            let area = (edge as f64 * edge as f64) as f32;
            for profile in out.profiles {
                assert_eq!(profile[0], [area * 0.25, area * 0.5, area, area]);
                assert_eq!(&profile[1..], &[[0.; 4]; 4]);
            }
        }
        let native = build_with_grid([0, 0], 60, 6, |x, _| {
            Ok(AppearanceColumn {
                height: 3 * x,
                colors: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            })
        })
        .unwrap();
        assert_eq!(native.sampled_columns, 208);
        assert_eq!(native.profiles[2][2], [3600., 7200., 0., 10_800.]);
    }

    #[test]
    fn grid_configuration_defaults_to_six_and_explicit_builder_is_bounded() {
        for input in [
            None,
            Some(""),
            Some("6"),
            Some("8"),
            Some(" 10"),
            Some("010"),
        ] {
            assert_eq!(parse_grid(input), 6);
        }
        assert_eq!(parse_grid(Some("10")), 10);
        for grid in [0, 1, 8, 11, usize::MAX] {
            assert!(build_with_grid([0, 0], 10, grid, |_, _| Ok(column(0, [1.; 3]))).is_err());
        }
        let selected = build_visible_appearance([0, 0], 1, |_, _| Ok(column(0, [1.; 3]))).unwrap();
        assert_eq!(selected.profiles[0][0][3], 1.);
    }
}
