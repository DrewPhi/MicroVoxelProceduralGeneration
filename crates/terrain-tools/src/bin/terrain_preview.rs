//! Render the implicit 3D source terrain to PNGs for tuning parameters.
//!
//! Usage: terrain_preview [--config FILE] [--watch] [--KEY VALUE ...]
//! Writes into `out`:
//! - `height.png`    top-surface height, normalized grayscale;
//! - `shaded.png`    top surface lit with the analytic density normal;
//! - `overhangs.png` shaded map with overhang columns (air under the top) in red;
//! - `iso.png`       isometric 3D voxel view (when `iso = true`);
//! - `iso_fit.png`   the same view of the fitted `min of n quadrics` terrain,
//!   and `fit_error.png`, a per-patch voxel-mismatch heatmap (when `fit = true`);
//!
//! and prints stats and a patch-size hint.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use terrain_core::fit::{FitParams, FittedTerrain, Patch, fit_patch};
use terrain_core::{FbmParams, OverhangParams, Terrain, TerrainParams};
use terrain_tools::color::{ramp, write_png};
use terrain_tools::iso::{self, IsoSettings, Shading};
use terrain_tools::par_map;

#[derive(Clone, Debug)]
struct Preview {
    center: [f64; 2],
    extent: f64,
    resolution: u32,
    voxel_size: f64,
    scan_step: f64,
    sun_azimuth: f64,
    sun_elevation: f64,
    contour: f64,
    out: PathBuf,
    iso: bool,
    iso_width: u32,
    iso_extent: f64,
    iso_max_columns: u32,
    iso_yaw: f64,
    iso_pitch: f64,
    iso_exaggeration: f64,
    iso_shading: Shading,
    iso_shadows: bool,
    iso_mark_floaters: bool,
    fit: bool,
    fit_sparse: bool,
    fit_params: FitParams,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            extent: 4096.0,
            resolution: 1024,
            voxel_size: 0.25,
            scan_step: 0.5,
            sun_azimuth: 250.0,
            sun_elevation: 35.0,
            contour: 0.0,
            out: PathBuf::from("output/terrain"),
            iso: true,
            iso_width: 1600,
            iso_extent: 64.0,
            iso_max_columns: 512,
            iso_yaw: 225.0,
            iso_pitch: 35.264,
            iso_exaggeration: 1.0,
            iso_shading: Shading::Auto,
            iso_shadows: true,
            iso_mark_floaters: true,
            fit: true,
            fit_sparse: true,
            fit_params: FitParams::default(),
        }
    }
}

impl Preview {
    const KEYS: &'static str = "center_x center_z extent resolution voxel_size scan_step \
        sun_azimuth sun_elevation contour out iso iso_width iso_extent iso_max_columns iso_yaw \
        iso_pitch iso_exaggeration iso_shading iso_shadows iso_mark_floaters fit fit_sparse";

    fn set(&mut self, key: &str, value: &str) -> Result<bool, String> {
        let num = |v: &str| -> Result<f64, String> {
            v.parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(|| format!("{key}: cannot parse {v:?}"))
        };
        let int = |v: &str| -> Result<u32, String> {
            v.parse().map_err(|_| format!("{key}: cannot parse {v:?}"))
        };
        let flag = |v: &str| -> Result<bool, String> {
            v.parse()
                .map_err(|_| format!("{key}: expected true or false, got {v:?}"))
        };
        match key {
            "center_x" => self.center[0] = num(value)?,
            "center_z" => self.center[1] = num(value)?,
            "extent" => self.extent = num(value)?,
            "resolution" => self.resolution = int(value)?,
            "voxel_size" => self.voxel_size = num(value)?,
            "scan_step" => self.scan_step = num(value)?,
            "sun_azimuth" => self.sun_azimuth = num(value)?,
            "sun_elevation" => self.sun_elevation = num(value)?,
            "contour" => self.contour = num(value)?,
            "out" => self.out = PathBuf::from(value),
            "iso" => self.iso = flag(value)?,
            "iso_width" => self.iso_width = int(value)?,
            "iso_extent" => self.iso_extent = num(value)?,
            "iso_max_columns" => self.iso_max_columns = int(value)?,
            "iso_yaw" => self.iso_yaw = num(value)?,
            "iso_pitch" => self.iso_pitch = num(value)?,
            "iso_exaggeration" => self.iso_exaggeration = num(value)?,
            "iso_shading" => self.iso_shading = value.parse()?,
            "iso_shadows" => self.iso_shadows = flag(value)?,
            "iso_mark_floaters" => self.iso_mark_floaters = flag(value)?,
            "fit" => self.fit = flag(value)?,
            "fit_sparse" => self.fit_sparse = flag(value)?,
            k if self.fit_params.set(k, value)? => {}
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn validate(&self) -> Result<(), String> {
        let positive = [
            ("extent", self.extent),
            ("voxel_size", self.voxel_size),
            ("scan_step", self.scan_step),
        ];
        for (name, v) in positive {
            if v <= 0.0 {
                return Err(format!("{name} must be > 0"));
            }
        }
        if !(16..=8192).contains(&self.resolution) {
            return Err("resolution must be 16..=8192".into());
        }
        if self.contour < 0.0 {
            return Err("contour must be >= 0".into());
        }
        Ok(())
    }
}

struct Args {
    config: PathBuf,
    watch: bool,
    overrides: Vec<(String, String)>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        config: PathBuf::from("params/terrain.conf"),
        watch: false,
        overrides: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--watch" => args.watch = true,
            "--help" | "-h" => {
                println!(
                    "terrain_preview [--config FILE] [--watch] [--KEY VALUE ...]\n\
                     surface keys:  {}\n\
                     overhang keys: {}\n\
                     preview keys:  {}\n\
                     fit keys:      {}",
                    FbmParams::KEYS.join(" "),
                    OverhangParams::KEYS.join(" "),
                    Preview::KEYS,
                    FitParams::KEYS.join(" ")
                );
                std::process::exit(0);
            }
            "--config" => {
                args.config = PathBuf::from(it.next().ok_or("--config needs a path")?);
            }
            flag if flag.starts_with("--") => {
                let key = flag.trim_start_matches("--").replace('-', "_");
                let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
                args.overrides.push((key, value));
            }
            other => return Err(format!("unexpected argument {other:?} (try --help)")),
        }
    }
    Ok(args)
}

/// Read `key = value` lines (with `#` comments), then apply CLI overrides.
fn load(args: &Args) -> Result<(TerrainParams, Preview), String> {
    let mut terrain = TerrainParams::default();
    let mut preview = Preview::default();
    let mut pairs = Vec::new();
    if args.config.exists() {
        let text = std::fs::read_to_string(&args.config)
            .map_err(|e| format!("{}: {e}", args.config.display()))?;
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line.split_once('=').ok_or_else(|| {
                format!(
                    "{}:{}: expected `key = value`",
                    args.config.display(),
                    n + 1
                )
            })?;
            pairs.push((k.trim().to_string(), v.trim().to_string()));
        }
    } else {
        eprintln!("note: {} not found, using defaults", args.config.display());
    }
    pairs.extend(args.overrides.iter().cloned());
    for (k, v) in &pairs {
        if !terrain.set(k, v)? && !preview.set(k, v)? {
            return Err(format!("unknown parameter {k:?} (try --help)"));
        }
    }
    terrain.validate()?;
    preview.validate()?;
    Ok((terrain, preview))
}

#[derive(Clone, Copy, Default)]
struct Pixel {
    top: f64,
    normal: [f64; 3],
    overhang: bool,
}

fn render(params: &TerrainParams, p: &Preview) -> Result<(), String> {
    let start = Instant::now();
    let terrain = Terrain::new(params.clone())?;
    let n = p.resolution as usize;
    let step = p.extent / n as f64;
    let (x0, z0) = (p.center[0] - p.extent / 2.0, p.center[1] - p.extent / 2.0);
    let pixels = par_map(n * n, |k| {
        let (col, row) = (k % n, k / n);
        // Image row 0 is the north (+z) edge.
        let x = x0 + (col as f64 + 0.5) * step;
        let z = z0 + p.extent - (row as f64 + 0.5) * step;
        let scan = terrain.scan(&terrain.column(x, z), p.scan_step);
        let g = scan.gradient;
        let len = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        Pixel {
            top: scan.top,
            normal: [-g[0] / len, -g[1] / len, -g[2] / len],
            overhang: scan.overhang,
        }
    });
    let scan_time = start.elapsed();

    let (min, max) = pixels.iter().fold((f64::MAX, f64::MIN), |(lo, hi), px| {
        (lo.min(px.top), hi.max(px.top))
    });
    let mean = pixels.iter().map(|px| px.top).sum::<f64>() / pixels.len() as f64;
    let span = (max - min).max(1e-9);
    let overhang_frac = pixels.iter().filter(|px| px.overhang).count() as f64 / pixels.len() as f64;
    let slope_deg = |px: &Pixel| px.normal[1].clamp(-1.0, 1.0).acos().to_degrees();
    let max_slope = pixels.iter().map(slope_deg).fold(0.0, f64::max);
    let mean_slope = pixels.iter().map(slope_deg).sum::<f64>() / pixels.len() as f64;

    let (az, el) = (p.sun_azimuth.to_radians(), p.sun_elevation.to_radians());
    let sun = [az.sin() * el.cos(), el.sin(), az.cos() * el.cos()];

    let gray: Vec<u8> = pixels
        .iter()
        .map(|px| ((px.top - min) / span * 255.0).round() as u8)
        .collect();
    let mut shaded = Vec::with_capacity(n * n * 3);
    let mut overhangs = Vec::with_capacity(n * n * 3);
    for px in &pixels {
        let nn = px.normal;
        let lambert = (nn[0] * sun[0] + nn[1] * sun[1] + nn[2] * sun[2]).max(0.0);
        let mut light = 0.25 + 0.75 * lambert;
        if p.contour > 0.0 {
            let h = px.top / p.contour;
            let slope = (1.0 - nn[1] * nn[1]).max(0.0).sqrt() / nn[1].abs().max(1e-3);
            if (h - h.round()).abs() * p.contour < 0.5 * step * slope.max(1e-6) {
                light *= 0.55;
            }
        }
        let c = ramp((px.top - min) / span);
        shaded.extend(c.map(|v| (v * light * 255.0).clamp(0.0, 255.0) as u8));
        let g = 0.35 + 0.5 * light;
        let o = if px.overhang {
            [1.0, 0.15, 0.1]
        } else {
            [g, g, g]
        };
        overhangs.extend(o.map(|v: f64| (v * 255.0).clamp(0.0, 255.0) as u8));
    }

    std::fs::create_dir_all(&p.out).map_err(|e| format!("{}: {e}", p.out.display()))?;
    let res = p.resolution;
    write_png(
        &p.out.join("height.png"),
        res,
        res,
        &gray,
        png::ColorType::Grayscale,
    )?;
    write_png(
        &p.out.join("shaded.png"),
        res,
        res,
        &shaded,
        png::ColorType::Rgb,
    )?;
    write_png(
        &p.out.join("overhangs.png"),
        res,
        res,
        &overhangs,
        png::ColorType::Rgb,
    )?;

    let s = &params.surface;
    let o = &params.overhang;
    let shortest_3d = o.wavelength / o.lacunarity.powi(o.octaves as i32 - 1);
    let shortest = s.shortest_wavelength().min(shortest_3d);
    let cell = shortest / 4.0;
    println!(
        "seed {} | 2D: {} octaves {:.1} m .. {:.2} m, ±{:.0} m | 3D: {} octaves {:.1} m .. {:.2} m, ±{:.1} m, squash {}, coverage {}",
        s.seed,
        s.octaves,
        s.base_wavelength,
        s.shortest_wavelength(),
        s.amplitude,
        o.octaves,
        o.wavelength,
        shortest_3d,
        terrain.max_shell(),
        o.squash,
        o.coverage
    );
    println!(
        "map {:.0} m @ {step:.2} m/px | top surface min {min:.1} m max {max:.1} m mean {mean:.1} m | slope mean {mean_slope:.1}° max {max_slope:.1}°",
        p.extent
    );
    println!(
        "overhang columns {:.2}% (scan step {} m) | fit hint: cells ≤ {cell:.2} m ({} voxels of {} m)",
        overhang_frac * 100.0,
        p.scan_step,
        (cell / p.voxel_size).floor().max(1.0),
        p.voxel_size
    );
    println!(
        "maps -> {}/{{height,shaded,overhangs}}.png in {:.0} ms",
        p.out.display(),
        scan_time.as_secs_f64() * 1e3
    );

    if p.iso {
        let iso_start = Instant::now();
        let settings = IsoSettings {
            center: p.center,
            extent: if p.iso_extent > 0.0 {
                p.iso_extent
            } else {
                p.extent
            },
            width: p.iso_width,
            max_columns: p.iso_max_columns,
            voxel_size: p.voxel_size,
            yaw: p.iso_yaw,
            pitch: p.iso_pitch,
            exaggeration: p.iso_exaggeration,
            shading: p.iso_shading,
            shadows: p.iso_shadows,
            mark_floaters: p.iso_mark_floaters,
            sun,
        };
        let img = iso::render(&terrain, &settings)?;
        let path = p.out.join("iso.png");
        write_png(&path, img.width, img.height, &img.rgb, png::ColorType::Rgb)?;
        let floating = match img.floating {
            Some(0) => "no floating voxels".to_string(),
            Some(f) => format!("{f} floating voxels (magenta)"),
            None => "floaters not counted".to_string(),
        };
        println!(
            "iso {:.0} m square: {} columns of {} m voxels, {} faces, {floating} -> {} ({}x{}) in {:.0} ms",
            settings.extent,
            img.columns,
            img.voxel,
            img.faces,
            path.display(),
            img.width,
            img.height,
            iso_start.elapsed().as_secs_f64() * 1e3
        );
        if p.fit {
            fit_preview(&terrain, p, &settings)?;
        }
    }
    Ok(())
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn run_once(args: &Args) -> Result<(), String> {
    let (terrain, preview) = load(args)?;
    render(&terrain, &preview)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let result = run_once(&args);
    if !args.watch {
        if let Err(e) = result {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = result {
        eprintln!("error: {e}");
    }
    println!("watching {} (Ctrl+C to stop)", args.config.display());
    let mut last = mtime(&args.config);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let now = mtime(&args.config);
        if now != last {
            last = now;
            println!("---");
            if let Err(e) = run_once(&args) {
                eprintln!("error: {e}");
            }
        }
    }
}

/// Fit `min of n quadrics` patches over the iso region, render the fitted
/// terrain with the same view, and write a per-patch error heatmap.
fn fit_preview(terrain: &Terrain, p: &Preview, settings: &IsoSettings) -> Result<(), String> {
    let start = Instant::now();
    let fp = FitParams {
        voxel_size: p.voxel_size,
        ..p.fit_params.clone()
    };
    fp.validate()?;
    let half = 0.5 * settings.extent;
    let (pi0, pj0, ni, nj) = FittedTerrain::region(
        &fp,
        settings.center[0] - half,
        settings.center[1] - half,
        settings.center[0] + half,
        settings.center[1] + half,
    );
    // The iso view samples one density column per preview voxel. When those
    // are coarser than the patches, only fit the patches the view touches.
    let view = iso::column_grid(settings);
    let mut needed = vec![!p.fit_sparse; ni * nj];
    let mut touched = Vec::with_capacity(view.n * view.n);
    for j in 0..view.n {
        for i in 0..view.n {
            let (x, z) = view.center(i, j);
            let s = fp.patch_size();
            let (a, b) = ((x / s).floor() as i64 - pi0, (z / s).floor() as i64 - pj0);
            let idx = (b as usize) * ni + a as usize;
            needed[idx] = true;
            touched.push(idx);
        }
    }
    let patches = par_map(ni * nj, |k| {
        let (pi, pj) = (pi0 + (k % ni) as i64, pj0 + (k / ni) as i64);
        Some(if needed[k] {
            fit_patch(terrain, &fp, pi, pj)
        } else {
            // Never sampled by the view: an empty placeholder.
            Patch {
                center: [0.0; 3],
                half: [1.0; 3],
                y_lo: 0.0,
                y_hi: 0.0,
                quadrics: Vec::new(),
                voxels: 0,
                mismatched: 0,
                surface: 0,
            }
        })
    });
    let fitted = FittedTerrain {
        params: fp.clone(),
        pi0,
        pj0,
        ni,
        nj,
        patches: patches.into_iter().map(Option::unwrap).collect(),
    };
    let fit_time = start.elapsed();

    let voxels: usize = fitted.patches.iter().map(|q| q.voxels).sum();
    let wrong: usize = fitted.patches.iter().map(|q| q.mismatched).sum();
    let surface: usize = fitted.patches.iter().map(|q| q.surface).sum();
    let fitted_count = needed.iter().filter(|&&n| n).count();
    let quadrics: usize = fitted.patches.iter().map(|q| q.quadrics.len()).sum();
    let bytes: usize = fitted.patches.iter().map(|q| q.storage_bytes()).sum();
    let rate = |q: &Patch| q.mismatched as f64 / q.voxels.max(1) as f64;
    let worst = fitted.patches.iter().map(rate).fold(0.0, f64::max);
    let mut hist = [0usize; 33];
    for (q, _) in fitted.patches.iter().zip(&needed).filter(|(_, n)| **n) {
        hist[q.quadrics.len().min(32)] += 1;
    }
    let used: Vec<String> = hist
        .iter()
        .enumerate()
        .filter(|(_, c)| **c > 0)
        .map(|(n, c)| format!("{n}:{c}"))
        .collect();
    println!(
        "fit: {} patches of {} m ({}x{} columns), n <= {} quadrics, {:.2} avg used [{}], {:.1} KiB as f32, {:.0} ms",
        fitted_count,
        fp.patch_size(),
        fp.cell_columns,
        fp.cell_columns,
        fp.quadratics,
        quadrics as f64 / fitted_count.max(1) as f64,
        used.join(" "),
        bytes as f64 / 1024.0,
        fit_time.as_secs_f64() * 1e3
    );
    println!(
        "fit: {} of {} band voxels disagree with the source ({:.3}%; {:.3} per surface voxel), worst patch {:.2}%",
        wrong,
        voxels,
        100.0 * wrong as f64 / voxels.max(1) as f64,
        wrong as f64 / surface.max(1) as f64,
        100.0 * worst
    );

    // Error heatmap over the view's columns (north up): each pixel shows the
    // patch under that column. Gray = exact, red = errors (full at >= 5%).
    let scale = (1024 / view.n).max(1);
    let w = view.n * scale;
    let mut rgb = vec![0u8; w * w * 3];
    for y in 0..w {
        for x in 0..w {
            let (i, j) = (x / scale, view.n - 1 - y / scale);
            let r = rate(&fitted.patches[touched[j * view.n + i]]);
            let t = (r / 0.05).min(1.0);
            let c = if r == 0.0 {
                [60.0, 60.0, 60.0]
            } else {
                [90.0 + 165.0 * t, 70.0 * (1.0 - t), 40.0 * (1.0 - t)]
            };
            let o = (y * w + x) * 3;
            rgb[o..o + 3].copy_from_slice(&c.map(|v: f64| v as u8));
        }
    }
    let (w, h) = (w, w);
    write_png(
        &p.out.join("fit_error.png"),
        w as u32,
        h as u32,
        &rgb,
        png::ColorType::Rgb,
    )?;

    let iso_start = Instant::now();
    let img = iso::render(&fitted, settings)?;
    let path = p.out.join("iso_fit.png");
    write_png(&path, img.width, img.height, &img.rgb, png::ColorType::Rgb)?;
    println!(
        "iso fit: {} faces, {} -> {} and {} in {:.0} ms",
        img.faces,
        match img.floating {
            Some(f) => format!("{f} floating voxels"),
            None => "floaters not counted".into(),
        },
        path.display(),
        p.out.join("fit_error.png").display(),
        iso_start.elapsed().as_secs_f64() * 1e3
    );
    Ok(())
}
