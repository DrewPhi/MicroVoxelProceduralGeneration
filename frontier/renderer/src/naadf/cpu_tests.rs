use super::*;
use frontier_world::VOXEL_SIZE;

// Frozen pre-optimization Jacobi implementation: independent indexing oracle.
fn reference_expand(
    nodes: &[u32],
    dims: [usize; 3],
    strides: [usize; 3],
    i: usize,
    axis: usize,
    bits: u32,
) -> u32 {
    let old = nodes[i];
    if old & 0xc000_0000 != 0 {
        return old;
    }
    let mask = (1u32 << bits) - 1;
    let mut out = old;
    for positive in [false, true] {
        let side = axis * 2 + usize::from(positive);
        let shift = side as u32 * bits;
        let extent = (old >> shift) & mask;
        if extent == mask {
            continue;
        }
        let coordinate = (i / strides[axis]) % dims[axis];
        let gap = extent as usize + 1;
        let adjacent = if positive {
            coordinate.checked_add(gap).filter(|v| *v < dims[axis])
        } else {
            coordinate.checked_sub(gap)
        };
        let Some(adjacent) = adjacent else {
            continue;
        };
        let j = if positive {
            i + gap * strides[axis]
        } else {
            i - gap * strides[axis]
        };
        let neighbor = nodes[j];
        if neighbor & 0xc000_0000 != 0 {
            continue;
        }
        if (0..6)
            .filter(|s| s / 2 != axis)
            .any(|s| ((neighbor >> (s as u32 * bits)) & mask) < ((old >> (s as u32 * bits)) & mask))
        {
            continue;
        }
        let neighbor_extent = (neighbor >> shift) & mask;
        let available = if positive {
            dims[axis] - 1 - coordinate
        } else {
            coordinate
        };
        let value = (extent + 1 + neighbor_extent)
            .min(mask)
            .min(available as u32);
        let _ = adjacent;
        out = (out & !(mask << shift)) | (value << shift);
    }
    out
}
fn reference_bounds(
    nodes: &mut Vec<u32>,
    dims: [usize; 3],
    strides: [usize; 3],
    bits: u32,
    cancel: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut scratch = vec![0; nodes.len()];
    // Deterministic Jacobi updates; every intermediate cache is safe to use.
    for _ in 0..((1 << bits) - 1) {
        let mut changed = false;
        for axis in 0..3 {
            for i in 0..nodes.len() {
                if i % 32768 == 0 && cancel() {
                    return Err("NAADF build superseded".into());
                }
                scratch[i] = reference_expand(nodes, dims, strides, i, axis, bits);
                changed |= scratch[i] != nodes[i];
            }
            std::mem::swap(nodes, &mut scratch);
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

#[test]
fn coordinate_spans_match_original_jacobi_for_rectangular_layouts() {
    for dims in [
        [1, 1, 1],
        [1, 7, 3],
        [6, 1, 5],
        [9, 4, 1],
        [7, 5, 3],
        [33, 2, 2],
    ] {
        for strides in [
            [1, dims[0], dims[0] * dims[1]],
            [dims[1], 1, dims[0] * dims[1]],
        ] {
            for seed in 0..12_u64 {
                let mut rng = seed + 1;
                let nodes: Vec<u32> = (0..dims.iter().product())
                    .map(|_| {
                        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                        match rng >> 61 {
                            0 => FULL | 5,
                            1 => CHILD | 64,
                            _ => 0,
                        }
                    })
                    .collect();
                let mut expected = nodes.clone();
                let mut actual = nodes;
                reference_bounds(&mut expected, dims, strides, 5, &|| false).unwrap();
                bounds(&mut actual, dims, strides, 5, &|| false).unwrap();
                assert_eq!(actual, expected, "{dims:?}, {strides:?}, {seed}");
            }
        }
    }
}

#[test]
fn sparse_edits_preserve_canonical_cells_and_untouched_column_pigments() {
    for recipe in [
        frontier_world::FbmRecipe::Legacy,
        frontier_world::FbmRecipe::Alpine,
    ] {
        let base = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe,
                ..Default::default()
            },
        )
        .unwrap();
        let min = Cell {
            x: -48,
            y: (base.height_m(-2., -1.) * 16.) as i64 - 32,
            z: -32,
        };
        let dims = [3, 4, 2];
        let build = |world: &World| {
            build_region(
                world,
                min,
                dims,
                128 * 1048576,
                &AtomicU32::new(0),
                None,
                false,
                &|| false,
            )
            .unwrap()
        };
        let baseline = build(&base);
        let mut world = base.clone();
        // One edited column and edits across native chunk faces, plus a remote
        // edit that must not disable unrelated local column fast paths.
        for (dx, dy, dz, m) in [
            (0, 0, 0, Material::Air),
            (15, 15, 15, Material::Wood),
            (16, 16, 0, Material::Air),
            (17, 17, 1, Material::Stone),
            (16, 48, 1, Material::Wood),
            (10000, 1, 10000, Material::Stone),
        ] {
            world.set(
                Cell {
                    x: min.x + dx,
                    y: min.y + dy,
                    z: min.z + dz,
                },
                m,
            );
        }
        let edited = build(&world);
        assert!(edited.columns.iter().filter(|s| s.fast).count() >= 3);
        for z in 0..dims[2] * 16 {
            for x in 0..dims[0] * 16 {
                for y in 0..dims[1] * 16 {
                    let c = Cell {
                        x: min.x + x as i64,
                        y: min.y + y as i64,
                        z: min.z + z as i64,
                    };
                    let actual = edited.material([x, y, z]);
                    let pigment = &edited.palette[(actual & (FULL - 1)) as usize];
                    assert_eq!(pigment.material, id(world.material(c)), "{c:?}");
                    if x >= 32 {
                        let before = baseline.material([x, y, z]);
                        let before = &baseline.palette[(before & (FULL - 1)) as usize];
                        assert_eq!(
                            bytemuck::bytes_of(pigment),
                            bytemuck::bytes_of(before),
                            "{c:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "CPU-only reversed-order bounds microbenchmark"]
fn profile_root_coordinate_spans() {
    let n = std::hint::black_box(96);
    let dims = [n; 3];
    let strides = [n, 1, n * n];
    let seed: Vec<u32> = (0..n * n * n)
        .map(|i| {
            if i % n < 33 + (i / n % 13) {
                FULL | 5
            } else {
                0
            }
        })
        .collect();
    for optimized in [false, true] {
        let warmup = Instant::now();
        while warmup.elapsed().as_secs_f64() < 1. {
            let mut nodes = seed.clone();
            if optimized {
                bounds(&mut nodes, dims, strides, 5, &|| false).unwrap();
            } else {
                reference_bounds(&mut nodes, dims, strides, 5, &|| false).unwrap();
            }
            std::hint::black_box(nodes);
        }
    }
    for order in 0..4 {
        for optimized in if order % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let mut nodes = seed.clone();
            let start = Instant::now();
            if optimized {
                bounds(&mut nodes, dims, strides, 5, &|| false).unwrap();
            } else {
                reference_bounds(&mut nodes, dims, strides, 5, &|| false).unwrap();
            }
            eprintln!(
                "root coordinate spans optimized={optimized} ms={}",
                start.elapsed().as_secs_f64() * 1000.
            );
        }
    }
}

#[test]
fn compact_overlay_preserves_exact_snapshots_with_plants_negative_edges_and_edits() {
    for recipe in [
        frontier_world::FbmRecipe::Legacy,
        frontier_world::FbmRecipe::Alpine,
    ] {
        let mut world = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe,
                ..Default::default()
            },
        )
        .unwrap()
        .with_vegetation();
        let center = [-41., world.height_m(-41., -250.), -250.];
        let min = Cell {
            x: -43 * 16,
            y: ((center[1] * 16.) as i64).div_euclid(16) * 16 - 32,
            z: -252 * 16,
        };
        for edited in [false, true] {
            if edited {
                for (dx, dy, dz, material) in [
                    (0, 0, 0, Material::Air),
                    (15, 15, 15, Material::Wood),
                    (16, 16, 16, Material::Stone),
                    (63, 63, 63, Material::Wood),
                ] {
                    world.set(
                        Cell {
                            x: min.x + dx,
                            y: min.y + dy,
                            z: min.z + dz,
                        },
                        material,
                    );
                }
            }
            let build = |compact, chunk_local| {
                build_region_options(
                    &world,
                    min,
                    [4, 8, 4],
                    128 * 1048576,
                    &AtomicU32::new(0),
                    None,
                    false,
                    &|| false,
                    false,
                    false,
                    false,
                    compact,
                    chunk_local,
                )
                .unwrap()
            };
            let control = build(false, false);
            for (compact, chunk_local) in [(true, false), (false, true), (true, true)] {
                let candidate = build(compact, chunk_local);
                if recipe == frontier_world::FbmRecipe::Alpine {
                    assert!(
                        control.plant_cells > 0,
                        "Alpine fixture must exercise plant overlay"
                    );
                }
                assert_eq!(candidate.chunks, control.chunks);
                assert_eq!(candidate.blocks, control.blocks);
                assert_eq!(candidate.voxels, control.voxels);
                assert_eq!(
                    bytemuck::cast_slice::<Pigment, u8>(&candidate.palette),
                    bytemuck::cast_slice::<Pigment, u8>(&control.palette)
                );
                assert_eq!(candidate.plant_cells, control.plant_cells);
                assert_eq!(candidate.sampled_columns, control.sampled_columns);
            }
        }
    }
}

#[test]
#[ignore = "run separately with FRONTIER_NAADF_COLUMN_CACHE=0 and =1; verifies local edit cache gates"]
fn source_column_cache_reuses_distant_edits_and_bypasses_only_affected_tile() {
    let cache_enabled = super::super::column_cache::enabled();
    let mut world = World::fbm(893147923, frontier_world::FbmParams::default()).unwrap();
    let min = Cell {
        x: -64,
        y: ((world.height_m(-4., -4.) * 16.) as i64).div_euclid(16) * 16 - 16,
        z: -64,
    };
    let build = |world: &World| {
        build_region(
            world,
            min,
            [2; 3],
            128 * 1048576,
            &AtomicU32::new(0),
            None,
            false,
            &|| false,
        )
        .unwrap()
    };
    let cold = build(&world);
    assert_eq!(cold.sampled_columns, 1024);
    world.set(
        Cell {
            x: 900000,
            y: 10,
            z: -900000,
        },
        Material::Wood,
    );
    let distant = build(&world);
    assert_eq!(
        distant.sampled_columns,
        if cache_enabled { 0 } else { 1024 }
    );
    assert_eq!(distant.chunks, cold.chunks);
    assert_eq!(distant.blocks, cold.blocks);
    assert_eq!(distant.voxels, cold.voxels);
    assert_eq!(
        bytemuck::cast_slice::<Pigment, u8>(&distant.palette),
        bytemuck::cast_slice::<Pigment, u8>(&cold.palette)
    );
    world.set(
        Cell {
            x: min.x + 3,
            y: min.y + 20,
            z: min.z + 3,
        },
        Material::Wood,
    );
    let local = build(&world);
    assert_eq!(
        local.sampled_columns,
        if cache_enabled { 256 } else { 1024 }
    );
    assert!(!local.columns[0].fast);
    assert!(local.columns[1..].iter().all(|s| s.fast));
    for z in 0..32 {
        for x in 0..32 {
            for y in 0..32 {
                let cell = Cell {
                    x: min.x + x as i64,
                    y: min.y + y as i64,
                    z: min.z + z as i64,
                };
                let word = local.material([x, y, z]);
                assert_eq!(
                    local.palette[(word & (FULL - 1)) as usize].material,
                    id(world.material(cell)),
                    "{cell:?}"
                );
            }
        }
    }
}

#[test]
fn nonaligned_negative_column_edits_cannot_take_air_or_solid_summary_shortcuts() {
    for (x, z) in [(-31, -47), (-1, -17)] {
        let source = World::fbm(782397491, frontier_world::FbmParams::default()).unwrap();
        let mut low = i64::MAX;
        let mut high = i64::MIN;
        for dz in 0..32 {
            for dx in 0..32 {
                let h = (source.column(x + dx, z + dz).height_m() / VOXEL_SIZE) as i64;
                low = low.min(h);
                high = high.max(h);
            }
        }
        let summary = ColumnSummary {
            low,
            high,
            deep: id(Material::Stone),
            fast: true,
            owned: true,
        };
        for air in [true, false] {
            let min = Cell {
                x,
                y: if air { high + 64 } else { low - 96 },
                z,
            };
            let edited_cell = Cell {
                x: x + 15,
                y: min.y + 5,
                z: z + 15,
            };
            let mut world = source.clone();
            world.set(
                edited_cell,
                if air { Material::Wood } else { Material::Air },
            );
            let build = |cached| {
                build_region(
                    &world,
                    min,
                    [2; 3],
                    128 * 1048576,
                    &AtomicU32::new(0),
                    cached,
                    false,
                    &|| false,
                )
                .unwrap()
            };
            let control = build(None);
            let cached = |_, _| Some(summary);
            let actual = build(Some(&cached));
            assert!(
                !actual.columns[0].fast,
                "edit in a second global bin must disable its local column"
            );
            assert!(actual.columns[1..].iter().all(|column| column.fast));
            assert_eq!(actual.chunks, control.chunks);
            assert_eq!(actual.blocks, control.blocks);
            assert_eq!(actual.voxels, control.voxels);
            for dz in 0..32 {
                for dx in 0..32 {
                    for dy in 0..32 {
                        let c = Cell {
                            x: x + dx as i64,
                            y: min.y + dy as i64,
                            z: z + dz as i64,
                        };
                        let word = actual.material([dx, dy, dz]);
                        assert_eq!(
                            actual.palette[(word & (FULL - 1)) as usize].material,
                            id(world.material(c)),
                            "{c:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn nonaligned_topology_columns_match_canonical_at_ownership_and_feature_boundaries() {
    let world = World::fbm_topology(
        748192,
        frontier_world::FbmParams {
            amplitude: 64.,
            octaves: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let base = world.topology_base_cells().unwrap();
    // First crosses ownership from an unowned origin; the others cross the
    // excavation and cave boundaries and exercise actual volume materials.
    for (x, z) in [(-129, -129), (-81, -113), (-73, -81)] {
        let min = Cell { x, y: base - 48, z };
        let snapshot = build_region(
            &world,
            min,
            [1, 2, 1],
            128 * 1048576,
            &AtomicU32::new(0),
            None,
            false,
            &|| false,
        )
        .unwrap();
        assert!(snapshot.columns.iter().all(|column| !column.fast));
        for dz in 0..16 {
            for dx in 0..16 {
                for dy in 0..32 {
                    let cell = Cell {
                        x: x + dx as i64,
                        y: min.y + dy as i64,
                        z: z + dz as i64,
                    };
                    let word = snapshot.material([dx, dy, dz]);
                    assert_eq!(
                        snapshot.palette[(word & (FULL - 1)) as usize].material,
                        id(world.material(cell)),
                        "{cell:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn dirty_root_proof_5bit_cuboids_match_naive_point_intersection() {
    // Independent lattice-point oracle: the encoded counts include current root
    // plus each neighboring root, so both decoded endpoints are inclusive.
    let cp = [0_i32, 31, 511];
    for axis in 0..3 {
        for negative in 0..32_i32 {
            for positive in 0..32_i32 {
                let mut faces = [3_i32, 17, 31, 0, 9, 24];
                faces[2 * axis] = negative;
                faces[2 * axis + 1] = positive;
                let word = faces.iter().enumerate().fold(0_u32, |word, (face, count)| {
                    word | ((*count as u32) << (face * 5))
                });
                let lo: [i32; 3] =
                    std::array::from_fn(|a| cp[a] - ((word >> (10 * a)) & 31) as i32);
                let hi: [i32; 3] =
                    std::array::from_fn(|a| cp[a] + ((word >> (10 * a + 5)) & 31) as i32);
                for a in 0..3 {
                    assert_eq!(lo[a], cp[a] - faces[2 * a]);
                    assert_eq!(hi[a], cp[a] + faces[2 * a + 1]);
                }
                for delta in -34..=34 {
                    for width in [0, 1, 7, 31] {
                        let mut dirty_min = cp;
                        let mut dirty_max = cp;
                        dirty_min[axis] += delta;
                        dirty_max[axis] += delta + width;
                        let decoded =
                            (0..3).all(|a| hi[a] >= dirty_min[a] && lo[a] <= dirty_max[a]);
                        let naive = (-negative..=positive).any(|offset| {
                            let mut point = cp;
                            point[axis] += offset;
                            (0..3).all(|a| point[a] >= dirty_min[a] && point[a] <= dirty_max[a])
                        });
                        assert_eq!(
                            decoded, naive,
                            "axis={axis} counts={negative},{positive} delta={delta} width={width}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn edits_at_different_heights_keep_unaffected_chunks_in_the_same_column_fast() {
    for recipe in [
        frontier_world::FbmRecipe::Legacy,
        frontier_world::FbmRecipe::Alpine,
        frontier_world::FbmRecipe::SeededWorld,
    ] {
        let mut world = World::fbm(
            719,
            frontier_world::FbmParams {
                recipe,
                amplitude: 16.,
                horizontal_scale: 128.,
                octaves: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let x = -17;
        let z = -33;
        let ground = (world.ground_height_m(x as f64 * VOXEL_SIZE, z as f64 * VOXEL_SIZE)
            / VOXEL_SIZE) as i64;
        let min = Cell {
            x,
            y: ground.div_euclid(16) * 16 - 48,
            z,
        };
        let dims = [1, 10, 1];
        let build = |world: &World| {
            build_region(
                world,
                min,
                dims,
                128 * 1048576,
                &AtomicU32::new(0),
                None,
                false,
                &|| false,
            )
            .unwrap()
        };
        let before = build(&world);
        // Poison the old surface-band sample, remove underground terrain, and
        // add actual fluid above the source. Each occupies a distinct y root.
        world.set(
            Cell {
                x,
                y: ground - 1,
                z,
            },
            Material::Wood,
        );
        world.set(Cell { x, y: min.y + 1, z }, Material::Air);
        world.set_fluid_occupancy([(
            Cell {
                x,
                y: ground + 48,
                z,
            },
            true,
        )]);
        let after = build(&world);
        assert!(
            !after.columns[0].fast,
            "whole-column summaries remain edit-sensitive"
        );
        assert_eq!(after.edit_blocked_chunks, 3, "{recipe:?}");
        assert!(before.fast_skipped_chunks >= 5, "{recipe:?}");
        assert!(
            after.fast_skipped_chunks >= before.fast_skipped_chunks.saturating_sub(3),
            "{recipe:?}"
        );
        assert!(
            after.constructed_chunks <= before.constructed_chunks + 3,
            "{recipe:?}"
        );
        assert_eq!(after.fast_skipped_chunks + after.constructed_chunks, 10);
        for dz in 0..16 {
            for dx in 0..16 {
                for dy in 0..160 {
                    let c = Cell {
                        x: x + dx as i64,
                        y: min.y + dy as i64,
                        z: z + dz as i64,
                    };
                    let actual = after.material([dx, dy, dz]);
                    let pigment = &after.palette[(actual & (FULL - 1)) as usize];
                    assert_eq!(pigment.material, id(world.material(c)), "{recipe:?} {c:?}");
                    if dx != 0 || dz != 0 {
                        let original = before.material([dx, dy, dz]);
                        let original = &before.palette[(original & (FULL - 1)) as usize];
                        assert_eq!(
                            bytemuck::bytes_of(pigment),
                            bytemuck::bytes_of(original),
                            "{recipe:?} {c:?}"
                        );
                    }
                }
            }
        }
    }
}
