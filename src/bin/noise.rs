use microvoxel::{
    generation::{ProceduralTerrain, TerrainConfig},
    grid::CellPos,
    Result, World,
};
use std::{
    env,
    fs::File,
    io::{BufWriter, Write},
    time::Instant,
};
fn grid(value: f64) -> Result<i64> {
    if !value.is_finite() || ((value * 16.0).round() - value * 16.0).abs() > 1e-6 {
        return Err("sizes/origins must align to the 1/16-meter grid".into());
    }
    Ok((value * 16.0).round() as i64)
}
fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() > 7 {
        return Err(
            "noise [spacing_m] [size_m] [seed] [prefix] [origin_x_m] [origin_z_m] [climate]".into(),
        );
    }
    let number = |i, default| -> Result<f64> {
        Ok(args.get(i).map_or(Ok(default), |s: &String| s.parse())?)
    };
    let step = grid(number(0, 4.0)?)?;
    let size = grid(number(1, 2048.0)?)?;
    let ox = grid(number(4, 0.0)?)?;
    let oz = grid(number(5, 0.0)?)?;
    if step < 1
        || size < 1
        || ox < 0
        || oz < 0
        || ox.checked_add(size).ok_or("overflow")? > 160000
        || oz.checked_add(size).ok_or("overflow")? > 160000
    {
        return Err("bounds must lie in the current 10 km terrain region".into());
    }
    let side = (size + step - 1) / step;
    if side > 4096 {
        return Err(
            "image limit: 4096 samples per side; sample a smaller region at native spacing".into(),
        );
    }
    let seed = args.get(2).map_or(Ok(42u64), |s| s.parse())?;
    let prefix = args.get(3).map_or("terrain", String::as_str);
    let start = Instant::now();
    let config =
        TerrainConfig::for_climate(args.get(6).map_or(Ok(Default::default()), |s| s.parse())?);
    let world = World::new(ProceduralTerrain::with_config(seed, config)?);
    let generation = start.elapsed();
    let count = (side * side) as usize;
    let mut terrain = Vec::with_capacity(count * 2);
    let mut water = Vec::with_capacity(count * 2);
    let mut bedrock = Vec::with_capacity(count * 2);
    let mut materials = Vec::with_capacity(count);
    for z in 0..side {
        for x in 0..side {
            let gx = ox + x * step;
            let gz = oz + z * step;
            let c = world.surface_column(gx, gz).ok_or("unprovided column")?;
            for (bytes, level) in [
                (&mut terrain, c.ground),
                (&mut water, if c.water > c.ground { c.water } else { 0 }),
                (&mut bedrock, c.bedrock),
            ] {
                let value = (level as f64 / 16384.0 * 65535.0)
                    .round()
                    .clamp(0.0, 65535.0) as u16;
                bytes.extend_from_slice(&value.to_be_bytes());
            }
            materials.push(
                world
                    .voxel_at(CellPos::new(gx, c.ground - 1, gz))
                    .known()
                    .ok_or("unprovided surface")?
                    .material
                    .0 as u8,
            );
        }
    }
    for (suffix, max, pixels) in [
        ("", 65535, &terrain),
        ("_water", 65535, &water),
        ("_bedrock", 65535, &bedrock),
        ("_material", 255, &materials),
    ] {
        let path = format!("{prefix}{suffix}.pgm");
        let mut file = BufWriter::new(File::create(&path)?);
        writeln!(
            file,
            "P5\n# canonical grid: 16 cells/m; elevation white = 1024 m\n{side} {side}\n{max}"
        )?;
        file.write_all(pixels)?;
        file.flush()?;
        println!("Saved {path}");
    }
    println!(
        "{side} x {side}; canonical cells independent of output spacing; region {:?}; total {:?}",
        generation,
        start.elapsed()
    );
    Ok(())
}
