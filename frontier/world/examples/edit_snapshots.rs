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

    let min = [-8; 2];
    let max = [1608, 104];
    let mut generic = Vec::new();
    let mut paged = Vec::new();
    for _ in 0..30 {
        let start = Instant::now();
        let mut indexed = world.edit_cells_in_columns(min, max);
        indexed.retain(|&c| world.terrain_override(c).is_some());
        generic.push(start.elapsed().as_secs_f64() * 1000.);
        let start = Instant::now();
        let terrain = world
            .terrain_edit_cells_in_columns(min, max, 524288)
            .unwrap();
        paged.push(start.elapsed().as_secs_f64() * 1000.);
        assert_eq!(terrain, indexed);
    }
    generic.sort_by(f64::total_cmp);
    paged.sort_by(f64::total_cmp);
    println!("153600 authored addresses, 30 reads: indexed median {:.3} ms, p95 {:.3} ms; terrain pages median {:.3} ms, p95 {:.3} ms",
        generic[14], generic[28], paged[14], paged[28]);
}
