//! Independent certificates checked against canonical native columns, including
//! water-visible tops, descriptor boundaries and held-out source parameters.
use crate::{
    affine_bounds::{
        bound_native_height, bound_proxy_residual, BoundBudget, ColumnBox, ProxyPlane,
    },
    FbmParams, FbmRecipe, FbmTerrain, VOXEL_SIZE,
};

fn params() -> FbmParams {
    FbmParams {
        recipe: FbmRecipe::SeededWorld,
        ..Default::default()
    }
}
fn verify(terrain: &FbmTerrain, region: ColumnBox, budget: BoundBudget) -> usize {
    let bound = bound_native_height(terrain, region, budget).expect("bounded native address");
    let center = bound.center_xz;
    // A nonhorizontal independent proxy catches lost coordinate correlation as
    // well as incorrect visible-height intervals.
    let plane = ProxyPlane {
        height_at_center: terrain.height_m(center[0], center[1]),
        dx: 0.017,
        dz: -0.023,
    };
    let residual = bound_proxy_residual(terrain, region, plane, budget).unwrap();
    let axis = |n: u32| -> Vec<u32> {
        if n <= 17 {
            (0..n).collect()
        } else {
            (0..=16).map(|i| ((n - 1) as u64 * i / 16) as u32).collect()
        }
    };
    let mut wet = 0;
    for iz in axis(region.size_cells[1]) {
        for ix in axis(region.size_cells[0]) {
            let x = region.min[0] + ix as i64;
            let z = region.min[1] + iz as i64;
            let column = terrain.column_cells(x, z);
            wet += usize::from(column.visible_top > column.ground_top);
            let actual = column.visible_top as f64 * VOXEL_SIZE;
            assert!(actual >= bound.height_range[0] && actual <= bound.height_range[1],
                "seed={} params={:?} region={region:?} column=({x},{z}) visible={actual} bound={bound:?}",
                terrain.seed(), terrain.params());
            let position = [(x as f64 + 0.5) * VOXEL_SIZE, (z as f64 + 0.5) * VOXEL_SIZE];
            let proxy = plane.height_at_center
                + plane.dx * (position[0] - center[0])
                + plane.dz * (position[1] - center[1]);
            assert!(
                (actual - proxy).abs() <= residual.error_m,
                "seed={} region={region:?} visible={actual} proxy={proxy} residual={residual:?}",
                terrain.seed()
            );
        }
    }
    wet
}

#[test]
fn seeded_certificates_cover_river_cross_sections_lakes_and_negative_cache_corners() {
    let mut tested_lakes = 0;
    let mut wet = 0;
    for seed in [997, 0xcafe_1234] {
        let terrain = FbmTerrain::new(seed, params()).unwrap();
        let mut anchors = vec![
            [-9600., -9600.],
            [-9600.0625, 0.],
            [-0.0625, -0.0625],
            [0., 0.],
            [9600., -9600.],
        ];
        let mut river_count = 0;
        let mut lake_count = 0;
        'regions: for rz in [-1, 0, 1] {
            for rx in [-1, 0, 1] {
                let center = [rx as f64 * 9600., rz as f64 * 9600.];
                let reaches =
                    terrain.seeded_reaches(center.map(|v| v - 3600.), center.map(|v| v + 3600.));
                for r in reaches {
                    if r.a.stage < 0. || r.b.stage < 0. {
                        continue;
                    }
                    if r.lake {
                        if lake_count >= 1 {
                            continue;
                        }
                        lake_count += 1;
                        for factor in [0., 0.8, 0.99, 1., 1.01, 2.99] {
                            anchors.push([r.a.x + factor * r.width, r.a.z]);
                        }
                        for distance in [
                            r.outer_radius() - VOXEL_SIZE,
                            r.outer_radius(),
                            r.outer_radius() + VOXEL_SIZE,
                        ] {
                            anchors.push([r.a.x + distance, r.a.z]);
                        }
                    } else {
                        if river_count >= 2 {
                            continue;
                        }
                        river_count += 1;
                        let dx = r.b.x - r.a.x;
                        let dz = r.b.z - r.a.z;
                        let length = dx.hypot(dz);
                        for factor in [0., 0.8, 0.99, 1.01, 2.99] {
                            anchors.push([
                                (r.a.x + r.b.x) * 0.5 - dz / length * factor * r.width,
                                (r.a.z + r.b.z) * 0.5 + dx / length * factor * r.width,
                            ]);
                        }
                        for distance in [
                            r.outer_radius() - VOXEL_SIZE,
                            r.outer_radius(),
                            r.outer_radius() + VOXEL_SIZE,
                        ] {
                            anchors.push([
                                (r.a.x + r.b.x) * 0.5 - dz / length * distance,
                                (r.a.z + r.b.z) * 0.5 + dx / length * distance,
                            ]);
                        }
                        anchors.push([r.a.x, r.a.z]);
                    }
                }
                if lake_count > 0 && river_count >= 2 {
                    break 'regions;
                }
            }
        }
        // Descriptor search is finite and fixed in physical coordinates; the
        // fixture must actually exercise the feature classes named by this test.
        assert!(
            river_count >= 2 && lake_count > 0,
            "seed={seed} river={river_count} lake={lake_count}"
        );
        tested_lakes += lake_count;
        for anchor in anchors {
            let c = anchor.map(|v| (v / VOXEL_SIZE).floor() as i64);
            for cells in [1, 9, 257] {
                let region = ColumnBox {
                    min: c.map(|v| v - (cells / 2) as i64),
                    size_cells: [cells; 2],
                };
                wet += verify(&terrain, region, BoundBudget::default());
            }
        }
    }
    assert!(
        tested_lakes >= 2 && wet > 100,
        "lakes={tested_lakes} wet={wet}"
    );
}

#[test]
fn seeded_certificates_survive_extreme_parameters_and_budget_fallbacks() {
    for p in [
        FbmParams {
            amplitude: 0.,
            ..params()
        },
        FbmParams {
            amplitude: 100000.,
            horizontal_scale: 16.,
            octaves: 16,
            roughness: 1.,
            ..params()
        },
        FbmParams {
            amplitude: 0.125,
            horizontal_scale: 16.,
            octaves: 1,
            roughness: 0.,
            ..params()
        },
        FbmParams {
            amplitude: 3700.,
            horizontal_scale: 10_000_000.,
            octaves: 16,
            roughness: 1.,
            ..params()
        },
    ] {
        let terrain = FbmTerrain::new(0x7234_abcd, p).unwrap();
        let spacing_cells = (2400. * p.horizontal_scale / 1800. / VOXEL_SIZE).floor() as i64;
        for min in [
            [-31, -1],
            [0, 0],
            [-4 * spacing_cells - 1, 4 * spacing_cells - 1],
        ] {
            for size_cells in [[11, 7], [513, 257]] {
                for budget in [
                    BoundBudget::default(),
                    BoundBudget {
                        max_noise_cells: 0,
                        max_cells_per_noise: 1,
                    },
                ] {
                    verify(&terrain, ColumnBox { min, size_cells }, budget);
                }
            }
        }
    }
}

#[test]
fn watershed_certificates_cover_banks_and_region_edges() {
    let mut wet = 0;
    for seed in [19, 997, 40961] {
        let terrain = FbmTerrain::new(
            seed,
            FbmParams {
                recipe: FbmRecipe::Watershed,
                ..params()
            },
        )
        .unwrap();
        let mut anchors = vec![[0., 0.], [-9600., 0.], [0., 9600.]];
        for r in terrain
            .seeded_reaches([-4500.; 2], [4500.; 2])
            .iter()
            .step_by(13)
        {
            let mid = [(r.a.x + r.b.x) * 0.5, (r.a.z + r.b.z) * 0.5];
            anchors.extend([mid, [mid[0] + r.width, mid[1]]]);
        }
        for m in anchors {
            let c = m.map(|v| (v / VOXEL_SIZE).floor() as i64);
            wet += verify(
                &terrain,
                ColumnBox {
                    min: c.map(|v| v - 64),
                    size_cells: [129; 2],
                },
                BoundBudget::default(),
            );
        }
    }
    assert!(wet > 100, "wet samples {wet}");
}

#[test]
fn watershed_water_stays_close_to_the_bed_at_a_low_saddle() {
    let terrain = FbmTerrain::new(
        997,
        FbmParams {
            recipe: FbmRecipe::Watershed,
            ..params()
        },
    )
    .unwrap();
    // This reach originally extrapolated a stage over 380 m above its bed.
    let x = (-3802.083333333333_f64 / VOXEL_SIZE).floor() as i64;
    let z = (2135.416666666667_f64 / VOXEL_SIZE).floor() as i64;
    let column = terrain.column_cells(x, z);
    assert!(
        column.visible_top > column.ground_top,
        "the channel should still be wet"
    );
    assert!((column.visible_top - column.ground_top) as f64 * VOXEL_SIZE <= 9. + VOXEL_SIZE);
    verify(
        &terrain,
        ColumnBox {
            min: [x - 32, z - 32],
            size_cells: [65; 2],
        },
        BoundBudget::default(),
    );
}

#[test]
fn seeded_huge_region_global_fallback_still_contains_sea_and_land() {
    for seed in [997, 0x8215_11] {
        let terrain = FbmTerrain::new(seed, params()).unwrap();
        verify(
            &terrain,
            ColumnBox {
                min: [-200000, -160000],
                size_cells: [400001, 320001],
            },
            BoundBudget {
                max_noise_cells: 16,
                max_cells_per_noise: 2,
            },
        );
    }
}

#[test]
fn seeded_distant_columns_and_negative_chunk_borders_reproduce_after_eviction() {
    use crate::{Cell, World};
    let world = World::fbm(19, params()).unwrap();
    let mut cells = Vec::new();
    for center in [-1_000_000., -9600., 0., 9600., 1_000_000.] {
        let c = Cell::from_meters([center, 0., -center]);
        for offset in [-1, 0, 1] {
            cells.push((c.x + offset, c.z - offset));
        }
    }
    let sample = |x, z| {
        let column = world.column(x, z);
        let ground = (column.ground_height_m() / VOXEL_SIZE) as i64;
        let top = (column.height_m() / VOXEL_SIZE) as i64;
        let y = [top + 1, top - 1, ground - 1, ground - 2, ground - 13];
        for &y in &y {
            assert_eq!(column.material(y), world.material(Cell { x, y, z }));
        }
        (ground, top, y.map(|y| column.material(y)))
    };
    let expected: Vec<_> = cells.iter().map(|&(x, z)| sample(x, z)).collect();
    // These widely separated points exceed the thread-local graph cache size.
    for (&(x, z), expected) in cells.iter().zip(expected).rev() {
        assert_eq!(sample(x, z), expected);
    }
}

#[test]
fn seeded_lake_certificate_excludes_hidden_bed_depth() {
    let t = FbmTerrain::new(997, params()).unwrap();
    for rz in -4..=4 {
        for rx in -4..=4 {
            let center = [rx as f64 * 9600., rz as f64 * 9600.];
            for reach in t.seeded_reaches(center.map(|v| v - 2000.), center.map(|v| v + 2000.)) {
                if !reach.lake || reach.a.stage <= 0. {
                    continue;
                }
                let c = crate::Cell::from_meters([reach.a.x - 0.5, 0., reach.a.z - 0.5]);
                let region = ColumnBox {
                    min: [c.x, c.z],
                    size_cells: [16, 16],
                };
                let bound = bound_native_height(&t, region, BoundBudget::default()).unwrap();
                assert!(
                    bound.height_range[1] - bound.height_range[0] < VOXEL_SIZE * 2.,
                    "flat water must not inherit its hidden bed's error: {bound:?}"
                );
                verify(&t, region, BoundBudget::default());
                return;
            }
        }
    }
    panic!("test survey must contain an above-sea sink lake");
}

#[test]
fn seeded_valley_support_lookup_and_half_node_halo_cache_corners_are_certified() {
    for seed in [19, 113] {
        let terrain = FbmTerrain::new(seed, params()).unwrap();
        let reaches = terrain.seeded_reaches([-3600., -3600.], [3600., 3600.]);
        let r = *reaches
            .iter()
            .find(|r| !r.lake && r.a.stage > 0.)
            .expect("wet river fixture");
        let dx = r.b.x - r.a.x;
        let dz = r.b.z - r.a.z;
        let length = dx.hypot(dz);
        for side in [-1., 1.] {
            for delta in [-VOXEL_SIZE, 0., VOXEL_SIZE] {
                let distance = r.outer_radius() + delta;
                let x = (r.a.x + r.b.x) * 0.5 - dz / length * distance * side;
                let z = (r.a.z + r.b.z) * 0.5 + dx / length * distance * side;
                if delta < 0. {
                    assert!(
                        terrain
                            .seeded_reaches([x, z], [x, z])
                            .iter()
                            .any(|q| q.owner == r.owner && q.segment == r.segment),
                        "finite support lookup omitted an active valley segment at {x},{z}"
                    );
                }
                let c = crate::Cell::from_meters([x, 0., z]);
                for cells in [1, 17, 513] {
                    verify(
                        &terrain,
                        ColumnBox {
                            min: [c.x - cells as i64 / 2, c.z - cells as i64 / 2],
                            size_cells: [cells; 2],
                        },
                        BoundBudget::default(),
                    );
                }
            }
        }
        // These boxes straddle each corner of a cache region and extend half a
        // drainage-node spacing on all sides. Their source comes from multiple
        // independent region caches, including negative-coordinate halos.
        for center in [
            [-9600., -9600.],
            [-9600., 0.],
            [0., 0.],
            [9600., 0.],
            [0., 9600.],
            [9600., 9600.],
        ] {
            let c = crate::Cell::from_meters([center[0] - 1200., 0., center[1] - 1200.]);
            verify(
                &terrain,
                ColumnBox {
                    min: [c.x, c.z],
                    size_cells: [38401; 2],
                },
                BoundBudget::default(),
            );
        }
    }
}

#[test]
fn seeded_flat_ocean_has_no_artificial_quantization_error() {
    let t = FbmTerrain::new(7, params()).unwrap();
    for z in [-4800., -2400., 0., 2400., 4800.] {
        for x in [-4800., -2400., 0., 2400., 4800.] {
            if t.ground_height_m(x, z) > -16. {
                continue;
            }
            let min = [x - 1., z - 1.];
            let max = [x + 1., z + 1.];
            if t.seeded_reaches(min, max)
                .iter()
                .any(|r| r.a.stage > 0. || r.b.stage > 0.)
            {
                continue;
            }
            let c = crate::Cell::from_meters([min[0], 0., min[1]]);
            let region = ColumnBox {
                min: [c.x, c.z],
                size_cells: [32, 32],
            };
            let bound = bound_native_height(&t, region, BoundBudget::default()).unwrap();
            assert!(
                bound.height_range[1] - bound.height_range[0] < 1e-10,
                "{bound:?}"
            );
            assert_eq!(bound.center_height_m, 0.);
            assert_eq!(bound.remainder_m, 0.);
            verify(&t, region, BoundBudget::default());
            return;
        }
    }
    panic!("survey must contain a simple below-sea region");
}

#[test]
fn normalized_channel_weights_keep_motion_offender_bounds_useful() {
    let t = FbmTerrain::new(19, params()).unwrap();
    // These source-v8 route tiles previously reported 51.22, 41.15 and 15.86 m
    // affine error because independently normalized weights lost correlation.
    for min in [[-17920, 3072], [-17408, 2560], [-14848, 4096]] {
        let mut worst = 0_f64;
        for z in (min[1]..min[1] + 512).step_by(64) {
            for x in (min[0]..min[0] + 512).step_by(64) {
                let region = ColumnBox {
                    min: [x, z],
                    size_cells: [64; 2],
                };
                let bound = bound_native_height(&t, region, BoundBudget::default()).unwrap();
                let point = |x: i64, z: i64| {
                    [
                        x as f64 * VOXEL_SIZE,
                        t.column_cells(x, z).visible_top as f64 * VOXEL_SIZE,
                        z as f64 * VOXEL_SIZE,
                    ]
                };
                let center = point(x + 32, z + 32);
                let corners = [
                    point(x, z),
                    point(x, z + 64),
                    point(x + 64, z + 64),
                    point(x + 64, z),
                ];
                for i in 0..4 {
                    worst = worst.max(
                        bound
                            .triangle_error([center, corners[i], corners[(i + 1) % 4]])
                            .unwrap(),
                    );
                }
                verify(&t, region, BoundBudget::default());
            }
        }
        assert!(
            worst < 6.,
            "route region {min:?} lost useful weighted normalization: {worst}"
        );
    }
}
