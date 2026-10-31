//! Multi-seed terrain evidence. Actual native renders plus physical field metrics.
use crate::{gpu::Gpu, settings::Settings};
use glam::{Mat4, Vec3};
use microvoxel::{
    generation::{ClimatePreset, Generator, Refinement, Terrain, TerrainConfig},
    render::{
        lod::{MeshKey, TileKey},
        terrain::Sampler,
    },
    World,
};
use std::{collections::BTreeSet, fmt::Write as _, path::Path, sync::Arc, time::Instant};
const W: u32 = 640;
const H: u32 = 360;
fn image(path: &Path, w: u32, h: u32, data: &[u8]) -> microvoxel::Result<()> {
    let mut encoder =
        png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), w, h);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(data)?;
    Ok(())
}
fn edge_distance(mask: &[u8]) -> Vec<f32> {
    let mut distance = vec![1e6f32; (W * H) as usize];
    for y in 1..H as usize - 1 {
        for x in 1..W as usize - 1 {
            let i = y * W as usize + x;
            if mask[i * 4] > 127
                && [i - 1, i + 1, i - W as usize, i + W as usize]
                    .iter()
                    .any(|j| mask[j * 4] < 128)
            {
                distance[i] = 0.;
            }
        }
    }
    for i in 0..distance.len() {
        let x = i % W as usize;
        if x > 0 {
            distance[i] = distance[i].min(distance[i - 1] + 1.);
        }
        if i >= W as usize {
            distance[i] = distance[i].min(distance[i - W as usize] + 1.);
        }
    }
    for i in (0..distance.len()).rev() {
        let x = i % W as usize;
        if x + 1 < W as usize {
            distance[i] = distance[i].min(distance[i + 1] + 1.);
        }
        if i + (W as usize) < distance.len() {
            distance[i] = distance[i].min(distance[i + W as usize] + 1.);
        }
    }
    distance
}
fn silhouette(a: &[u8], b: &[u8]) -> [f64; 3] {
    let da = edge_distance(a);
    let db = edge_distance(b);
    let mut max = 0f64;
    let mut sum = 0.;
    let mut count = 0f64;
    let mut lost = 0.;
    let mut foreground = 0f64;
    for i in 0..da.len() {
        if da[i] == 0. {
            let v = db[i] as f64;
            max = max.max(v);
            sum += v;
            count += 1.;
        }
        if db[i] == 0. {
            let v = da[i] as f64;
            max = max.max(v);
            sum += v;
            count += 1.;
        }
        if a[i * 4] > 127 {
            foreground += 1.;
            if b[i * 4] < 128 {
                lost += 1.;
            }
        }
    }
    [sum / count.max(1.), max, lost / foreground.max(1.)]
}
fn render_patch(
    gpu: &mut Gpu,
    world: Arc<World<Terrain>>,
    center: [f64; 2],
    level: u8,
    stem: &Path,
) -> microvoxel::Result<(Vec<u8>, Vec<u8>, usize, f64)> {
    gpu.tiles.clear();
    gpu.bytes = 0;
    gpu.quality_mask = false;
    gpu.quality_water = false;
    let grid = world.spec().grid;
    let cell = grid.meters();
    let step = 1i64 << (gpu.grid.finest - level);
    let edge = 32 * step;
    let x0 = ((center[0] / cell) as i64).div_euclid(512) * 512;
    let z0 = ((center[1] / cell) as i64).div_euclid(512) * 512;
    let mut sampler = Sampler::new(world.clone());
    let mut cover = BTreeSet::new();
    let start = Instant::now();
    for z in (z0..z0 + 512).step_by(edge as usize) {
        for x in (x0..x0 + 512).step_by(edge as usize) {
            let key = MeshKey {
                corner_levels: None,
                tile: TileKey {
                    level,
                    x: (x / edge) as u32,
                    z: (z / edge) as u32,
                },
                neighbors: [level; 4],
                revision: 0,
            };
            let mesh = sampler.mesh(key)?;
            cover.insert(key);
            gpu.upload(key, mesh, &cover)?;
        }
    }
    let build_ms = start.elapsed().as_secs_f64() * 1000.;
    let p = [(x0 + 256) as f64 * cell, (z0 + 256) as f64 * cell];
    let y = world
        .environment_at(grid.position([p[0], 0., p[1]]).unwrap())
        .unwrap()
        .elevation_m as f64;
    let bounds = world
        .terrain_summary([x0 as f64 * cell, z0 as f64 * cell], 32.)
        .unwrap();
    let eye = [p[0] - 34., bounds.max_m + 24., p[1] - 34.];
    let direction = Vec3::new(34., (y - eye[1]) as f32, 34.).normalize();
    let matrix = Mat4::perspective_rh(60f32.to_radians(), W as f32 / H as f32, 0.025, 24000.)
        * Mat4::look_to_rh(Vec3::ZERO, direction, Vec3::Y);
    let texture = gpu.offscreen();
    let view = texture.create_view(&Default::default());
    gpu.render_view(&view, eye, matrix, &cover, None, false);
    gpu.device.poll(wgpu::Maintain::Wait);
    gpu.screenshot(
        &texture,
        &stem.with_file_name(format!(
            "{}.png",
            stem.file_name().unwrap().to_string_lossy()
        )),
    )?;
    gpu.quality_mask = true;
    gpu.render_view(&view, eye, matrix, &cover, None, false);
    let mask = gpu.read_pixels(&texture)?;
    gpu.screenshot(
        &texture,
        &stem.with_file_name(format!(
            "{}.mask.png",
            stem.file_name().unwrap().to_string_lossy()
        )),
    )?;
    gpu.quality_water = true;
    gpu.render_view(&view, eye, matrix, &cover, None, false);
    let water = gpu.read_pixels(&texture)?;
    gpu.screenshot(
        &texture,
        &stem.with_file_name(format!(
            "{}.water.png",
            stem.file_name().unwrap().to_string_lossy()
        )),
    )?;
    gpu.quality_water = false;
    Ok((mask, water, gpu.bytes, build_ms))
}
pub fn run(directory: &Path, requested_seed: u64) -> microvoxel::Result<()> {
    std::fs::create_dir_all(directory)?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let mut gpu = pollster::block_on(Gpu::new(&instance, None, W, H))?;
    gpu.hud_visible = false;
    gpu.detail_settings(&Settings {
        haze: 0.,
        ambient: false,
        shadows: false,
        antialias: false,
        transitions: false,
        memory_mib: 256,
        ..Default::default()
    });
    let mut csv=String::from("seed,climate,landscape,x_m,z_m,method,build_fine_ms,build_coarse_ms,fine_bytes,coarse_bytes,silhouette_mean_l1_px,silhouette_max_l1_px,lost_foreground_fraction,vertical_depth_rmse_m,grid_field_error_m,geological_features,lost_water_fraction\n");
    let seeds = if requested_seed == 42 {
        vec![42, 137, 909]
    } else {
        vec![requested_seed]
    };
    let mut reference = String::from("source,seed,climate,min_m,max_m,mean_slope,p95_slope\n");
    let dem: Vec<[f64; 3]> = include_str!("../../../../assets/reference/grand_canyon_srtm.csv")
        .lines()
        .skip(1)
        .map(|line| {
            let v: Vec<f64> = line.split(',').map(|v| v.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        })
        .collect();
    let dx = 0.002 * 111320. * dem[0][0].to_radians().cos();
    let dz = 0.002 * 111132.;
    let stats = |h: &[f64]| {
        let mut slopes = Vec::new();
        for z in 0..7 {
            for x in 0..7 {
                let i = z * 8 + x;
                slopes.push(((h[i + 1] - h[i]) / dx).hypot((h[i + 8] - h[i]) / dz));
            }
        }
        slopes.sort_by(f64::total_cmp);
        [
            h.iter().copied().fold(f64::INFINITY, f64::min),
            h.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            slopes.iter().sum::<f64>() / slopes.len() as f64,
            slopes[46],
        ]
    };
    let observed: Vec<_> = dem.iter().map(|p| p[2]).collect();
    let r = stats(&observed);
    writeln!(reference, "SRTM90m,,,{}, {},{},{}", r[0], r[1], r[2], r[3])?;
    for seed in seeds {
        for climate in [
            ClimatePreset::Temperate,
            ClimatePreset::Arid,
            ClimatePreset::Tropical,
        ] {
            let terrain = Terrain::with_config(seed, TerrainConfig::for_climate(climate))?;
            let mut manifest = String::from("kind,x_m,y_m,z_m,radius_m,half_length_m\n");
            for f in terrain.geological_features() {
                writeln!(
                    manifest,
                    "{:?},{},{},{},{},{}",
                    f.kind, f.center_m[0], f.center_m[1], f.center_m[2], f.radius_m, f.length_m
                )?;
            }
            std::fs::write(
                directory.join(format!("{seed}_{climate}_features.csv")),
                manifest,
            )?;
            let mut points = [[4250., 6250.]; 3];
            let mut scores = [f64::NEG_INFINITY; 3];
            let mut overview = Vec::new();
            for z in 0..96 {
                for x in 0..96 {
                    let p = [
                        (x as f64 + 0.5) * 10000. / 96.,
                        (z as f64 + 0.5) * 10000. / 96.,
                    ];
                    let s = terrain.sample_m(p[0], p[1]).unwrap();
                    let v = ((s.height_m / 1024.).sqrt() * 255.) as u8;
                    overview.extend_from_slice(&[v, v, v]);
                    let e = s.environment;
                    let candidate = [
                        e.slope as f64,
                        1. - e.slope as f64 - (e.landform_weights[0] - 1.).abs() as f64,
                        if s.water_m > s.height_m {
                            1. + e.drainage_area_m2 as f64 / 1e8
                        } else {
                            e.ground_wetness as f64
                        },
                    ];
                    for i in 0..3 {
                        if candidate[i] > scores[i]
                            && p[0] > 64.
                            && p[1] > 64.
                            && p[0] < 9900.
                            && p[1] < 9900.
                        {
                            scores[i] = candidate[i];
                            points[i] = p;
                        }
                    }
                }
            }
            image(
                &directory.join(format!("{seed}_{climate}_height.png")),
                96,
                96,
                &overview,
            )?;
            let heights: Vec<_> = (0..64)
                .map(|i| {
                    terrain
                        .sample_m(4250. + (i % 8) as f64 * dx, 6250. + (i / 8) as f64 * dz)
                        .unwrap()
                        .height_m
                })
                .collect();
            let r = stats(&heights);
            writeln!(
                reference,
                "generated,{seed},{climate},{},{},{},{}",
                r[0], r[1], r[2], r[3]
            )?;
            std::fs::write(directory.join("reference_metrics.csv"), &reference)?;
            for (landscape, point) in ["mountain", "plain", "water"].into_iter().zip(points) {
                for mode in [Refinement::Noise, Refinement::Geomorphic] {
                    let generator = terrain.with_refinement(mode);
                    let mut grid_error = 0f64;
                    for scale in [
                        microvoxel::GridScale::new(1, 4)?,
                        microvoxel::GridScale::default(),
                        microvoxel::GridScale::new(1, 100)?,
                        microvoxel::GridScale::new(1, 1000)?,
                    ] {
                        let variant = generator.at_grid(scale);
                        grid_error = grid_error.max(
                            (variant.sample_m(point[0], point[1]).unwrap().height_m
                                - generator.sample_m(point[0], point[1]).unwrap().height_m)
                                .abs(),
                        );
                    }
                    let features = generator.geological_features().len();
                    let mut depth_error = 0.;
                    // Top-down vertical rays: exact physical field versus a 1 m proxy.
                    for z in 0..16 {
                        for x in 0..16 {
                            let px = point[0] + x as f64 * 0.5;
                            let pz = point[1] + z as f64 * 0.5;
                            let exact = generator.sample_m(px, pz).unwrap().height_m;
                            let c = generator
                                .display_column((px * 16.) as i64, (pz * 16.) as i64, 16)
                                .unwrap();
                            depth_error += (exact - c.ground as f64 / 16.).powi(2);
                        }
                    }
                    let world = Arc::new(World::new(generator));
                    let name = if mode == Refinement::Noise {
                        "noise"
                    } else {
                        "geomorphic"
                    };
                    let stem = directory.join(format!("{seed}_{climate}_{landscape}_{name}"));
                    let (fine, fwater, fb, ft) = render_patch(
                        &mut gpu,
                        world.clone(),
                        point,
                        13,
                        &stem.with_extension("fine"),
                    )?;
                    let (coarse, cwater, cb, ct) =
                        render_patch(&mut gpu, world, point, 9, &stem.with_extension("coarse"))?;
                    let error = silhouette(&fine, &coarse);
                    let water_total = fwater.chunks_exact(4).filter(|p| p[0] > 127).count();
                    let water_loss = fwater
                        .chunks_exact(4)
                        .zip(cwater.chunks_exact(4))
                        .filter(|(a, b)| a[0] > 127 && b[0] < 128)
                        .count() as f64
                        / water_total.max(1) as f64;
                    writeln!(csv,"{seed},{climate},{landscape},{},{},{name},{ft:.3},{ct:.3},{fb},{cb},{:.4},{:.4},{:.6},{:.5},{grid_error},{features},{water_loss:.6}",point[0],point[1],error[0],error[1],error[2],(depth_error/256.).sqrt())?;
                    println!("Quality {seed} {climate} {landscape} {name}: silhouette {:?}, fine {ft:.1} ms",error);
                    std::fs::write(directory.join("metrics.csv"), &csv)?;
                }
            }
        }
    }
    std::fs::write(directory.join("README.txt"),"Manifest v1: seeds 42/137/909, temperate/arid/tropical, selected slope/plain/wet sites. Noise and geomorphic amplification share regional inputs but are not old-release snapshots. Fine 6.25 cm versus coarse 1 m over a 32 m patch. Silhouette distance uses symmetric L1 pixel edges; foreground loss is not river topology. Vertical depth RMSE uses top-down physical field versus a 1 m proxy, not perspective depth readback. Grid error evaluates identical physical points at 25cm/6.25cm/1cm/1mm without dense allocation. Geometry/appearance and feature validity also have core tests. Site labels are heuristic selectors, not guaranteed biomes. A small public SRTM crop is bundled with provenance; reference_metrics.csv compares broad elevation and slope statistics, not geological accuracy. --seed with a non-42 value selects a held-out seed.\n")?;
    Ok(())
}
