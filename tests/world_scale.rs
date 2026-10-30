//! Landscape compression is independent of grid resolution and presentation LOD.
use microvoxel::{
    generation::{Generator, ProceduralTerrain},
    CellPos, Edit, GridScale, Voxel, World,
};
use std::sync::OnceLock;
fn source() -> &'static ProceduralTerrain {
    static SOURCE: OnceLock<ProceduralTerrain> = OnceLock::new();
    SOURCE.get_or_init(|| ProceduralTerrain::new(42))
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-7, "{a} != {b}");
}
#[test]
fn compression_preserves_the_landscape_climate_and_grid_independently() {
    let source = source();
    for scale in [1, 2, 10, 100] {
        let world = source.at_world_scale(scale).unwrap();
        assert_eq!(world.spec().grid, source.spec().grid);
        assert_eq!(world.spec().extent_m, 10000 / scale);
        assert_eq!(
            world.regional_memory_bytes(),
            source.regional_memory_bytes()
        );
        for [x, z] in [
            [4250., 6250.],
            [1728., 8896.],
            [2624., 9536.],
            [128.25, 300.5],
        ] {
            let a = source.sample_m(x, z).unwrap();
            let b = world.sample_m(x / scale as f64, z / scale as f64).unwrap();
            near(a.height_m, b.height_m * scale as f64);
            near(a.water_m, b.water_m * scale as f64);
            near(a.bedrock_m, b.bedrock_m * scale as f64);
            assert_eq!(a.material, b.material);
            assert_eq!(a.environment.temperature_c, b.environment.temperature_c);
            assert_eq!(a.environment.moisture, b.environment.moisture);
            assert!(
                (a.environment.soil_depth_m - b.environment.soil_depth_m * scale as f32).abs()
                    < 1e-4
            );
            for grid in [GridScale::default(), GridScale::new(1, 100).unwrap()] {
                let finer = world.at_grid(grid);
                near(
                    finer
                        .sample_m(x / scale as f64, z / scale as f64)
                        .unwrap()
                        .height_m,
                    b.height_m,
                );
                assert_eq!(finer.world_scale(), scale);
            }
        }
    }
    for scale in [0, 101, u32::MAX] {
        assert!(source.at_world_scale(scale).is_err());
    }
    let a = source.at_world_scale(3).unwrap().at_world_scale(1).unwrap();
    assert_eq!(a.spec(), source.spec()); // Rounded extents must not compound.
    assert_eq!(a.identity(), source.identity());
}
#[test]
fn compressed_queries_summaries_and_geological_features_use_game_coordinates() {
    let source = source();
    let world = source.at_world_scale(10).unwrap();
    let grid = world.spec().grid;
    for [x, z] in [[425., 625.], [172.8, 889.6], [262.4, 953.6]] {
        let px = grid.cells(x).unwrap();
        let pz = grid.cells(z).unwrap();
        let p = grid.point(CellPos::new(px, 0, pz));
        let s = world.sample_m(p[0], p[2]).unwrap();
        let c = world.column_hint(px, pz).unwrap();
        assert!((c.ground as f64 * grid.meters() - s.height_m).abs() <= grid.meters());
        let e = world.environment_at(CellPos::new(px, 0, pz)).unwrap();
        let original = source
            .environment_at(CellPos::new(px * 10, 0, pz * 10))
            .unwrap();
        assert!((e.elevation_m * 10. - original.elevation_m).abs() < 1e-3);
        let a = source.summarize_m([p[0] * 10., p[2] * 10.], 40.).unwrap();
        let b = world.summarize_m([p[0], p[2]], 4.).unwrap();
        near(a.min_m, b.min_m * 10.);
        near(a.max_m, b.max_m * 10.);
        near(a.error_m, b.error_m * 10.);
    }
    for (a, b) in source
        .geological_features()
        .iter()
        .zip(world.geological_features())
    {
        for i in 0..3 {
            near(a.center_m[i], b.center_m[i] * 10.);
        }
        near(a.radius_m, b.radius_m * 10.);
        near(a.length_m, b.length_m * 10.);
        assert!(world.has_volumetric_features([b.center_m[0] - 1., b.center_m[2] - 1.], 2.));
    }
}
#[test]
fn compressed_water_footprints_are_congruent_and_scale_is_part_of_save_identity() {
    let source = source();
    let compressed = source.at_world_scale(10).unwrap();
    // Scan bounded regional metadata, not native voxels, for a real channel.
    let mut found = false;
    'rows: for z in (128..9872).step_by(64) {
        for x in (128..9872).step_by(64) {
            let origin = [x as f64, z as f64];
            if let Some(a) = source.water_footprint(origin, 32.) {
                let b = compressed
                    .water_footprint(origin.map(|v| v / 10.), 3.2)
                    .unwrap();
                near(a.point[0], b.point[0] * 10.);
                near(a.point[1], b.point[1] * 10.);
                near(a.width_m, b.width_m * 10.);
                near(a.coverage, b.coverage);
                assert_eq!(a.id, b.id);
                found = true;
                break 'rows;
            }
        }
    }
    assert!(found);
    let mut world = World::new(compressed);
    world
        .apply(&[Edit::set(CellPos::new(100, 1, 100), Voxel::AIR)])
        .unwrap();
    let path = std::env::temp_dir().join(format!("voxel-world-scale-{}.mvw", std::process::id()));
    world.save_edits(&path).unwrap();
    let mut same = World::new(source.at_world_scale(10).unwrap());
    same.load_edits(&path).unwrap();
    let mut different = World::new(source.at_world_scale(1).unwrap());
    assert!(different.load_edits(&path).is_err());
    std::fs::remove_file(path).unwrap();
}
