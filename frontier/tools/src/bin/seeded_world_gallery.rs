//! Canonical CPU field samples, not engine renders or frame-rate measurements.
use frontier_world::{Cell, FbmParams, FbmRecipe, FbmTerrain, voxel_size};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = PathBuf::from("output/seeded-world/cpu");
    let mut seeds = Vec::<u64>::new();
    let mut spans = Vec::<f64>::new();
    let mut n = 257usize;
    let mut recipe = FbmRecipe::SeededWorld;
    let mut center = [0f64; 2];
    let mut args = frontier_world::voxel_grid_args(std::env::args().skip(1))?;
    while let Some(arg) = args.next() {
        if arg == "--help" {
            println!("seeded_world_gallery [--output DIR] [--seed INTEGER] [--span METERS] [--resolution 257] [--center X,Z] [--recipe seeded-world|watershed]\nRepeat --seed/--span; defaults seeds 7,19,83 and spans 10000,100000m. Canonical CPU samples only; eval timing is not FPS. Then run python3 tools/seeded_world_gallery.py DIR");
            return Ok(());
        }
        let value = args.next().ok_or("flag requires a value")?;
        match arg.as_str() {
            "--output" => out = value.into(),
            "--seed" => seeds.push(value.parse()?),
            "--span" => spans.push(value.parse()?),
            "--recipe" => {
                recipe = match value.as_str() {
                    "seeded-world" => FbmRecipe::SeededWorld,
                    "watershed" => FbmRecipe::Watershed,
                    _ => return Err("--recipe requires seeded-world|watershed".into()),
                }
            }
            "--resolution" => n = value.parse()?,
            "--center" => {
                let (x, z) = value.split_once(',').ok_or("center requires X,Z")?;
                center = [x.parse()?, z.parse()?];
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if seeds.is_empty() {
        seeds = vec![7, 19, 83];
    }
    if spans.is_empty() {
        spans = vec![10000., 100000.];
    }
    if !(2..=2049).contains(&n)
        || spans.iter().any(|s| !s.is_finite() || *s <= 0. || *s > 1e8)
        || center.iter().any(|v| !v.is_finite() || v.abs() > 1e9)
    {
        return Err(
            "resolution requires 2..2049; spans finite 0..1e8m; center finite within ±1e9m".into(),
        );
    }
    fs::create_dir_all(&out)?;
    let mut metrics = BufWriter::new(File::create(out.join("metrics.csv"))?);
    writeln!(metrics,"file,seed,source_version,appearance_version,span_m,resolution,ground_min_m,ground_max_m,visible_min_m,visible_max_m,water_fraction,temperature_min,temperature_max,moisture_min,moisture_max,eval_ms,eval_ns_per_sample")?;
    let mut biomes = BufWriter::new(File::create(out.join("biomes.csv"))?);
    writeln!(biomes, "file,biome,samples,fraction")?;
    for seed in seeds {
        let terrain = FbmTerrain::new(
            seed,
            FbmParams {
                recipe,
                ..Default::default()
            },
        )?;
        for &span in &spans {
            let file = format!("seeded_world_{seed}_{span}m");
            let spacing = span / (n - 1) as f64;
            // Include canonical column queries and diagnostics, exclude CSV IO.
            let start = Instant::now();
            let mut rows = Vec::with_capacity(n * n);
            for iz in 0..n {
                for ix in 0..n {
                    let x = center[0] + ix as f64 * spacing - span / 2.;
                    let z = center[1] + iz as f64 * spacing - span / 2.;
                    let cell = Cell::from_meters([x, 0., z]);
                    let col = terrain.column_cells(cell.x, cell.z);
                    let fields = terrain.sample_fields(
                        (cell.x as f64 + 0.5) * voxel_size(),
                        (cell.z as f64 + 0.5) * voxel_size(),
                    );
                    rows.push((x, z, col, fields));
                }
            }
            let elapsed = start.elapsed();
            let mut csv = BufWriter::new(File::create(out.join(format!("{file}.csv")))?);
            writeln!(csv,"x_m,z_m,ground_m,visible_m,water_depth_m,water_height_m,continentalness,temperature,moisture,mountain,biome,ground_material,surface_material,r,g,b")?;
            let mut low = [f64::INFINITY; 4];
            let mut high = [f64::NEG_INFINITY; 4];
            let mut water = 0usize;
            let mut counts = BTreeMap::new();
            for &(x, z, c, f) in &rows {
                let ground = c.ground_top as f64 * voxel_size();
                let visible = c.visible_top as f64 * voxel_size();
                let wet = c.visible_top > c.ground_top;
                water += usize::from(wet);
                *counts.entry(format!("{:?}", f.biome)).or_insert(0usize) += 1;
                for (i, v) in [ground, visible, f.temperature, f.moisture]
                    .into_iter()
                    .enumerate()
                {
                    low[i] = low[i].min(v);
                    high[i] = high[i].max(v);
                }
                let [r, g, b] = c.top_material().color();
                writeln!(
                    csv,
                    "{x},{z},{ground},{visible},{},{},{},{},{},{},{:?},{:?},{:?},{r},{g},{b}",
                    visible - ground,
                    if wet {
                        visible.to_string()
                    } else {
                        String::new()
                    },
                    f.continentalness,
                    f.temperature,
                    f.moisture,
                    f.mountain,
                    f.biome,
                    c.ground_surface,
                    c.top_material()
                )?;
            }
            let fraction = water as f64 / rows.len() as f64;
            writeln!(
                metrics,
                "{file},{seed},{},{},{span},{n},{},{},{},{},{fraction},{},{},{},{},{:.3},{:.1}",
                terrain.source_version(),
                terrain.appearance_version(),
                low[0],
                high[0],
                low[1],
                high[1],
                low[2],
                high[2],
                low[3],
                high[3],
                elapsed.as_secs_f64() * 1000.,
                elapsed.as_nanos() as f64 / rows.len() as f64
            )?;
            for (biome, count) in &counts {
                writeln!(
                    biomes,
                    "{file},{biome},{count},{}",
                    *count as f64 / rows.len() as f64
                )?;
            }
            let native_voxel_m = voxel_size();
            fs::write(out.join(format!("{file}.json")),format!("{{\"schema\":\"frontier-seeded-world-gallery-v1\",\"recipe\":\"{recipe:?}\",\"seed\":{seed},\"source_version\":{},\"appearance_version\":{},\"span_m\":{span},\"center_xz_m\":{center:?},\"resolution\":{n},\"grid_spacing_m\":{spacing},\"native_voxel_m\":{native_voxel_m},\"params\":{{\"horizontal_scale\":1800,\"amplitude\":1000,\"octaves\":8,\"roughness\":0.5}},\"eval_ms\":{},\"method\":\"CPU canonical quantized ground and visible columns; climate at native column center; elapsed includes column and field queries and row allocation, excludes CSV IO. Static water; sparse map samples do not measure river area exactly. Not engine rendering or FPS.\"}}\n",terrain.source_version(),terrain.appearance_version(),elapsed.as_secs_f64()*1000.))?;
            println!("{file}: ground {:.1}..{:.1}m, sampled water {:.2}%, biomes {counts:?}, CPU {:.1}ms",low[0],high[0],fraction*100.,elapsed.as_secs_f64()*1000.);
        }
    }
    Ok(())
}
