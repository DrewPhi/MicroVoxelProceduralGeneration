//! End-to-end generation contracts; all regional builds are shared in this test.
use microvoxel::{
    generation::{
        debug::{image_from_world, DebugLayer},
        ClimatePreset, Generator, ProceduralTerrain, TerrainConfig,
    },
    materials::{MaterialId, Voxel},
    CellPos, World,
};

#[test]
fn regional_environment_drives_diversity_and_preserves_world_contracts() {
    let generator = ProceduralTerrain::new(42);
    let stats = generator.environment_statistics();
    // A 10 km crop may occupy one geographic province; diversity belongs at
    // regional scale, not an artificial requirement to pack every type nearby.
    assert!(stats.landforms.iter().sum::<usize>() == stats.samples);
    assert!(stats.geology.iter().sum::<usize>() == stats.samples);
    assert!(stats.gentle_fraction > 0.20);
    assert!(generator.regional_memory_bytes() < 64 * 1024 * 1024);
    let p = CellPos::new(85000, 0, 80000);
    let column = generator.column_hint(p.x, p.z).unwrap();
    let mut world = World::new(generator);
    let e = world.environment_at(p).unwrap();
    assert_eq!(Some(e), world.environment_at(CellPos::new(p.x, 99999, p.z)));
    assert!(world.environment_at(CellPos::new(160000, 0, 0)).is_none());
    assert!(world.environment_at(CellPos::new(-1, 0, 0)).is_none());
    for x in (100..160000).step_by(7011) {
        let sample = world.environment_at(CellPos::new(x, 0, x / 2)).unwrap();
        assert!((sample.landform_weights.iter().sum::<f32>() - 1.).abs() < 1e-5);
        assert!((0. ..=1.).contains(&sample.moisture));
        assert!((0. ..=1.).contains(&sample.ground_wetness));
        assert!(sample.soil_depth_m >= 0. && sample.rainfall_mm_year.is_finite());
        assert!(
            sample.cold_season_c <= sample.temperature_c
                && sample.temperature_c <= sample.warm_season_c
        );
    }
    let surface = CellPos::new(p.x, column.ground - 1, p.z);
    let voxel = world.voxel_at(surface).known().unwrap();
    assert_eq!(
        voxel.appearance,
        world.appearance_at(surface, voxel.material)
    );
    assert_eq!(
        world
            .voxel_at(CellPos::new(p.x, 0, p.z))
            .known()
            .unwrap()
            .material,
        column.rock
    );
    assert!(column.rock.is_bedrock());
    for layer in DebugLayer::ALL {
        let image = image_from_world(&world, layer, 16).unwrap();
        assert_eq!(image.rgb.len(), 16 * 16 * 3);
        assert_eq!(world.voxel_at(surface).known(), Some(voxel));
    }
    assert!(image_from_world(&world, DebugLayer::Biomes, usize::MAX).is_err());
    // Long-term regional environment is an input, not mutable live weather.
    world
        .apply(&[microvoxel::world::Edit::set(surface, Voxel::AIR)])
        .unwrap();
    assert_eq!(world.environment_at(p), Some(e));
    let path =
        std::env::temp_dir().join(format!("climate-compatibility-{}.mvw", std::process::id()));
    world.save_edits(&path).unwrap();
    let other = ProceduralTerrain::with_config(42, TerrainConfig::for_climate(ClimatePreset::Arid))
        .unwrap();
    assert_ne!(world.identity(), other.identity());
    let dry_stats = other.environment_statistics();
    assert!(dry_stats.mean_rainfall < stats.mean_rainfall);
    assert!(dry_stats.mean_moisture < stats.mean_moisture);
    let mut dry_world = World::new(other);
    assert!(dry_world.load_edits(&path).is_err());
    assert_eq!(dry_world.storage().override_count(), 0);
    std::fs::remove_file(path).unwrap();
    // All added material definitions retain empty runtime physical properties.
    for id in [
        MaterialId::SANDSTONE,
        MaterialId::LIMESTONE,
        MaterialId::BASALT,
        MaterialId::MUD,
        MaterialId::SILT,
    ] {
        assert!(world.materials().get(id).unwrap().properties.is_empty());
    }
}

#[test]
fn large_region_has_diverse_connected_geography_with_bounded_memory() {
    let generator = ProceduralTerrain::with_spec(
        42,
        TerrainConfig::default(),
        microvoxel::WorldSpec {
            extent_m: 100_000,
            ..Default::default()
        },
    )
    .unwrap();
    let stats = generator.environment_statistics();
    assert!(stats.landforms.iter().all(|n| *n > stats.samples / 100));
    assert!(stats.geology.iter().all(|n| *n > 0));
    assert!(stats.max_elevation > 1400.);
    assert!(stats.min_elevation < 150.);
    assert!(stats.gentle_fraction > 0.4);
    assert!(generator.regional_memory_bytes() < 256 * 1024 * 1024);
    // Distant coordinates are populated, not a clamped copy of the old 10 km edge.
    let a = generator.sample_m(11_000., 15_000.).unwrap();
    let b = generator.sample_m(91_000., 85_000.).unwrap();
    assert!((a.height_m - b.height_m).abs() > 1.);
    let coarse = generator.at_grid(microvoxel::GridScale::new(1, 4).unwrap());
    let fine = generator.at_grid(microvoxel::GridScale::new(1, 100).unwrap());
    assert_eq!(
        coarse.sample_m(81_234., 78_456.).unwrap().height_m,
        fine.sample_m(81_234., 78_456.).unwrap().height_m
    );
    let world = World::new(generator);
    assert!(image_from_world(&world, DebugLayer::Biomes, 16).is_ok());
}
