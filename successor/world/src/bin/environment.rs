//! Export long-term climate/landform diagnostics without a GPU or dependencies.
use microvoxel::generation::{debug::DebugLayer, ClimatePreset, ProceduralTerrain, TerrainConfig};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
    time::Instant,
};
fn main() -> microvoxel::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--help") || args.len() > 5 {
        println!("environment [seed=42] [climate=temperate] [output_dir=output/environment] [wind_degrees=25] [boundary_moisture=preset default]");
        return Ok(());
    }
    let seed = args.first().map_or(Ok(42u64), |s| s.parse())?;
    let preset = args
        .get(1)
        .map_or(Ok(ClimatePreset::Temperate), |s| s.parse())?;
    let mut config = TerrainConfig::for_climate(preset);
    if let Some(v) = args.get(3) {
        config.wind_degrees = v.parse()?;
    }
    if let Some(v) = args.get(4) {
        config.boundary_moisture = v.parse()?;
    }
    let directory = Path::new(args.get(2).map_or("output/environment", String::as_str));
    fs::create_dir_all(directory)?;
    let start = Instant::now();
    let terrain = ProceduralTerrain::with_config(seed, config)?;
    let summary=format!("Seed {seed}, climate {preset}, wind towards {:.1} degrees, boundary moisture {:.2}\nRegional generation {:?}; {:.2} MiB of regional data (excluding column cache/scratch)\n{}",config.wind_degrees,config.boundary_moisture,start.elapsed(),terrain.regional_memory_bytes() as f64/1048576.,terrain.environment_statistics());
    println!("{summary}");
    fs::write(directory.join("summary.txt"), &summary)?;
    let mut legend=String::from("Regional diagnostic maps: 10 km square; north/top is z=0; x increases right.\nColors are diagnostics, not voxel palettes.\n\n");
    for layer in DebugLayer::ALL {
        let image = terrain.debug_image(layer, 512)?;
        let path = directory.join(format!("{layer}.ppm"));
        let mut file = BufWriter::new(File::create(path)?);
        writeln!(file, "P6\n{} {}\n255", image.size, image.size)?;
        file.write_all(&image.rgb)?;
        legend.push_str(&format!("{layer}: {}\n", layer.legend()));
    }
    fs::write(directory.join("legend.txt"), legend)?;
    println!("Saved 12 maps and legends to {}", directory.display());
    Ok(())
}
