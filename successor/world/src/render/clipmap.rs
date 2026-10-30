//! Stable nested regular-grid bands. Tile identity is anchored in world space;
//! yaw never changes residency. The retained balanced cover supplies fallback.
use super::lod::{Grid, Layout, Quality, Selection, TerrainError, TileKey, SIDE};
use std::collections::BTreeMap;

pub fn select(
    eye: [f64; 3],
    height: u32,
    fov: f64,
    previous: &Layout,
    budget: usize,
    quality: Quality,
    mut terrain: impl FnMut(TileKey) -> Option<TerrainError>,
) -> Selection {
    let grid = previous.grid;
    let fov = if fov.is_finite() {
        fov.clamp(0.1, 3.)
    } else {
        1.1
    };
    let focal = height.max(1) as f64 / (2. * (fov * 0.5).tan());
    let requested = if quality.pixels.is_finite() {
        quality.pixels.clamp(1., 16.)
    } else {
        2.
    };
    let limit = budget.max(1);
    // Exact collision is separately queried. Reserve at most a quarter of the
    // tile budget for the native disk, including at millimeter cell sizes.
    let native_radius = if quality.native_radius.is_finite() {
        quality.native_radius.clamp(0., 32.)
    } else {
        6.
    }
    .min(grid.cell_m() * focal / requested)
    .min(grid.cell_m() * SIDE as f64 * (limit as f64 / (4. * std::f64::consts::PI)).sqrt());
    // cell_pixels = (tile_edge / SIDE) * focal / distance. A tighter image
    // target therefore expands each anchored regular-grid refinement band.
    let mut radius = focal / (SIDE as f64 * requested);
    let mut last = Layout::root_in(grid);
    let mut limited = false;
    let mut rejected_radius = None;
    let mut accepted_radius = None;
    let mut fitting_steps = 0;
    let mut bounds_cache = BTreeMap::new();
    for _ in 0..40 {
        let mut layout = Layout::root_in(grid);
        let mut pending = vec![*layout.tiles.first().unwrap()];
        let mut aborted = false;
        while let Some(key) = pending.pop() {
            if key.level >= grid.finest {
                continue;
            }
            let [x, z] = key.origin_in(grid);
            let edge = key.edge_in(grid) as f64 * grid.cell_m();
            let low = [x as f64 * grid.cell_m(), z as f64 * grid.cell_m()];
            let dx = (low[0] - eye[0]).max(eye[0] - low[0] - edge).max(0.);
            let dz = (low[1] - eye[2]).max(eye[2] - low[1] - edge).max(0.);
            let bounds = *bounds_cache.entry(key).or_insert_with(|| terrain(key));
            let dy = bounds.map_or(0., |e| (e.low - eye[1]).max(eye[1] - e.high).max(0.));
            let distance = dx.max(dz).max(dy);
            let center = [x + key.edge_in(grid) / 2, z + key.edge_in(grid) / 2];
            let retained = previous
                .at(center[0], center[1])
                .is_some_and(|p| p.level > key.level);
            let threshold = (edge * radius).max(native_radius) * if retained { 1.2 } else { 1. };
            // Feature refinement relaxes with the bands under budget pressure;
            // a sampled residual must not make every retry identically fail.
            let feature = bounds.is_some_and(|e| {
                distance < edge * radius * 1.5
                    && e.deviation * focal / distance.max(1.) > requested * 2.
            });
            if distance < threshold || feature {
                let children: Vec<_> = key
                    .children()
                    .filter(|c| {
                        let [x, z] = c.origin_in(grid);
                        x < grid.extent && z < grid.extent
                    })
                    .collect();
                if layout.tiles.len() + children.len() - 1 > limit {
                    aborted = true;
                    break;
                }
                layout.split(key);
                let mut children = children;
                children.sort_by(|a, b| {
                    distance_to(*b, grid, eye).total_cmp(&distance_to(*a, grid, eye))
                });
                pending.extend(children);
            }
        }
        if !aborted {
            layout.balance();
            if layout.tiles.len() <= limit {
                last = layout;
                let Some(high) = rejected_radius else {
                    break;
                };
                accepted_radius = Some(radius);
                fitting_steps += 1;
                if fitting_steps >= 7 {
                    break;
                }
                radius = (radius + high) * 0.5;
                continue;
            }
        }
        limited = true;
        rejected_radius = Some(radius);
        if let Some(low) = accepted_radius {
            fitting_steps += 1;
            if fitting_steps >= 7 {
                break;
            }
            radius = (low + radius) * 0.5;
        } else {
            radius *= 0.75;
        }
    }
    // P95 over visible leaf tiles, including native leaves. This is a sampled
    // projected-cell metric, not pixel weighting or a geometric error bound.
    // Missing vertical bounds conservatively place the terrain at eye height.
    let visibility = if quality.visibility.is_finite() {
        quality.visibility.max(0.)
    } else {
        24000.
    };
    let mut samples = Vec::new();
    for &key in &last.tiles {
        let bounds = *bounds_cache.entry(key).or_insert_with(|| terrain(key));
        if let Some(px) =
            visible_pixels(key, grid, eye, focal, quality.direction, visibility, bounds)
        {
            samples.push(px);
        }
    }
    samples.sort_by(f64::total_cmp);
    let achieved = if samples.is_empty() {
        0.
    } else {
        samples[((samples.len() - 1) as f64 * 0.95) as usize]
    };
    Selection {
        layout: last,
        requested_pixels: requested,
        achieved_pixels: achieved,
        budget_limited: limited,
        native_policy_radius: native_radius,
    }
}

fn visible_pixels(
    key: TileKey,
    grid: Grid,
    eye: [f64; 3],
    focal: f64,
    direction: [f64; 3],
    visibility: f64,
    bounds: Option<TerrainError>,
) -> Option<f64> {
    let [x, z] = key.origin_in(grid);
    let edge = key.edge_in(grid) as f64 * grid.cell_m();
    let low = [x as f64 * grid.cell_m(), z as f64 * grid.cell_m()];
    let dx = (low[0] - eye[0]).max(eye[0] - low[0] - edge).max(0.);
    let dz = (low[1] - eye[2]).max(eye[2] - low[1] - edge).max(0.);
    if dx.hypot(dz) > visibility {
        return None;
    }
    let dy = bounds.map_or(0., |b| (b.low - eye[1]).max(eye[1] - b.high).max(0.));
    let distance = dx.hypot(dz).hypot(dy).max(0.1);
    let center = [
        low[0] + edge * 0.5 - eye[0],
        bounds.map_or(eye[1], |b| (b.low + b.high) * 0.5) - eye[1],
        low[1] + edge * 0.5 - eye[2],
    ];
    let length = (center[0] * center[0] + center[1] * center[1] + center[2] * center[2])
        .sqrt()
        .max(0.1);
    let facing =
        (center[0] * direction[0] + center[1] * direction[1] + center[2] * direction[2]) / length;
    if facing + edge * 0.9 / length <= 0.5 {
        return None;
    }
    Some(key.step_in(grid) as f64 * grid.cell_m() * focal / distance)
}
fn distance_to(k: TileKey, g: Grid, eye: [f64; 3]) -> f64 {
    let [x, z] = k.origin_in(g);
    let e = k.edge_in(g) as f64 * g.cell_m();
    let dx = (x as f64 * g.cell_m() + e * 0.5 - eye[0]).abs();
    let dz = (z as f64 * g.cell_m() + e * 0.5 - eye[2]).abs();
    dx.max(dz)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flat(_: TileKey) -> Option<TerrainError> {
        Some(TerrainError {
            low: 0.,
            high: 0.,
            deviation: 0.,
            feature_scale: f64::INFINITY,
        })
    }
    #[test]
    fn yaw_does_not_rebuild_world_and_layout_is_balanced() {
        let root = Layout::root();
        let q = Quality::default();
        let a = select([5000., 100., 5000.], 720, 1.1, &root, 1024, q, flat);
        let b = select(
            [5000., 100., 5000.],
            720,
            1.1,
            &root,
            1024,
            Quality {
                direction: [1., 0., 0.],
                ..q
            },
            flat,
        );
        assert_eq!(a.layout, b.layout);
        assert!(a.layout.tiles.len() > 1 && a.layout.tiles.len() <= 1024);
        for &key in &a.layout.tiles {
            for level in a.layout.neighbors(key) {
                assert!(level.abs_diff(key.level) <= 1);
            }
        }
    }
    #[test]
    fn millimeter_native_radius_is_projected_and_budget_bounded() {
        let spec = crate::WorldSpec {
            grid: crate::GridScale::new(1, 1000).unwrap(),
            ..Default::default()
        };
        let root = Layout::root_in(Grid::new(spec));
        let result = select(
            [5000., 0.002, 5000.],
            1080,
            1.134,
            &root,
            1024,
            Quality::default(),
            flat,
        );
        assert!(result.native_policy_radius < 1.);
        assert!(result.layout.tiles.len() > 1 && result.layout.tiles.len() <= 1024);
    }
    #[test]
    fn tighter_quality_refines_and_improves_measured_p95_within_budget() {
        let root = Layout::root();
        let eye = [5000., 50., 5000.];
        let low = select(
            eye,
            720,
            1.1,
            &root,
            4096,
            Quality {
                pixels: 16.,
                ..Quality::default()
            },
            flat,
        );
        let high = select(
            eye,
            720,
            1.1,
            &root,
            4096,
            Quality {
                pixels: 2.,
                ..Quality::default()
            },
            flat,
        );
        eprintln!(
            "low tiles={} P95={} high tiles={} P95={}",
            low.layout.tiles.len(),
            low.achieved_pixels,
            high.layout.tiles.len(),
            high.achieved_pixels
        );
        assert!(!low.budget_limited);
        assert!(high.layout.tiles.len() <= 4096);
        assert!(high.layout.tiles.len() > low.layout.tiles.len() * 2);
        assert!(high.achieved_pixels < low.achieved_pixels * 0.5);
        assert_ne!(low.achieved_pixels, low.requested_pixels);
        assert_ne!(high.achieved_pixels, high.requested_pixels);
    }
    #[test]
    fn quality_steps_refine_monotonically_before_budget_saturation() {
        let root = Layout::root();
        let mut count = 0;
        let mut p95 = f64::INFINITY;
        for pixels in [16., 8., 4., 2.] {
            let result = select(
                [5000., 100., 5000.],
                720,
                1.1,
                &root,
                16384,
                Quality {
                    pixels,
                    ..Quality::default()
                },
                flat,
            );
            assert!(!result.budget_limited);
            assert!(result.layout.tiles.len() > count);
            assert!(result.achieved_pixels < p95);
            count = result.layout.tiles.len();
            p95 = result.achieved_pixels;
        }
    }
    #[test]
    fn one_leaf_metric_matches_analytic_projection_and_terrain_height() {
        let root = Layout::root();
        let center = root.grid.root as f64 * root.grid.cell_m() * 0.5;
        let q = Quality {
            direction: [0., -1., 0.],
            native_radius: 0.,
            ..Quality::default()
        };
        let result = select([center, 1000., center], 720, 1.1, &root, 1, q, flat);
        let focal = 720. / (2. * (1.1f64 * 0.5).tan());
        let root_cell = root.tiles.first().unwrap().step_in(root.grid) as f64 * root.grid.cell_m();
        assert_eq!(result.achieved_pixels, root_cell * focal / 1000.);
        let raised = select([center, 1000., center], 720, 1.1, &root, 1, q, |_| {
            Some(TerrainError {
                low: 500.,
                high: 500.,
                deviation: 0.,
                feature_scale: f64::INFINITY,
            })
        });
        assert_eq!(raised.achieved_pixels, result.achieved_pixels * 2.);
    }
    #[test]
    fn reporting_matches_independent_projected_leaf_percentile() {
        let root = Layout::root();
        let eye = [5000., 100., 5000.];
        let q = Quality::default();
        let result = select(eye, 720, 1.1, &root, 64, q, flat);
        let focal = 720. / (2. * (1.1f64 * 0.5).tan());
        let mut expected: Vec<_> = result
            .layout
            .tiles
            .iter()
            .filter_map(|&key| {
                visible_pixels(
                    key,
                    root.grid,
                    eye,
                    focal,
                    q.direction,
                    q.visibility,
                    flat(key),
                )
            })
            .collect();
        expected.sort_by(f64::total_cmp);
        assert_eq!(
            result.achieved_pixels,
            expected[((expected.len() - 1) as f64 * 0.95) as usize]
        );
        assert!(result.budget_limited);
        assert!(result.achieved_pixels > result.requested_pixels);
        assert!(result.layout.tiles.len() <= 64);
    }
    #[test]
    fn caller_budget_and_empty_visible_sample_set_are_honored() {
        let root = Layout::root();
        let result = select(
            [-1000., 100., -1000.],
            720,
            1.1,
            &root,
            1,
            Quality {
                visibility: 0.,
                ..Quality::default()
            },
            flat,
        );
        assert_eq!(result.layout.tiles.len(), 1);
        assert_eq!(result.achieved_pixels, 0.);
    }
}
