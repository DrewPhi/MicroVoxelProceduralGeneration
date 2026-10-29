//! Full-region orthographic export using the normal voxel mesh/color pipeline.
use crate::gpu::Gpu;
use glam::{Mat4, Vec3};
use microvoxel::{
    generation::ProceduralTerrain,
    render::{
        lod::{MeshKey, TileKey, EXTENT},
        terrain::Sampler,
    },
    World,
};
use std::{collections::BTreeSet, path::Path, sync::Arc, time::Instant};

pub fn render(world: Arc<World<ProceduralTerrain>>, path: &Path) -> microvoxel::Result<()> {
    let start = Instant::now();
    let (width, height) = (3200, 2400);
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let mut gpu = pollster::block_on(Gpu::new(&instance, None, width, height))?;
    // Uniform 16 m display cells give equal detail to the entire region. This is
    // a bounded overview, not an allocation of every 6.25 cm voxel in the world.
    let level = 5;
    let edge = TileKey { level, x: 0, z: 0 }.edge();
    let side = ((EXTENT + edge - 1) / edge) as u32;
    let mut sampler = Sampler::new(Arc::clone(&world));
    let mut cover = BTreeSet::new();
    for z in 0..side {
        for x in 0..side {
            let key = MeshKey {
                tile: TileKey { level, x, z },
                neighbors: [level; 4],
                revision: world.revision(),
            };
            let mesh = sampler.mesh(key)?;
            cover.insert(key);
            gpu.upload(key, mesh, &cover)?;
        }
        if z % 5 == 0 {
            println!("Isometric overview: row {}/{side}", z + 1);
        }
    }
    // Equal X/Y/Z viewing direction gives a true isometric projection. Heights
    // use their actual meter scale; no vertical exaggeration or perspective.
    let eye = [15000., 10250., 15000.];
    let direction = Vec3::new(-1., -1., -1.);
    let horizontal = 15600.;
    let vertical = horizontal * height as f32 / width as f32;
    let matrix = Mat4::orthographic_rh(
        -horizontal / 2.,
        horizontal / 2.,
        -vertical / 2.,
        vertical / 2.,
        1.,
        40000.,
    ) * Mat4::look_to_rh(Vec3::ZERO, direction, Vec3::Y);
    let texture = gpu.offscreen();
    let view = texture.create_view(&Default::default());
    let visible = gpu.render_view(&view, eye, matrix, &cover, None, true);
    gpu.device.poll(wgpu::Maintain::Wait);
    gpu.screenshot(&texture, path)?;
    println!("10 km x 10 km; {width} x {height} PNG; 16 m display cells; true-scale heights; {visible} tiles; {:.2} MiB mesh data; {:.2}s total", gpu.bytes as f64/1048576., start.elapsed().as_secs_f64());
    Ok(())
}
