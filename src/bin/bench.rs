use microvoxel::{
    generation::{FlatTerrain, Generator, ProceduralTerrain},
    materials::{Appearance, MaterialId},
    CellPos, Edit, Voxel, World,
};
use std::time::Instant;
fn measure<G: Generator>(mut world: World<G>) -> microvoxel::Result<()> {
    let start = Instant::now();
    let mut checksum = 0u64;
    for i in 0..200_000i64 {
        let p = CellPos::new(100 + i % 32, i % 64, 100 + (i / 32) % 32);
        if let Some(v) = world.voxel_at(p).known() {
            checksum = checksum.wrapping_add(v.material.0 as u64 + v.appearance.0 as u64);
        }
    }
    println!(
        "200,000 native cell queries: {:?} (checksum {checksum})",
        start.elapsed()
    );
    let edits: Vec<_> = (0..4096)
        .map(|i| {
            Edit::set(
                CellPos::new(i % 32, 20 + (i / 32) % 4, i / 128),
                Voxel::new(MaterialId::SAND, Appearance(123)),
            )
        })
        .collect();
    let start = Instant::now();
    world.apply(&edits)?;
    println!("4,096 transactional changes: {:?}; {} edited bricks; {} stored overrides; ~{} storage bytes",start.elapsed(),world.storage().chunk_count(),world.storage().override_count(),world.storage().memory_bytes());
    Ok(())
}
fn main() -> microvoxel::Result<()> {
    let start = Instant::now();
    if std::env::args().any(|s| s == "--terrain") {
        let world = World::new(ProceduralTerrain::new(42));
        println!("10 km macro-generation: {:?}", start.elapsed());
        measure(world)
    } else {
        println!("Flat baseline benchmark (use --terrain for the full generator)");
        measure(World::new(FlatTerrain {
            height: 32,
            water: 32,
            seed: 42,
        }))
    }
}
