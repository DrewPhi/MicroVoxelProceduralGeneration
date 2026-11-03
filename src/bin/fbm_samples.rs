//! Dense, bounded target fixture for offline fitting. No alternate noise implementation.
#[allow(dead_code)]
#[path = "../preview.rs"]
mod preview;
#[allow(dead_code)]
#[path = "../terrain.rs"]
mod terrain;
use rayon::prelude::*;
use std::{
    io::{BufWriter, Write},
    path::PathBuf,
};
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: fbm_samples RECIPE.json OUTPUT_DIRECTORY".into());
    }
    let recipe: preview::Recipe = serde_json::from_slice(&std::fs::read(&args[0])?)?;
    recipe.validate()?;
    let t = terrain::Terrain::new(recipe.terrain.clone());
    let s = t.p.voxel_m();
    let n = (recipe.inspection.span / s).round() as usize;
    if n == 0 || n > 1024 {
        return Err("Fixture must have 1..1024 cells per axis".into());
    }
    let center = [
        recipe.inspection.x,
        t.base_height(recipe.inspection.x, recipe.inspection.z),
        recipe.inspection.z,
    ];
    let origin = center.map(|v| (v / s).floor() as i64 - n as i64 / 2);
    let out = PathBuf::from(&args[1]);
    std::fs::create_dir_all(&out)?;
    let meta = serde_json::json!({"version":1,"recipe":recipe,"origin_cells":origin,"dimensions":[n,n,n],"voxel_m":s,"order":"C-order [y,z,x]; x fastest","density":"positive solid; source Rust fbm v1; outside world air","dtype":"little-endian f32"});
    std::fs::write(out.join("target.json"), serde_json::to_vec_pretty(&meta)?)?;
    let mut file = BufWriter::new(std::fs::File::create(out.join("density.f32.tmp"))?);
    let start = std::time::Instant::now();
    for y in 0..n {
        let layer: Vec<f32> = (0..n * n)
            .into_par_iter()
            .map(|a| {
                let c = [
                    origin[0] + (a % n) as i64,
                    origin[1] + y as i64,
                    origin[2] + (a / n) as i64,
                ];
                let p = c.map(|v| (v as f64 + 0.5) * s);
                if p[0] < 0.
                    || p[2] < 0.
                    || p[1] < 0.
                    || p[0] >= t.p.world_m
                    || p[2] >= t.p.world_m
                    || p[1] >= t.p.ceiling_m
                {
                    -1.0e6
                } else {
                    t.density(p) as f32
                }
            })
            .collect();
        for v in layer {
            file.write_all(&v.to_le_bytes())?;
        }
        if y % 16 == 0 {
            println!(
                "Export layer {y}/{n}; {:.1}s",
                start.elapsed().as_secs_f64()
            );
        }
    }
    file.flush()?;
    drop(file);
    std::fs::rename(out.join("density.f32.tmp"), out.join("density.f32"))?;
    println!(
        "Target ready: {n}³ cells in {:.1}s",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
