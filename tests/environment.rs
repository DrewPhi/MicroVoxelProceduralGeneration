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
    // Check useful diversity, not an exact snapshot of tuning coefficients.
    assert!(stats.landforms.iter().all(|n| *n > stats.samples / 50));
    assert!(stats.geology.iter().all(|n| *n > 0));
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
