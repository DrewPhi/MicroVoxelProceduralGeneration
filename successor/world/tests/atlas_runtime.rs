//! Saved-atlas integration checks. No compiler or network access is triggered.
use microvoxel::{
    generation::{Generator, Terrain},
    world::{Edit, Lookup, World},
    CellPos, GridScale, Voxel,
};
use std::{path::PathBuf, time::Instant};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../output/atlas-climate/seed42")
}
fn source(scale: u32, grid: GridScale) -> Terrain {
    Terrain::from_atlas(&fixture(), grid, scale).expect("generate saved seed42 atlas first")
}

#[test]
#[ignore = "requires local compiled seed42 atlas (no generation)"]
fn saved_atlas_queries_cache_scales_and_edit_reload() {
    let start = Instant::now();
    let grid = GridScale::default();
    let terrain = source(10, grid);
    eprintln!(
        "atlas load {:.3}s, payload {:.2} MiB",
        start.elapsed().as_secs_f64(),
        terrain.regional_memory_bytes() as f64 / 1048576.
    );
    let coords: Vec<_> = (0..4096i64)
        .map(|i| {
            (
                (i * 104729 + 160001) % 1_599_999,
                (i * 15485863 + 480003) % 1_599_999,
            )
        })
        .collect();
    let query_start = Instant::now();
    let expected: Vec<_> = coords
        .iter()
        .map(|&(x, z)| {
            let c = terrain.column_hint(x, z).unwrap();
            assert!(c.water >= c.ground);
            assert!(c.water < terrain.spec().ceiling_cells());
            (c.ground, c.water, c.surface)
        })
        .collect();
    eprintln!(
        "4096 distributed cold columns {:.3}s",
        query_start.elapsed().as_secs_f64()
    );
    // Different request order and bounded patch eviction cannot change occupancy.
    for (&(x, z), &(ground, water, surface)) in coords.iter().zip(&expected).rev() {
        let c = terrain.column_hint(x, z).unwrap();
        assert_eq!((c.ground, c.water, c.surface), (ground, water, surface));
        for step in [4, 64, 1024] {
            terrain.display_column(x, z, step).unwrap();
        }
        assert_eq!(terrain.column_hint(x, z).unwrap().ground, ground);
    }
    // Same source coordinates at different native resolutions and world scales.
    let fine = terrain.at_grid(GridScale::new(1, 100).unwrap());
    let large = source(1, grid);
    let compressed = source(3, grid);
    for [x, z] in [
        [40000., 35000.],
        [20684., 82.],
        [53120., 61000.],
        [97500., 47500.],
    ] {
        let native = terrain
            .column_hint(grid.cells(x).unwrap(), grid.cells(z).unwrap())
            .unwrap();
        let fine_col = fine
            .column_hint((x * 100.) as i64, (z * 100.) as i64)
            .unwrap();
        assert!(
            (native.ground as f64 * grid.meters() - fine_col.ground as f64 * 0.01).abs()
                <= grid.meters()
        );
        let unscaled = large
            .column_hint(grid.cells(x * 10.).unwrap(), grid.cells(z * 10.).unwrap())
            .unwrap();
        assert!(
            (native.ground as f64 * grid.meters() * 10. - unscaled.ground as f64 * grid.meters())
                .abs()
                <= grid.meters() * 10.
        );
    }
    let last = compressed.spec().extent_cells() - 1;
    assert!(compressed.column_hint(last, last).is_some());
    assert!(compressed
        .terrain_summary([compressed.spec().extent_m as f64 - 1.; 2], 1.)
        .is_some());
    let (x, z) = coords[0];
    let ground = expected[0].0;
    let pos = CellPos::new(x, ground - 1, z);
    let mut world = World::new(terrain);
    world.apply(&[Edit::set(pos, Voxel::AIR)]).unwrap();
    let path = std::env::temp_dir().join(format!("atlas-reload-{}.mvw", std::process::id()));
    world.save_edits(&path).unwrap();
    let mut reloaded = World::new(source(10, grid));
    reloaded.load_edits(&path).unwrap();
    assert_eq!(reloaded.voxel_at(pos), Lookup::Known(Voxel::AIR));
    let mut incompatible = World::new(compressed);
    assert!(incompatible.load_edits(&path).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore = "requires local compiled seed42 atlas (CPU collision, no GPU)"]
fn saved_atlas_player_walk_stop_resume_stays_clear() {
    use microvoxel::player::{Player, PlayerInput, TICK_SECONDS};
    let grid = GridScale::default();
    let world = World::new(source(10, grid));
    let x = grid.cells(97576.175).unwrap();
    let z = grid.cells(47560.052).unwrap();
    let mut player = Player::new(42, CellPos::new(x, 0, z)).unwrap();
    assert!(player.place_above(&world, x, z));
    let start = player.root;
    let mut stopped = start;
    for tick in 0..3600 {
        let moving = tick < 360 || tick >= 600;
        player
            .advance(
                &world,
                TICK_SECONDS,
                PlayerInput {
                    forward: if moving { 1. } else { 0. },
                    yaw: 0.,
                    sprint: tick >= 600,
                    jump: tick == 900,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(player.clear(&world), "overlap after tick {tick}");
        if tick == 599 {
            stopped = player.root;
        }
    }
    assert!((stopped.x - start.x).abs() > grid.cells(1.).unwrap());
    assert!((player.root.x - stopped.x).abs() > grid.cells(100.).unwrap());
    eprintln!(
        "3600 atlas physics ticks: start={start:?}, after stop={stopped:?}, finish={:?}",
        player.root
    );
}
