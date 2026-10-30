//! Equal-footprint CPU measurements. No graphics context or window is required.
use microvoxel::{
    generation::Terrain,
    render::{
        lod::{MeshKey, TileKey},
        terrain::Sampler,
    },
    World,
};
use std::{sync::Arc, time::Instant};
pub fn run(world: Arc<World<Terrain>>) -> microvoxel::Result<()> {
    if world.spec() != microvoxel::WorldSpec::default() {
        return Err("the legacy equal-footprint benchmark uses default-grid fixtures; use --terrain-quality for scale checks".into());
    }
    println!("Same 16 m × 16 m patch, fresh profile cache for each LOD; includes colors and greedy merging:");
    for level in (10..=13).rev() {
        let mut sampler = Sampler::new(Arc::clone(&world));
        let start = Instant::now();
        let mut faces = 0;
        let mut quads = 0;
        let mut bytes = 0;
        let edge = 32 * (1i64 << (13 - level));
        let x0 = 149_056i64.div_euclid(256) * 256;
        let z0 = 80_592i64.div_euclid(256) * 256;
        for z in (z0..z0 + 256).step_by(edge as usize) {
            for x in (x0..x0 + 256).step_by(edge as usize) {
                let mesh = sampler.mesh(MeshKey {
                    tile: TileKey {
                        level,
                        x: (x / edge) as u32,
                        z: (z / edge) as u32,
                    },
                    neighbors: [level; 4],
                    revision: world.revision(),
                })?;
                faces += mesh.faces();
                quads += mesh.quads();
                bytes += mesh.bytes();
            }
        }
        println!(
            "{:>5.2} cm | {:>8} exposed faces -> {:>7} rectangles | {:>7.2} MiB | {:>8.2} ms",
            100. * (1i64 << (13 - level)) as f64 / 16.,
            faces,
            quads,
            bytes as f64 / (1024. * 1024.),
            start.elapsed().as_secs_f64() * 1000.
        );
    }
    Ok(())
}
