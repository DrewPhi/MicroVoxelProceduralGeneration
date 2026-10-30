//! Measure whole-brick sharing opportunities without introducing a DAG.
use microvoxel::{
    generation::{FlatTerrain, Generator, ProceduralTerrain},
    render::{
        lod::{MeshKey, TileKey},
        ray::Brick,
        terrain::Sampler,
    },
    World,
};
use std::{
    collections::{hash_map::DefaultHasher, BTreeSet},
    hash::{Hash, Hasher},
    sync::Arc,
    time::Instant,
};
fn shape(brick: &Brick) -> u64 {
    let mut h = DefaultHasher::new();
    for n in brick.words[..brick.nodes * 8].chunks_exact(8) {
        for i in 0..8 {
            let v = match i {
                0 | 4 => n[i] as i64 - brick.bounds[0][0] as i64,
                2 | 6 => n[i] as i64 - brick.bounds[0][2] as i64,
                _ => n[i] as i64,
            };
            v.hash(&mut h);
        }
    }
    for r in brick.words[brick.nodes * 8..(brick.nodes + brick.runs) * 8].chunks_exact(8) {
        [r[0], r[1], r[4], r[5], r[6] & 65535].hash(&mut h);
    }
    h.finish()
}
fn inspect<G: Generator>(name: &str, generator: G) {
    let mut sampler = Sampler::new(Arc::new(World::new(generator)));
    let mut shapes = BTreeSet::new();
    let mut full = BTreeSet::new();
    let mut bytes = 0;
    let start = Instant::now();
    for i in 0..16 {
        let key = MeshKey {
            tile: TileKey {
                level: 13,
                x: 2125 + i % 4,
                z: 3125 + i / 4,
            },
            neighbors: [13; 4],
            revision: 0,
        };
        let brick = sampler.ray_brick(key, || false).unwrap();
        shapes.insert(shape(&brick));
        let mut hash = DefaultHasher::new();
        brick.words.hash(&mut hash);
        full.insert(hash.finish());
        bytes += brick.bytes();
    }
    println!("{name}: 16 native bricks; {} normalized geometry patterns, {} complete payloads; {:.2} MiB unshared; {:.2}ms build",shapes.len(),full.len(),bytes as f64/1048576.,start.elapsed().as_secs_f64()*1000.);
}
fn main() {
    inspect(
        "flat terrain",
        FlatTerrain {
            height: 30000,
            water: 30000,
            seed: 42,
        },
    );
    inspect("procedural terrain", ProceduralTerrain::new(42));
    println!("Geometry fingerprints omit colors and normalize X/Z. This measures whole bricks, not potential sharing of internal subtrees. No DAG or interning is enabled.");
}
