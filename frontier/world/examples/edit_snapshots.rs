//! Single-cell gameplay writes while a renderer retains the preceding world.
//! cargo run --release -p frontier-world --example edit_snapshots
use frontier_world::{Cell, FbmParams, Material, World};
use std::time::Instant;

fn main() {
    let mut world = World::fbm(
        7,
        FbmParams {
            amplitude: 0.,
            ..Default::default()
        },
    )
    .unwrap();
    world.restore_terrain_edits(
        (0..1600).flat_map(|x| (0..96).map(move |z| (Cell { x, y: 47, z }, Material::Wood))),
    );
    let mut samples = Vec::new();
    for i in 0..400 {
        let cell = Cell {
            x: (i * 611) % 1600,
            y: 47,
            z: (i * 31) % 96,
        };
        let old = world.clone();
        let before = old.terrain_override(cell);
        let next = if before == Some(Material::Wood) {
            Material::Stone
        } else {
            Material::Wood
        };
        let start = Instant::now();
        world.set(cell, next);
        samples.push(start.elapsed().as_secs_f64() * 1000.);
        assert_eq!(old.terrain_override(cell), before);
        assert_eq!(world.terrain_override(cell), Some(next));
    }
    samples.sort_by(f64::total_cmp);
    println!("153600 saved edits, 400 snapshot-preserving writes: median {:.4} ms, p95 {:.4} ms, p99 {:.4} ms",
        samples[199], samples[379],samples[395]);
}
