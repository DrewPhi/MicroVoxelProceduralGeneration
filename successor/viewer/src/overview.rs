//! Full-region orthographic export using the normal voxel mesh/color pipeline.
use crate::gpu::Gpu;
use glam::{Mat4, Vec3};
use microvoxel::{
    generation::Terrain,
    render::{
        lod::{Grid, MeshKey, TileKey},
        terrain::Sampler,
    },
    World,
};
use std::{collections::BTreeSet, path::Path, sync::Arc, time::Instant};

pub fn render(
    world: Arc<World<Terrain>>,
    path: &Path,
    area_km2: Option<f64>,
    memory_mib: usize,
    center: Option<[f64; 2]>,
) -> microvoxel::Result<()> {
    let start = Instant::now();
    let (width, height) = (4096, 3072);
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let mut gpu = pollster::block_on(Gpu::new(&instance, None, width, height))?;
    gpu.grid = Grid::new(world.spec());
    gpu.detail_settings(&crate::settings::Settings {
        memory_mib,
        ..Default::default()
    });
    let scale = world.baseline().world_scale() as f64;
    let world_edge = world.spec().extent_m as f64;
    let requested_edge = area_km2.map_or(world_edge, |a| (a * 1_000_000.).sqrt());
    if requested_edge > world_edge {
        return Err("overview crop exceeds the source world; increase --extent-km".into());
    }
    // Crop the SAME region solve: generating a smaller independent world would
    // change drainage/climate boundary conditions and invalidate the comparison.
    if center.is_some_and(|p| {
        p.iter()
            .any(|v| !v.is_finite() || *v < 0. || *v >= world_edge)
    }) {
        return Err("overview center lies outside this world".into());
    }
    let mut peak = center.unwrap_or([world_edge * 0.5, world_edge * 0.5]);
    let mut highest = f64::NEG_INFINITY;
    if area_km2.is_some() && center.is_none() {
        for iz in 0..256 {
            for ix in 0..256 {
                let x = (ix as f64 + 0.5) * world_edge / 256.;
                let z = (iz as f64 + 0.5) * world_edge / 256.;
                if let Some(s) = world.baseline().sample_m(x, z) {
                    if s.height_m > highest {
                        highest = s.height_m;
                        peak = [x, z];
                    }
                }
            }
        }
    }
    // Bounded overview density; crop edges align to display cells, not pages.
    let source_extent = requested_edge * scale;
    let display_m = 16. * 2f64.powf((source_extent / 12_500.).max(1.).log2().round());
    let level = gpu.grid.finest.saturating_sub(
        (display_m / world.baseline().world_scale() as f64 / gpu.grid.cell_m())
            .log2()
            .round()
            .max(0.) as u8,
    );
    let step = TileKey { level, x: 0, z: 0 }.step_in(gpu.grid);
    let step_m = step as f64 * gpu.grid.cell_m();
    let crop_edge = if area_km2.is_some() {
        (requested_edge / step_m).floor().max(1.) * step_m
    } else {
        world_edge
    };
    let origin = peak.map(|v| {
        ((v - crop_edge * 0.5).clamp(0., world_edge - crop_edge) / step_m).floor() * step_m
    });
    let low = origin.map(|v| (v / gpu.grid.cell_m()).round() as i64);
    let high = low.map(|v| v + (crop_edge / gpu.grid.cell_m()).round() as i64);
    let edge = TileKey { level, x: 0, z: 0 }.edge_in(gpu.grid);
    let first = low.map(|v| (v / edge) as u32);
    let end = high.map(|v| ((v + edge - 1) / edge) as u32);
    let mut sampler = Sampler::new(Arc::clone(&world));
    let mut cover = BTreeSet::new();
    for z in first[1]..end[1] {
        for x in first[0]..end[0] {
            let key = MeshKey {
                corner_levels: None,
                tile: TileKey { level, x, z },
                neighbors: [level; 4],
                revision: world.revision(),
            };
            let mut mesh = sampler.display_mesh(key, || false)?;
            // Only whole display cells are removed. Retain original color/height
            // records so the cropped terrain is identical to the parent world.
            for part in &mut mesh.parts {
                part.surface.quads.retain(|q| {
                    let px = mesh.origin[0] + q.origin[0] as i64;
                    let pz = mesh.origin[2] + q.origin[2] as i64;
                    px >= low[0] && pz >= low[1] && px < high[0] && pz < high[1]
                });
            }
            cover.insert(key);
            gpu.upload(key, mesh, &cover)?;
        }
        if (z - first[1]) % 5 == 0 {
            println!(
                "Isometric overview: row {}/{}",
                z - first[1] + 1,
                end[1] - first[1]
            );
        }
    }
    // Equal X/Y/Z viewing direction gives a true isometric projection. Heights
    // use their actual meter scale; no vertical exaggeration or perspective.
    let extent = crop_edge as f32;
    let ratio = extent / 10000.;
    let eye = [
        origin[0] + 15000. * ratio as f64,
        10250. * ratio as f64,
        origin[1] + 15000. * ratio as f64,
    ];
    let direction = Vec3::new(-1., -1., -1.);
    let horizontal = 15600. * ratio;
    let vertical = horizontal * height as f32 / width as f32;
    let matrix = Mat4::orthographic_rh(
        -horizontal / 2.,
        horizontal / 2.,
        -vertical / 2.,
        vertical / 2.,
        1.,
        40000. * ratio,
    ) * Mat4::look_to_rh(Vec3::ZERO, direction, Vec3::Y);
    let texture = gpu.offscreen();
    let view = texture.create_view(&Default::default());
    let visible = gpu.render_view(&view, eye, matrix, &cover, None, true);
    gpu.device.poll(wgpu::Maintain::Wait);
    gpu.screenshot(&texture, path)?;
    let peak_info = if center.is_some() {
        format!("requested_crop_center_m={peak:?}")
    } else if area_km2.is_some() {
        format!("highest_sample_m={highest:.1}\nhighest_sample_xz={peak:?}")
    } else {
        "highest_sample=not scanned (full-region view)".into()
    };
    std::fs::write(path.with_extension("txt"),format!(
        "seed={}\nworld_edge_m={world_edge}\ncrop_origin_m={origin:?}\ncrop_edge_m={crop_edge}\ncrop_area_km2={}\ndisplay_cell_m={step_m}\nterrain_budget_mib={memory_mib}\n{peak_info}\nTrue-scale isometric; same regional solve, no terrain or vertical rescaling.\n",
        world.identity().seed,crop_edge*crop_edge/1_000_000.))?;
    println!(
        "Crop: {origin:?} meters, {:.3} km2; {peak_info}",
        crop_edge * crop_edge / 1_000_000.
    );

    println!("{extent} m x {extent} m; {width} x {height} PNG; {display_m} source-meter display cells; true-scale heights; {visible} tiles; {:.2} MiB mesh data; {:.2}s total", gpu.bytes as f64/1048576., start.elapsed().as_secs_f64());
    Ok(())
}
