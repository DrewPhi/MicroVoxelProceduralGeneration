//! Independent consumer tests: use only public canonical queries and bound data.
use frontier_world::{
    affine_bounds::{bound_native_height, BoundBudget, ColumnBox},
    FbmParams, FbmRecipe, FbmTerrain, VOXEL_SIZE,
};

#[test]
fn held_out_worlds_parameters_and_closed_voxel_faces_stay_enclosed() {
    let mut rng = 0x2c81_4f92_11ab_793du64;
    let mut next = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        rng
    };
    for recipe in [
        FbmRecipe::Legacy,
        FbmRecipe::Regional,
        FbmRecipe::RegionalUnwarped,
    ] {
        for parameters in [
            FbmParams::default(),
            FbmParams {
                horizontal_scale: 16.,
                octaves: 16,
                roughness: 1.,
                ..Default::default()
            },
            FbmParams {
                horizontal_scale: 10_000_000.,
                amplitude: 100_000.,
                octaves: 1,
                roughness: 0.,
                ..Default::default()
            },
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        ] {
            for _ in 0..16 {
                let terrain = FbmTerrain::new(
                    next(),
                    FbmParams {
                        recipe,
                        ..parameters
                    },
                )
                .unwrap();
                let min = [0, 1].map(|_| (next() % 2_000_000) as i64 - 1_000_000);
                let cells = 1u32 << (next() % 9);
                let bound = bound_native_height(
                    &terrain,
                    ColumnBox {
                        min,
                        size_cells: [cells; 2],
                    },
                    BoundBudget::default(),
                )
                .unwrap();
                for _ in 0..16 {
                    let col = [0, 1].map(|a| min[a] + (next() % cells as u64) as i64);
                    let h = terrain.height_m(
                        (col[0] as f64 + 0.5) * VOXEL_SIZE,
                        (col[1] as f64 + 0.5) * VOXEL_SIZE,
                    );
                    assert!(h >= bound.height_range[0] && h <= bound.height_range[1]);
                    // A native top extends to all four closed corners, not just
                    // its sampled center. The same bound must contain each one.
                    for dx in [0., 1.] {
                        for dz in [0., 1.] {
                            let p = [
                                (col[0] as f64 + dx) * VOXEL_SIZE,
                                (col[1] as f64 + dz) * VOXEL_SIZE,
                            ];
                            let predicted = bound.center_height_m
                                + (0..2)
                                    .map(|a| {
                                        bound.coefficients[a] * (p[a] - bound.center_xz[a])
                                            / bound.half_extent_m[a]
                                    })
                                    .sum::<f64>();
                            assert!((h - predicted).abs() <= bound.remainder_m,
                                "recipe={recipe:?} params={parameters:?} column={col:?} bound={bound:?} h={h} affine={predicted}");
                        }
                    }
                }
            }
        }
    }
}
