//! Camera-independent certificates for canonical terrain. No sampled estimate is a proof.
use crate::{FbmRecipe, FbmTerrain, Material, World, VOXEL_SIZE};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeometryError {
    Certified(f64),
    Uncertified,
}
impl GeometryError {
    pub fn meters(self) -> Option<f64> {
        match self {
            Self::Certified(v) if v.is_finite() && v >= 0.0 => Some(v),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopologyClass {
    Heightfield,
    Volume,
    Unknown,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppearanceSummary {
    /// Exposed canonical face area (m²), indexed Air/Grass/Soil/Stone/Wood.
    pub material_area_m2: [f64; 5],
    /// Exposed canonical face area indexed +X/-X/+Y/-Y/+Z/-Z.
    pub normal_area_m2: [f64; 6],
}
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainSummary {
    pub min_height_m: f64,
    pub max_height_m: f64,
    pub topology: TopologyClass,
    pub revision: u64,
    pub error: GeometryError,
    pub appearance: AppearanceSummary,
}
impl TerrainSummary {
    /// Union children for visibility bounds and appearance. Parent proxy error
    /// must be recomputed: child errors do not certify a different parent mesh.
    pub fn merge(children: &[Self]) -> Option<Self> {
        let first = children.first()?;
        if children.iter().any(|v| v.revision != first.revision) {
            return None;
        }
        let mut result = first.clone();
        result.error = GeometryError::Uncertified;
        result.appearance = AppearanceSummary::default();
        for c in children {
            result.min_height_m = result.min_height_m.min(c.min_height_m);
            result.max_height_m = result.max_height_m.max(c.max_height_m);
            result.topology = match (result.topology, c.topology) {
                (TopologyClass::Volume, _) | (_, TopologyClass::Volume) => TopologyClass::Volume,
                (TopologyClass::Unknown, _) | (_, TopologyClass::Unknown) => TopologyClass::Unknown,
                _ => TopologyClass::Heightfield,
            };
            for i in 0..5 {
                result.appearance.material_area_m2[i] += c.appearance.material_area_m2[i];
            }
            for i in 0..6 {
                result.appearance.normal_area_m2[i] += c.appearance.normal_area_m2[i];
            }
        }
        Some(result)
    }
}

/// Finite global elevation envelope without visiting any native columns.
/// Interval arithmetic uses |noise|<=1 and quantization floor in [-voxel,0].
/// Bounds apply to the unedited source only, never arbitrary volume owners.
pub fn fbm_height_envelope(terrain: &FbmTerrain) -> [f64; 2] {
    let p = terrain.params();
    match p.recipe {
        FbmRecipe::Legacy => [-0.7325 * p.amplitude - VOXEL_SIZE, 1.13 * p.amplitude],
        _ => [
            -103.5 * p.amplitude / 1000.0 - VOXEL_SIZE,
            423.5 * p.amplitude / 1000.0,
        ],
    }
}

/// Analytic global Euclidean Lipschitz bound of the continuous generator.
/// Gradient noise: unit gradients, quintic fade derivative <= 15/8,
/// corner gradient difference <= 2 sqrt(2), output multiplier 1.6.
/// Each partial <= 1.6(1 + (15/8)2sqrt(2)); Euclidean norm < 16.
/// Clamping/absolute value preserve these bounds; warp uses the Jacobian norm.
pub fn conservative_fbm_slope(terrain: &FbmTerrain) -> f64 {
    let p = terrain.params();
    let weighted_frequency = |start: f64, count: u32, gain: f64| {
        let (mut sum, mut norm, mut weight, mut f) = (0.0, 0.0, 1.0, start);
        for _ in 0..count {
            sum += weight * f;
            norm += weight;
            weight *= gain;
            f *= 2.0;
        }
        sum / norm
    };
    let n = 16.0;
    match p.recipe {
        FbmRecipe::Legacy => {
            let warp = 1.0 + 2.0_f64.sqrt() * 0.32 * 0.55 * n;
            p.amplitude / p.horizontal_scale
                * warp
                * n
                * (0.85 * 2.8 * weighted_frequency(1.0, p.octaves, p.roughness) + 0.4 * 0.42)
        }
        _ => {
            let warp = if p.recipe == FbmRecipe::Regional {
                1.0 + 2.0_f64.sqrt() * (100.0 / 1800.0) * 0.4 * n
            } else {
                1.0
            };
            let gain = 0.8 * p.roughness;
            let mountain_slope = 5.0 * n * (0.3 + 0.35 * 0.6) / 1.35;
            // |noise| <= 1, |(1-sqrt(noise²+.04))²| <= 1,
            // derivative of the latter <= 2. All product factors bounded.
            let macro_slope = 90.0 * n * weighted_frequency(1800.0 / 2500.0, 3, gain);
            let ridges =
                320.0 * (mountain_slope + 2.0 * n * weighted_frequency(1800.0 / 1400.0, 3, gain));
            let foothills = 12.0 * (0.7 * mountain_slope + n * 6.0);
            let detail =
                1.5 * (0.8 * mountain_slope + n * weighted_frequency(45.0, p.octaves, gain));
            p.amplitude / 1000.0 / p.horizontal_scale
                * warp
                * (macro_slope + ridges + foothills + detail)
        }
    }
}

/// Real-arithmetic model estimate. It does not by itself certify the
/// floating-point canonical generator; use `conservative_grid_error_at`.
pub fn ideal_grid_error_m(terrain: &FbmTerrain, step_cells: u32) -> f64 {
    conservative_fbm_slope(terrain) * 2.0_f64.sqrt() * step_cells as f64 * VOXEL_SIZE
        + (1.0 + 2.0_f64.sqrt()) * VOXEL_SIZE
}

/// Region-qualified certificate for triangles with horizontal diameter at most
/// sqrt(2)*step_cells*voxel_size, sampled from the same unedited FBM source.
/// Native face extraction has zero error separately; edits/skirts are excluded.
///
/// The floating-point proof domain is deliberately bounded: all native sample
/// addresses including a one-column halo must lie within +/-2^32 cells. Cell
/// centers are then exactly representable before source scaling. CPU source
/// evaluation uses Rust's ordinary f64 arithmetic (no fast-math/reassociation).
/// The source's validated parameter ranges and <=16 octave limit are required.
///
/// Conservative absolute roundoff propagation, in units eps*(1+Q): gradient
/// noise local arithmetic <=2^8; scaled/warped input <=2^10; applying its
/// Lipschitz bound16 and local arithmetic <=2^15; normalized octave sums <=2^16;
/// ridge/province/product composition <=2^20 times amplitude. We reserve2^24,
/// including final elevation arithmetic. Q bounds every noise input magnitude.
/// Clamps are nonexpansive; positive octave normalization is >=1. The floor
/// quantization is covered by the voxel allowance in the ideal-model bound.
///
/// Only regions whose roundoff reserve <=voxel/4 are certified, with twice
/// the reserve added for the two compared source evaluations. Outside this
/// domain return Uncertified: callers refine or retain explicitly unresolved
/// coverage. Exhaustive native certificates remain independent of this bound.
pub fn conservative_grid_error_at(
    terrain: &FbmTerrain,
    min: [i64; 2],
    width_cells: u32,
    step_cells: u32,
) -> GeometryError {
    if width_cells == 0 || step_cells == 0 || step_cells > width_cells {
        return GeometryError::Uncertified;
    }
    let address_bound = min
        .into_iter()
        .map(|v| {
            (v as i128 - 1)
                .abs()
                .max((v as i128 + width_cells as i128 + 1).abs())
        })
        .max()
        .unwrap();
    if address_bound > (1_i128 << 32) {
        return GeometryError::Uncertified;
    }
    let p = terrain.params();
    if p.validate().is_err() {
        return GeometryError::Uncertified;
    }
    let max_frequency = 2.0_f64.powi(p.octaves as i32 - 1)
        * if p.recipe == FbmRecipe::Legacy {
            1.0
        } else {
            45.0
        };
    let normalized = (address_bound as f64 + 0.5) * VOXEL_SIZE / p.horizontal_scale;
    let max_input = (normalized + 0.5) * max_frequency + 64.0;
    if !max_input.is_finite() || max_input > (1_u64 << 40) as f64 {
        return GeometryError::Uncertified;
    }
    let roundoff = (1_u64 << 24) as f64 * f64::EPSILON * p.amplitude * (1.0 + max_input);
    if !roundoff.is_finite() || roundoff > VOXEL_SIZE * 0.25 {
        return GeometryError::Uncertified;
    }
    GeometryError::Certified(ideal_grid_error_m(terrain, step_cells) + 2.0 * roundoff)
}

/// Bounded exhaustive native-column summary, with a one-column neighbor halo.
/// Returns None before work if the caller's work budget is exceeded. The source
/// must be pure FBM with no intersecting volume/edit ownership. Unsupported
/// topology is explicit, never silently replaced by an unedited heightfield.
pub fn summarize_native_columns(
    world: &World,
    min: [i64; 2],
    size: [u32; 2],
    max_columns: u64,
) -> Option<TerrainSummary> {
    let count = u64::from(size[0]).checked_mul(u64::from(size[1]))?;
    if count == 0 || count > max_columns {
        return None;
    }
    let end = [
        min[0].checked_add(i64::from(size[0]))?,
        min[1].checked_add(i64::from(size[1]))?,
    ];
    // Halo addresses must remain representable too.
    min[0].checked_sub(1)?;
    min[1].checked_sub(1)?;
    end[0].checked_add(1)?;
    end[1].checked_add(1)?;
    let intersects =
        |x: i64, z: i64| x >= min[0] - 1 && x <= end[0] && z >= min[1] - 1 && z <= end[1];
    let topology = if world.fbm_terrain().is_none() {
        TopologyClass::Unknown
    } else if (world.topology_base_cells().is_some()
        && min[0] < 128
        && end[0] > -128
        && min[1] < 128
        && end[1] > -128)
        || world.edit_cells().any(|c| intersects(c.x, c.z))
    {
        TopologyClass::Volume
    } else {
        TopologyClass::Heightfield
    };
    let mut s = TerrainSummary {
        min_height_m: f64::INFINITY,
        max_height_m: f64::NEG_INFINITY,
        topology,
        revision: world.revision(),
        error: GeometryError::Uncertified,
        appearance: AppearanceSummary::default(),
    };
    for z in min[1]..end[1] {
        for x in min[0]..end[0] {
            let c = world.column(x, z);
            let h = c.height_m();
            s.min_height_m = s.min_height_m.min(h);
            s.max_height_m = s.max_height_m.max(h);
            if topology != TopologyClass::Heightfield {
                continue;
            }
            let m = c.material((h / VOXEL_SIZE) as i64 - 1);
            add_face(&mut s.appearance, m, 2, VOXEL_SIZE * VOXEL_SIZE);
            for (dx, dz, axis) in [(1, 0, 0), (-1, 0, 1), (0, 1, 4), (0, -1, 5)] {
                let neighbor = world.column(x + dx, z + dz).height_m();
                // Each exposed side voxel can have a different depth material.
                let bottom = (neighbor / VOXEL_SIZE) as i64;
                let top = (h / VOXEL_SIZE) as i64;
                // Pure FBM has exactly three depth runs; count their areas without
                // work proportional to cliff height (or the world's vertical range).
                if bottom < top {
                    let face = VOXEL_SIZE * VOXEL_SIZE;
                    add_face(&mut s.appearance, m, axis, face);
                    let soil = (top - 1 - bottom.max(top - 12)).max(0);
                    let stone = (top - 12 - bottom).max(0);
                    add_face(&mut s.appearance, Material::Soil, axis, soil as f64 * face);
                    add_face(
                        &mut s.appearance,
                        Material::Stone,
                        axis,
                        stone as f64 * face,
                    );
                }
            }
        }
    }
    // Base-height samples cannot bound arbitrary edited/volumetric surfaces.
    // Infinite bounds explicitly prevent using this incomplete summary to cull.
    if topology != TopologyClass::Heightfield {
        s.min_height_m = f64::NEG_INFINITY;
        s.max_height_m = f64::INFINITY;
    }
    Some(s)
}
fn add_face(a: &mut AppearanceSummary, m: Material, axis: usize, area: f64) {
    let i = match m {
        Material::Air => 0,
        Material::Grass => 1,
        Material::Soil => 2,
        Material::Stone => 3,
        Material::Wood => 4,
    };
    a.material_area_m2[i] += area;
    a.normal_area_m2[axis] += area;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, FbmParams};
    #[test]
    fn bounded_summary_merges_and_rejects_edits() {
        let mut w = World::fbm(7, FbmParams::default()).unwrap();
        assert!(summarize_native_columns(&w, [0, 0], [4, 4], 15).is_none());
        let a = summarize_native_columns(&w, [0, 0], [2, 2], 4).unwrap();
        let b = summarize_native_columns(&w, [2, 0], [2, 2], 4).unwrap();
        let merged = TerrainSummary::merge(&[a.clone(), b.clone()]).unwrap();
        let all = summarize_native_columns(&w, [0, 0], [4, 2], 8).unwrap();
        assert_eq!(merged.min_height_m, all.min_height_m);
        assert_eq!(merged.max_height_m, all.max_height_m);
        assert_eq!(merged.appearance, all.appearance);
        assert_eq!(merged.error, GeometryError::Uncertified);
        w.set(
            Cell {
                x: 0,
                y: 9000,
                z: 0,
            },
            Material::Wood,
        );
        assert_eq!(
            summarize_native_columns(&w, [0, 0], [4, 2], 8)
                .unwrap()
                .topology,
            TopologyClass::Volume
        );
        assert!(TerrainSummary::merge(&[
            a,
            summarize_native_columns(&w, [0, 0], [1, 1], 1).unwrap()
        ])
        .is_none());
    }
    #[test]
    fn procedural_certificate_rejects_unproved_numeric_domains() {
        let t = FbmTerrain::new(7, FbmParams::default()).unwrap();
        assert!(conservative_grid_error_at(&t, [0, 0], 8192, 1024)
            .meters()
            .is_some());
        assert!(conservative_grid_error_at(&t, [1_i64 << 33, 0], 128, 16)
            .meters()
            .is_none());
        assert!(conservative_grid_error_at(&t, [i64::MAX - 1, 0], 128, 16)
            .meters()
            .is_none());
        let extreme = FbmTerrain::new(
            7,
            FbmParams {
                recipe: FbmRecipe::Regional,
                horizontal_scale: 16.0,
                amplitude: 100000.0,
                octaves: 16,
                roughness: 1.0,
            },
        )
        .unwrap();
        assert!(conservative_grid_error_at(&extreme, [0, 0], 512, 64)
            .meters()
            .is_none());
    }
    #[test]
    fn source_envelope_contains_all_recipes_without_native_generation() {
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
        ] {
            for seed in [0, 7, 113] {
                let t = FbmTerrain::new(
                    seed,
                    FbmParams {
                        recipe,
                        ..Default::default()
                    },
                )
                .unwrap();
                let bounds = fbm_height_envelope(&t);
                for x in -20..20 {
                    let h = t.height_m(x as f64 * 617.0, x as f64 * -313.0);
                    assert!(h >= bounds[0] && h <= bounds[1]);
                }
            }
        }
    }
    #[test]
    fn analytic_bound_covers_native_neighbor_variation() {
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
        ] {
            let t = FbmTerrain::new(
                7,
                FbmParams {
                    recipe,
                    ..Default::default()
                },
            )
            .unwrap();
            let slope = conservative_fbm_slope(&t);
            for x in -30..30 {
                let p = x as f64 * 173.0;
                assert!((t.height_m(p + 1.0, p) - t.height_m(p, p)).abs() <= slope + VOXEL_SIZE);
            }
            assert!(
                conservative_grid_error_at(&t, [0, 0], 32, 32)
                    .meters()
                    .unwrap()
                    > conservative_grid_error_at(&t, [0, 0], 32, 1)
                        .meters()
                        .unwrap()
            );
        }
    }
}
