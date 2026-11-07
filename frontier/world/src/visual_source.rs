//! B1 bounded CPU source experiment, deliberately disconnected from display selection.
//! Bounds concern sampled heights only, not unsampled ridges, materials, normals,
//! silhouettes or grid interpolation. They are ideal arithmetic bounds plus native
//! quantization; floating point enclosure is NOT certified. No render quality pass
//! follows from producing a page. Native queries and volume ownership are unchanged.
use crate::{FbmParams, FbmRecipe, FbmTerrain, World, VOXEL_SIZE};

#[derive(Clone, Copy, Debug)]
pub struct VisualRequest {
    /// Global native column address; never a page-relative noise origin.
    pub min: [i64; 2],
    /// Number of samples including both endpoints. z-major output.
    pub samples: [u32; 2],
    pub spacing_cells: u32,
    pub source_revision: u32,
    pub edit_revision: u64,
    pub retained_octaves: u32,
    pub max_samples: u64,
}

#[derive(Debug)]
pub struct VisualPage {
    pub request: VisualRequest,
    pub seed: u64,
    pub params: FbmParams,
    pub heights_m: Vec<f64>,
    pub ideal_sample_error_bound_m: f64,
    /// Actual scalar noise calls executed for elevation, excluding lookups and
    /// materials (this height-only experiment does not evaluate materials).
    pub elevation_noise_evaluations: u64,
    pub exact_fallback: bool,
}

/// Ideal arithmetic bound on omitted Legacy bands. Each ridge expression has
/// absolute value <=1; the full denominator stays fixed. Flooring two heights
/// contributes at most one native voxel. Zero omitted amplitude is exact.
pub fn ideal_omitted_bound_m(terrain: &FbmTerrain, retained: u32) -> f64 {
    let p = terrain.params();
    if p.recipe != FbmRecipe::Legacy || retained >= p.octaves {
        return 0.0;
    }
    let (mut norm, mut omitted, mut weight) = (0.0, 0.0, 1.0);
    for octave in 0..p.octaves {
        norm += weight;
        if octave >= retained {
            omitted += weight;
        }
        weight *= p.roughness;
    }
    let bound = p.amplitude * 0.85 * omitted / norm;
    if bound == 0.0 {
        0.0
    } else {
        bound + VOXEL_SIZE
    }
}

/// Refuses topology/edit ownership anywhere in the sampled rectangle including
/// a native-cell halo. Caller must retain an exact/volume path on rejection.
/// Revision checking is synchronous against this immutable world snapshot.
/// Regional uses its exact canonical generator: no unproven warp/material sieve.
pub fn generate_visual(world: &World, request: VisualRequest) -> Result<VisualPage, &'static str> {
    let terrain = world
        .fbm_terrain()
        .ok_or("fixture requires volume representation")?;
    if request.source_revision != terrain.source_version()
        || request.edit_revision != world.revision()
    {
        return Err("stale source or edit revision");
    }
    let p = terrain.params();
    if request.retained_octaves == 0 || request.retained_octaves > p.octaves {
        return Err("invalid retained octave count");
    }
    let count = u64::from(request.samples[0])
        .checked_mul(u64::from(request.samples[1]))
        .ok_or("sample count overflow")?;
    if count == 0 || count > request.max_samples || request.spacing_cells == 0 {
        return Err("invalid or over-budget sample grid");
    }
    let mut end = request.min;
    for (axis, value) in end.iter_mut().enumerate() {
        *value = value
            .checked_add(
                i64::from(request.samples[axis] - 1)
                    .checked_mul(i64::from(request.spacing_cells))
                    .ok_or("coordinate overflow")?,
            )
            .ok_or("coordinate overflow")?;
    }
    let lo = [
        request.min[0].checked_sub(1).ok_or("halo overflow")?,
        request.min[1].checked_sub(1).ok_or("halo overflow")?,
    ];
    let hi = [
        end[0].checked_add(1).ok_or("halo overflow")?,
        end[1].checked_add(1).ok_or("halo overflow")?,
    ];
    // Preserve reliable f64 native-column positioning, as required by FbmTerrain.
    if lo
        .into_iter()
        .chain(hi)
        .any(|c| c.unsigned_abs() >= (1_u64 << 52))
    {
        return Err("outside reliable native-center coordinates");
    }
    if world.topology_base_cells().is_some()
        && lo[0] < 128
        && hi[0] >= -128
        && lo[1] < 128
        && hi[1] >= -128
    {
        return Err("volume-owned region");
    }
    if world
        .edit_cells()
        .any(|c| c.x >= lo[0] && c.x <= hi[0] && c.z >= lo[1] && c.z <= hi[1])
    {
        return Err("edited region requires exact ownership");
    }
    let retained = if p.recipe == FbmRecipe::Legacy {
        request.retained_octaves
    } else {
        p.octaves
    };
    let per_sample = match p.recipe {
        FbmRecipe::Legacy => 3 + retained,
        FbmRecipe::Regional => 11 + p.octaves,
        FbmRecipe::RegionalUnwarped => 9 + p.octaves,
    };
    let mut heights_m =
        Vec::with_capacity(usize::try_from(count).map_err(|_| "allocation overflow")?);
    for z in 0..request.samples[1] {
        for x in 0..request.samples[0] {
            let cell = [
                request.min[0] + i64::from(x) * i64::from(request.spacing_cells),
                request.min[1] + i64::from(z) * i64::from(request.spacing_cells),
            ];
            heights_m.push(if p.recipe == FbmRecipe::Legacy {
                terrain.visual_legacy_height(cell, retained)
            } else {
                world.column(cell[0], cell[1]).height_m()
            });
        }
    }
    Ok(VisualPage {
        request,
        seed: terrain.seed(),
        params: p,
        heights_m,
        ideal_sample_error_bound_m: ideal_omitted_bound_m(terrain, retained),
        elevation_noise_evaluations: count * u64::from(per_sample),
        exact_fallback: retained == p.octaves,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, Material};
    fn request(world: &World) -> VisualRequest {
        VisualRequest {
            min: [-1024, -512],
            samples: [17, 9],
            spacing_cells: 16,
            source_revision: world.fbm_terrain().unwrap().source_version(),
            edit_revision: world.revision(),
            retained_octaves: 2,
            max_samples: 153,
        }
    }
    #[test]
    fn omitted_enclosure_and_exact_fallback_across_recipes_parameters_and_seeds() {
        for recipe in [
            FbmRecipe::Legacy,
            FbmRecipe::Regional,
            FbmRecipe::RegionalUnwarped,
        ] {
            for seed in [7, 113, 997] {
                for roughness in [0.0, 0.5, 1.0] {
                    let world = World::fbm(
                        seed,
                        FbmParams {
                            recipe,
                            roughness,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    for retained in [1, 2, 4, 8] {
                        let r = VisualRequest {
                            retained_octaves: retained,
                            ..request(&world)
                        };
                        let page = generate_visual(&world, r).unwrap();
                        for (i, &h) in page.heights_m.iter().enumerate() {
                            let x = r.min[0] + (i % 17) as i64 * 16;
                            let z = r.min[1] + (i / 17) as i64 * 16;
                            let exact = world.column(x, z).height_m();
                            assert!((h - exact).abs() <= page.ideal_sample_error_bound_m + 1e-7);
                            if page.exact_fallback {
                                assert_eq!(h, exact);
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn shared_anchors_negative_coordinates_and_source_identity() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let r = VisualRequest {
            min: [-128, -64],
            ..request(&world)
        };
        let fine = generate_visual(&world, r).unwrap();
        let coarse = generate_visual(
            &world,
            VisualRequest {
                samples: [9, 5],
                spacing_cells: 32,
                ..r
            },
        )
        .unwrap();
        for z in 0..5 {
            for x in 0..9 {
                assert_eq!(
                    coarse.heights_m[z * 9 + x],
                    fine.heights_m[z * 2 * 17 + x * 2]
                );
            }
        }
        let adjacent = generate_visual(
            &world,
            VisualRequest {
                min: [128, -64],
                samples: [3, 5],
                spacing_cells: 32,
                ..r
            },
        )
        .unwrap();
        for z in 0..5 {
            assert_eq!(adjacent.heights_m[z * 3], fine.heights_m[z * 2 * 17 + 16]);
        }
        assert_eq!(fine.seed, 7);
        assert_eq!(fine.params, world.fbm_terrain().unwrap().params());
        assert_eq!(fine.elevation_noise_evaluations, 153 * 5);
        assert!(generate_visual(
            &world,
            VisualRequest {
                source_revision: r.source_revision + 1,
                ..r
            }
        )
        .is_err());
    }
    #[test]
    fn edits_topology_limits_and_revisions_never_silently_disappear() {
        let mut world = World::fbm_topology(7, FbmParams::default()).unwrap();
        let r = request(&world);
        assert!(generate_visual(&world, VisualRequest { min: [-1, -1], ..r }).is_err());
        let cell = Cell {
            x: r.min[0] - 1,
            y: 20000,
            z: r.min[1],
        };
        world.set(cell, Material::Wood);
        assert!(generate_visual(&world, r).is_err());
        assert!(generate_visual(
            &world,
            VisualRequest {
                edit_revision: world.revision(),
                ..r
            }
        )
        .is_err());
        assert_eq!(world.material(cell), Material::Wood);
        let world = World::fbm(7, FbmParams::default()).unwrap();
        for bad in [
            VisualRequest {
                max_samples: 1,
                ..r
            },
            VisualRequest {
                min: [i64::MAX, 0],
                ..r
            },
            VisualRequest {
                spacing_cells: 0,
                ..r
            },
            VisualRequest {
                samples: [0, 1],
                ..r
            },
        ] {
            assert!(generate_visual(&world, bad).is_err());
        }
    }
}
