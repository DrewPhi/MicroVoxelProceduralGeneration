//! Repaint saved experimental landscapes without regenerating geometry.
use microvoxel::generation::{
    atlas_climate::Cell,
    atlas_hydrology::SECONDS_PER_YEAR,
    landscape_paint::{contexts, contexts_with_climate, sample, voxel_rgb},
    ClimatePreset, TerrainConfig,
};
use std::{
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or(
        "usage: landscape_paint DIRECTORY [temperate|arid|subpolar|continental|tropical]",
    )?);
    let requested_preset = args.next();
    let preset = requested_preset
        .as_deref()
        .unwrap_or("temperate")
        .parse::<ClimatePreset>()?;
    let metadata = fs::read_to_string(path.join("summary.txt"))?;
    let value = |key: &str| {
        metadata
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .ok_or("missing metadata")
    };
    let side: usize = value("side")?.parse()?;
    let extent: f64 = value("extent_m")?.parse()?;
    let seed: u64 = value("seed")?.parse()?;
    let input = fs::read(path.join("nodes.bin"))?;
    if input.len() != side * side * 64 {
        return Err("invalid nodes size".into());
    }
    let nodes: Vec<[f64; 8]> = input
        .chunks_exact(64)
        .map(|b| {
            std::array::from_fn(|i| f64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap()))
        })
        .collect();
    let input = fs::read(path.join("triangles.bin"))?;
    if input.len() % 12 != 0 {
        return Err("invalid triangle size".into());
    }
    let triangles: Vec<[u32; 3]> = input
        .chunks_exact(12)
        .map(|b| {
            std::array::from_fn(|i| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .collect();
    if triangles
        .iter()
        .flatten()
        .any(|&i| i as usize >= nodes.len())
    {
        return Err("triangle outside nodes".into());
    }
    let input = fs::read(path.join("water.bin"))?;
    if input.len() != nodes.len() * 8 {
        return Err("invalid water size".into());
    }
    let water: Vec<_> = input
        .chunks_exact(8)
        .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let start = Instant::now();
    let compiled = metadata
        .lines()
        .any(|l| l == "climate_model=ocean_transport_v1");
    let contexts = if compiled {
        if requested_preset.is_some() {
            return Err(
                "compiled atlas climate is authoritative; regenerate to change its settings".into(),
            );
        }
        let input = fs::read(path.join("climate.bin"))?;
        if input.len() != nodes.len() * 32 {
            return Err("invalid climate size".into());
        }
        let cells: Vec<_> = input
            .chunks_exact(32)
            .map(|b| {
                Cell::from_fields(std::array::from_fn(|i| {
                    f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())
                }))
            })
            .collect();
        let input = fs::read(path.join("flow.bin"))?;
        if input.len() != nodes.len() * 16 {
            return Err("invalid flow size".into());
        }
        let discharge: Vec<_> = input
            .chunks_exact(16)
            .map(|b| f64::from_le_bytes(b[..8].try_into().unwrap()) * SECONDS_PER_YEAR)
            .collect();
        contexts_with_climate(&nodes, &triangles, &water, &cells, Some(&discharge))
    } else {
        contexts(
            &nodes,
            &triangles,
            &water,
            side,
            extent,
            seed,
            TerrainConfig::for_climate(preset),
        )
    };
    let mut output = BufWriter::new(fs::File::create(path.join("paint.bin"))?);
    let mut coverage = [0f64; 5];
    for (node, &c) in nodes.iter().zip(&contexts) {
        let p = sample([node[0], node[1]], c, extent / (side - 1) as f64, seed);
        for (a, w) in coverage.iter_mut().zip(p.coverage) {
            *a += w as f64 / nodes.len() as f64;
        }
        for v in p
            .linear_rgb
            .into_iter()
            .chain([c.temperature as f32, c.moisture as f32])
            .chain(p.coverage)
        {
            output.write_all(&v.to_le_bytes())?;
        }
    }
    output.flush()?;
    // Four flat 8m swatches at native 6.25cm resolution. One pixel = one voxel;
    // frozen representative contexts isolate color detail from terrain geometry.
    let choose = |score: fn(&microvoxel::generation::landscape_paint::Context) -> f64| {
        *contexts
            .iter()
            .max_by(|a, b| score(a).total_cmp(&score(b)))
            .unwrap()
    };
    let swatches = [
        choose(|c| c.moisture - c.gradient[0].hypot(c.gradient[1])),
        choose(|c| -c.moisture),
        choose(|c| c.gradient[0].hypot(c.gradient[1])),
        choose(|c| -c.warm_temperature),
    ];
    let mut swatch = BufWriter::new(fs::File::create(path.join("paint-swatches.ppm"))?);
    swatch.write_all(b"P6\n512 128\n255\n")?;
    for z in 0..128 {
        for x in 0..512 {
            let mut context = swatches[x / 128];
            context.water_depth_m = 0.;
            let color = voxel_rgb(
                microvoxel::CellPos::new((x % 128) as i64, 0, z),
                microvoxel::GridScale::default(),
                context,
                seed,
            );
            swatch.write_all(&color)?;
        }
    }
    let climate_name = if compiled {
        "ocean_transport_v1".to_owned()
    } else {
        preset.to_string()
    };
    let report=format!("climate={climate_name}\npaint_seconds={:.4}\ncoverage_mean_nodes={coverage:?}\nformat=10 little-endian f32 per node: linear RGB, temperature, moisture, vegetation/rock/sediment/snow/wetground weights\n",start.elapsed().as_secs_f64());
    fs::write(path.join("paint.txt"), &report)?;
    println!("{report}");
    Ok(())
}
