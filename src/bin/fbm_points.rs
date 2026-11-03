//! Sparse global samples for continuity-preserving analytic reconstruction.
#[allow(dead_code)]
#[path = "../terrain.rs"]
mod terrain;
use std::io::Write;
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.len() != 3 {
        return Err("usage: fbm_points RECIPE.json POINTS.json OUTPUT.f64".into());
    }
    let recipe: serde_json::Value = serde_json::from_slice(&std::fs::read(&a[0])?)?;
    let params: terrain::Params = serde_json::from_value(recipe["terrain"].clone())?;
    params.validate()?;
    let t = terrain::Terrain::new(params);
    let points: Vec<[f64; 3]> = serde_json::from_slice(&std::fs::read(&a[1])?)?;
    if points.len() > 10_000_000 || points.iter().flatten().any(|v| !v.is_finite()) {
        return Err("Invalid sparse points".into());
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(&a[2])?);
    for p in points {
        out.write_all(&t.density(p).to_le_bytes())?;
    }
    out.flush()?;
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
