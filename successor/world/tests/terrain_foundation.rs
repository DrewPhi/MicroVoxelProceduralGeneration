use microvoxel::{
    generation::{FlatTerrain, Generator, ProceduralTerrain},
    physics::VoxelShape,
    render::lod,
    CellPos, Edit, GridScale, Voxel, World, WorldSpec,
};

#[test]
fn world_scale_is_exact_checked_and_does_not_reinterpret_saves() {
    assert_eq!("0.0625".parse::<GridScale>().unwrap(), GridScale::default());
    for bad in ["0", "NaN", "-1", "1/0", "0.0001"] {
        assert!(bad.parse::<GridScale>().is_err());
    }
    #[derive(Clone, Copy)]
    struct Scaled {
        grid: GridScale,
    }
    impl Generator for Scaled {
        fn spec(&self) -> WorldSpec {
            WorldSpec {
                grid: self.grid,
                ..Default::default()
            }
        }
        fn identity(&self) -> microvoxel::generation::GeneratorIdentity {
            FlatTerrain {
                height: 10,
                water: 10,
                seed: 42,
            }
            .identity()
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            FlatTerrain {
                height: 10,
                water: 10,
                seed: 42,
            }
            .voxel(p)
        }
    }
    let mut world = World::new(Scaled {
        grid: GridScale::new(1, 100).unwrap(),
    });
    world
        .apply(&[Edit::set(CellPos::new(0, 5, 0), Voxel::AIR)])
        .unwrap();
    let path = std::env::temp_dir().join(format!("microvoxel-grid-{}.mvw", std::process::id()));
    world.save_edits(&path).unwrap();
    let mut same = World::new(Scaled {
        grid: world.spec().grid,
    });
    same.load_edits(&path).unwrap();
    assert_eq!(
        world.voxel_at(CellPos::new(0, 5, 0)),
        same.voxel_at(CellPos::new(0, 5, 0))
    );
    let mut wrong = World::new(Scaled {
        grid: GridScale::default(),
    });
    assert!(wrong.load_edits(&path).is_err());
    std::fs::remove_file(path).unwrap();
    // A format-1 record is understood as the old default grid, provided its
    // generator/material identity still matches. New recipes are never migrated.
    let default = World::new(FlatTerrain {
        height: 10,
        water: 10,
        seed: 42,
    });
    let legacy =
        std::env::temp_dir().join(format!("microvoxel-legacy-grid-{}.mvw", std::process::id()));
    default.save_edits(&legacy).unwrap();
    let mut bytes = std::fs::read(&legacy).unwrap();
    bytes.drain(28..44);
    bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&16u32.to_le_bytes());
    std::fs::write(&legacy, bytes).unwrap();
    let mut restored = World::new(FlatTerrain {
        height: 10,
        water: 10,
        seed: 42,
    });
    restored.load_edits(&legacy).unwrap();
    std::fs::remove_file(legacy).unwrap();
    for denominator in [4, 16, 100, 1000] {
        let grid = GridScale::new(1, denominator).unwrap();
        let spec = WorldSpec {
            grid,
            ..Default::default()
        };
        let p = grid.position([128., 40., 64.]).unwrap();
        assert_eq!(grid.point(p), [128., 40., 64.]);
        assert_eq!(spec.page(p), [2, 1]);
        let space = lod::Grid::new(spec);
        assert!(space.root >= spec.extent_cells());
        let fine = lod::TileKey {
            level: space.finest,
            x: 0,
            z: 0,
        };
        assert_eq!(fine.step_in(space), 1);
        let selection = lod::select_quality(
            [100., 50., 100.],
            540,
            65f64.to_radians(),
            &lod::Layout::root_in(space),
            512,
            lod::Quality::default(),
        );
        assert!(
            selection.layout.tiles.len() <= 576,
            "tile work exploded at 1/{denominator}"
        );
    }
}

#[test]
fn column_masks_match_cell_collision_including_edits_and_cavities() {
    let mut world = World::new(FlatTerrain {
        height: 20,
        water: 20,
        seed: 42,
    });
    for y in 12..18 {
        world
            .apply(&[Edit::set(CellPos::new(1, y, 1), Voxel::AIR)])
            .unwrap();
    }
    let cells: Vec<_> = (0..40)
        .filter(|i| i % 3 != 0)
        .map(|i| [i % 3, i / 7, i % 5])
        .collect();
    let shape = VoxelShape::new(cells.clone());
    for y in 10..30 {
        for x in -2..4 {
            let root = CellPos::new(x, y, 0);
            let naive = cells.iter().all(|p| {
                world
                    .voxel_at(root.offset(p.map(i64::from)).unwrap())
                    .known()
                    .is_some_and(|v| v.is_air())
            });
            assert_eq!(shape.clear(&world, root), naive);
        }
    }
}

#[test]
fn physical_terrain_features_survive_grid_changes_and_sparse_edits() {
    let terrain = ProceduralTerrain::new(42);
    for d in [4, 16, 100, 1000] {
        let grid = GridScale::new(1, d).unwrap();
        let scaled = terrain.at_grid(grid);
        for p in [
            [4250., 6250.],
            [9316., 5037.],
            [1250., 8750.],
            [128.25, 300.5],
        ] {
            let a = terrain.sample_m(p[0], p[1]).unwrap();
            let b = scaled.sample_m(p[0], p[1]).unwrap();
            assert_eq!(a.height_m, b.height_m);
            assert_eq!(a.water_m, b.water_m);
            assert_eq!(a.material, b.material);
            let c = scaled
                .column_hint(grid.cells(p[0]).unwrap(), grid.cells(p[1]).unwrap())
                .unwrap();
            assert!((c.ground as f64 * grid.meters() - a.height_m).abs() <= grid.meters() + 1e-8);
        }
    }
    for boundary in [64., 128., 256.] {
        let a = terrain.sample_m(4250. + boundary - 1e-6, 6250.).unwrap();
        let b = terrain.sample_m(4250. + boundary + 1e-6, 6250.).unwrap();
        assert!(
            (a.height_m - b.height_m).abs() < 0.001,
            "boundary {boundary}: {} versus {}",
            a.height_m,
            b.height_m
        );
    }
    // Geographic provinces need not fit inside the initial 10 km crop.
    let terrain = ProceduralTerrain::with_spec(
        42,
        Default::default(),
        WorldSpec {
            extent_m: 100_000,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !terrain.geological_features().is_empty(),
        "seed 42 needs a geological capability fixture"
    );
    let feature = terrain.geological_features()[0];
    let arch = terrain
        .geological_features()
        .iter()
        .find(|f| f.kind == microvoxel::generation::features::FeatureKind::Arch)
        .expect("arch capability fixture");
    let roof = terrain
        .spec()
        .grid
        .position([
            arch.center_m[0],
            arch.center_m[1] + arch.radius_m * 1.5,
            arch.center_m[2],
        ])
        .unwrap();
    assert_eq!(
        terrain.voxel(roof).unwrap().material,
        microvoxel::MaterialId::SANDSTONE
    );
    assert!(
        terrain.spec().grid.point(roof)[1]
            > terrain
                .sample_m(arch.center_m[0], arch.center_m[2])
                .unwrap()
                .height_m
    );
    let [low, high] = feature.bounds();
    let origin = [low[0], low[2]];
    let edge = (high[0] - low[0]).max(high[2] - low[2]);
    assert!(terrain.has_display_volumetric_features(origin, edge, 0.25));
    assert!(!terrain.has_display_volumetric_features(origin, edge, 128.));
    assert!(terrain.has_volumetric_features(origin, edge));
    let center = feature.center_m;
    let summary = terrain
        .summarize_m([center[0] - 16., center[2] - 16.], 32.)
        .unwrap();
    assert_ne!(summary.features & microvoxel::generation::summary::VOID, 0);
    assert_eq!(
        summary.bounds_kind,
        microvoxel::generation::summary::BoundsKind::SampledWithResidual
    );
    let p = terrain.spec().grid.position(center).unwrap();
    let mut world = World::new(terrain);
    assert!(world.voxel_at(p).known().unwrap().is_air());
    let runs = world.column_runs(p.x, p.z, p.y - 1..p.y + 2).unwrap();
    assert!(runs
        .iter()
        .all(|r| r.material == microvoxel::MaterialId::AIR));
    world
        .apply(&[Edit::set(
            p,
            Voxel::new(
                microvoxel::MaterialId::ROCK,
                microvoxel::materials::Appearance(777),
            ),
        )])
        .unwrap();
    assert!(!world.voxel_at(p).known().unwrap().is_air());
    let changed = world
        .terrain_summary([center[0] - 1., center[2] - 1.], 2.)
        .unwrap();
    assert!(changed.feature_scale_m <= 0.0625);
}
