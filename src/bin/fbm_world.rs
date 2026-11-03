//! Finite authoring-region recipe and exact native-column top-down diagnostic.
//! This samples the existing global FBM; no fitting or cluster submission.
#[allow(dead_code)]
#[path = "../terrain.rs"]
mod terrain;
use rayon::prelude::*;
use std::{
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};
fn png(path: PathBuf, w: usize, h: usize, pixels: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut e = png::Encoder::new(BufWriter::new(fs::File::create(path)?), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(pixels)?;
    Ok(())
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let val = |name: &str, default: &str| {
        args.iter()
            .position(|s| s == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.into())
    };
    let input = val("--recipe", "output/fit-occupancy/recipe.json");
    let out = PathBuf::from(val("--output", "output/world-1km"));
    let w: usize = val("--width", "1024").parse()?;
    let size: f64 = val("--size", "1000").parse()?;
    let recipe: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
    let params: terrain::Params = serde_json::from_value(recipe["terrain"].clone())?;
    params.validate()?;
    if !(64..=2048).contains(&w) || !size.is_finite() || size <= 0. || size > params.world_m {
        return Err("Invalid preview size or region size".into());
    }
    let t = terrain::Terrain::new(params);
    let center = [
        recipe["inspection"]["x"].as_f64().unwrap_or(5000.),
        recipe["inspection"]["z"].as_f64().unwrap_or(5000.),
    ];
    let s = t.p.voxel_m();
    let patch = 8.;
    let x0 = (((center[0] - size / 2.) / patch).floor() * patch).clamp(0., t.p.world_m - size);
    let z0 = (((center[1] - size / 2.) / patch).floor() * patch).clamp(0., t.p.world_m - size);
    if (size / patch - size.div_euclid(patch)).abs() > 1e-8 {
        return Err("Region size must be a multiple of 8 meters".into());
    }
    fs::create_dir_all(&out)?;
    let start = Instant::now();
    let px = size / w as f64;
    println!(
        "Sampling {size}m x {size}m, seed {}, {}cm voxels; {w}² native columns",
        t.p.seed, t.p.voxel_cm
    );
    // OpenSimplex outputs are bounded by [-1,1]; normalized FBM preserves this.
    // Uncarved density is negative above base + overhang. Caves only remove solid.
    // Thus scanning down from this bound finds the highest occupied native cell.
    let heights: Vec<f32> = (0..w * w)
        .into_par_iter()
        .map(|id| {
            let x = ((x0 + (id % w) as f64 * px + px / 2.) / s).floor() * s + s / 2.;
            let z = ((z0 + (id / w) as f64 * px + px / 2.) / s).floor() * s + s / 2.;
            let base = t.base_height(x, z);
            let first = ((base + t.p.overhang_m) / s).ceil() as i64 + 1;
            for y in (0..=first.min((t.p.ceiling_m / s).floor() as i64 - 1)).rev() {
                if t.density_with_base_height([x, (y as f64 + 0.5) * s, z], base) > 0. {
                    return ((y + 1) as f64 * s) as f32;
                }
            }
            0.
        })
        .collect();
    let min = heights.iter().copied().fold(f32::INFINITY, f32::min) as f64;
    let max = heights.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    let bottom = ((min - 96.) / patch).floor().max(0.) * patch;
    let top = (((max + 24.) / patch).ceil() * patch).min(t.p.ceiling_m);
    let yspan = top - bottom;
    let mut file = BufWriter::new(fs::File::create(out.join("surface-heights.f32"))?);
    for v in &heights {
        file.write_all(&v.to_le_bytes())?;
    }
    file.flush()?;
    let mut pixels = vec![0u8; w * w * 3];
    pixels.par_chunks_mut(3).enumerate().for_each(|(id, rgb)| {
        let x = id % w;
        let z = id / w;
        let hx = (heights[z * w + (x + 1).min(w - 1)] - heights[z * w + x.saturating_sub(1)])
            as f64
            / (px * if x == 0 || x == w - 1 { 1. } else { 2. });
        let hz = (heights[(z + 1).min(w - 1) * w + x] - heights[z.saturating_sub(1) * w + x])
            as f64
            / (px * if z == 0 || z == w - 1 { 1. } else { 2. });
        let slope = (hx * hx + hz * hz).sqrt();
        let normal = [-hx, 1., -hz];
        let norm = (1. + slope * slope).sqrt();
        let sun = (-0.45 * normal[0] + 0.82 * normal[1] - 0.35 * normal[2]) / norm;
        let light = 0.42 + 0.58 * sun.max(0.);
        let rock = ((slope - 0.55) / 0.9).clamp(0., 1.);
        let elevation = ((heights[id] as f64 - min) / (max - min).max(1.)).clamp(0., 1.);
        let grass = [
            83. + 22. * elevation,
            125. + 20. * elevation,
            61. + 14. * elevation,
        ];
        let stone = [153., 147., 135.];
        for a in 0..3 {
            rgb[a] = ((grass[a] * (1. - rock) + stone[a] * rock) * light).clamp(0., 255.) as u8;
        }
    });
    png(out.join("top-down.png"), w, w, &pixels)?;
    // One vertical cut demonstrates the same field's cavities, not just a heightmap.
    let sh = 512;
    let mut slice = vec![0u8; w * sh * 3];
    slice
        .par_chunks_mut(sh * 3)
        .enumerate()
        .for_each(|(x, column)| {
            let xx = ((x0 + (x as f64 + 0.5) * px) / s).floor() * s + s / 2.;
            let zz = ((z0 + size / 2.) / s).floor() * s + s / 2.;
            let base = t.base_height(xx, zz);
            for (row, color) in column.chunks_mut(3).enumerate() {
                let yy = ((top - (row as f64 + 0.5) * yspan / sh as f64) / s).floor() * s + s / 2.;
                let solid = t.density_with_base_height([xx, yy, zz], base) > 0.;
                color.copy_from_slice(if solid {
                    &[142, 143, 145]
                } else {
                    &[28, 39, 53]
                });
            }
        });
    let mut rows = vec![0u8; w * sh * 3];
    for x in 0..w {
        for y in 0..sh {
            rows[(y * w + x) * 3..(y * w + x + 1) * 3]
                .copy_from_slice(&slice[(x * sh + y) * 3..(x * sh + y + 1) * 3]);
        }
    }
    png(out.join("vertical-slice.png"), w, sh, &rows)?;
    let origin = [
        (x0 / s).round() as i64,
        (bottom / s).round() as i64,
        (z0 / s).round() as i64,
    ];
    let dims = [
        (size / s).round() as u64,
        (yspan / s).round() as u64,
        (size / s).round() as u64,
    ];
    let manifest = serde_json::json!({"version":1,"stage":"unfitted_authoring_region","terrain":t.p,"bounds_m":{"min":[x0,bottom,z0],"max":[x0+size,top,z0+size]},"extent_m":[size,yspan,size],"origin_cells":origin,"dimensions":dims,"voxel_m":s,"proposed_patch_edge_m":8.,"proposed_quadratics_per_patch":5,"surface_height_range_m":[min,max],"vertical_margin_m":{"below_sampled_min":min-bottom,"above_sampled_max":top-max},"below_region":"solid for future rendering; not materialized","above_region":"air","preview":{"width":w,"height":w,"meters_per_pixel":px,"height_file":"surface-heights.f32","height_format":"little-endian f32; rows +Z, columns +X; native top face height","sampling":"Highest solid native voxel in each sampled XZ column; unsampled columns are not exhaustive bounds","color":"Slope-based grass/rock diagnostic with directional hillshade; not fitted biome materials","vertical_slice_z_m":z0+size/2.},"elapsed_seconds":start.elapsed().as_secs_f64(),"note":"Global FBM recipe + finite bounds define the full 3D solid/air target, including caves and overhangs. No native dense volume, quadratic fit or GPU/cluster job generated."});
    fs::write(
        out.join("world.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::write(out.join("recipe.json"), serde_json::to_vec_pretty(&recipe)?)?;
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("fbm_world: {e}");
        std::process::exit(1);
    }
}
