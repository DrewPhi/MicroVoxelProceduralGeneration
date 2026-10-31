//! Versioned, immutable compiler artifacts. Incomplete output is never published.
use super::Terrain;
use crate::GridScale;
use std::{fs, path::Path, process::Command, time::SystemTime};

fn validate(path: &Path, seed: u64) -> Result<(), String> {
    let summary = fs::read_to_string(path.join("summary.txt"))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    for (key, expected) in [
        ("seed", seed.to_string()),
        ("side", "1025".into()),
        ("extent_m", "1000000".into()),
        ("steps", "320".into()),
        ("dt_years", "10000".into()),
        ("climate_model", "ocean_transport_v1".into()),
        ("wind_degrees", "25".into()),
        ("latitude_at_z0", "35".into()),
        ("boundary_humidity", "0.65".into()),
        ("model", "coupled_evolution".into()),
        ("geography", "seeded_atlas_v1".into()),
        ("irregular", "true".into()),
    ] {
        let prefix = format!("{key}=");
        let mut values = summary
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix));
        if values.next() != Some(expected.as_str()) || values.next().is_some() {
            return Err(format!(
                "{}: atlas identity mismatch for {key} (expected {expected})",
                path.display()
            ));
        }
    }
    Ok(())
}

pub fn prepare_atlas(seed: u64, grid: GridScale, scale: u32) -> Result<Terrain, String> {
    crate::generation::ProceduralTerrain::validate_world_scale(scale)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = root.join("output/atlas-climate/seed42");
    if seed == 42 && validate(&fixture, seed).is_ok() {
        return Terrain::from_atlas(&fixture, grid, scale);
    }
    let cache_root = root.join("output/atlas-cache");
    let destination = cache_root.join(format!("v1-seed{seed}"));
    if destination.exists() {
        validate(&destination, seed)?;
        return Terrain::from_atlas(&destination, grid, scale);
    }
    let adjacent = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("landscape_lab")));
    let compiler = adjacent
        .filter(|p| p.is_file())
        .unwrap_or_else(|| root.join("successor/target/release/landscape_lab"));
    if !compiler.is_file() {
        return Err(format!("Atlas compiler missing: {}. Build landscape_lab with cargo build --release --manifest-path successor/Cargo.toml -p microvoxel --bin landscape_lab", compiler.display()));
    }
    fs::create_dir_all(&cache_root).map_err(|e| format!("{}: {e}", cache_root.display()))?;
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = cache_root.join(format!(".v1-seed{seed}-{}-{nonce}.tmp", std::process::id()));
    // Never reuse a temporary directory or remove artifacts left by another run.
    fs::create_dir(&temporary).map_err(|e| format!("{}: {e}", temporary.display()))?;
    eprintln!(
        "Compiling geographic atlas seed {seed}; this can take several minutes. Output: {}",
        temporary.display()
    );
    let status = Command::new(&compiler)
        .args([
            "--seed",
            &seed.to_string(),
            "--side",
            "1025",
            "--extent-km",
            "1000",
            "--steps",
            "320",
            "--dt",
            "10000",
            "--climate",
            "--wind-deg",
            "25",
            "--latitude",
            "35",
            "--boundary-humidity",
            "0.65",
            "--output",
        ])
        .arg(&temporary)
        .status()
        .map_err(|e| {
            format!(
                "Could not run {}: {e}; output retained at {}",
                compiler.display(),
                temporary.display()
            )
        })?;
    if !status.success() {
        return Err(format!(
            "Atlas compiler exited with {status}; output retained at {}",
            temporary.display()
        ));
    }
    validate(&temporary, seed)?;
    let terrain = Terrain::from_atlas(&temporary, grid, scale)?;
    match fs::rename(&temporary, &destination) {
        Ok(()) => Ok(terrain),
        Err(error) if destination.exists() => {
            // Another process may have published while this compiler ran.
            validate(&destination, seed)?;
            Terrain::from_atlas(&destination, grid, scale).map_err(|e| {
                format!("Atlas publication failed ({error}); concurrent cache invalid: {e}")
            })
        }
        Err(error) => Err(format!(
            "Could not publish atlas to {}: {error}; output retained at {}",
            destination.display(),
            temporary.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// End-to-end compiler/cache regression; may compile a canonical million-node atlas.
    /// Existing published output is reused and never removed by this test.
    #[test]
    #[ignore = "canonical seed1 compilation can take several minutes"]
    fn canonical_seed1_cache_cold_then_warm_identity() {
        use crate::generation::Generator;
        use std::time::Instant;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cache = root.join("output/atlas-cache/v1-seed1");
        let existed = cache.exists();
        let grid = GridScale::default();
        let start = Instant::now();
        let first = Terrain::prepare_atlas(1, grid, 10).expect("seed1 atlas compilation/load");
        let cold = start.elapsed();
        assert!(first.is_atlas());
        validate(&cache, 1).expect("published canonical cache identity");
        let identity = first.identity();
        let spec = first.spec();
        let positions = [
            [0, 0],
            [17, 43],
            [spec.extent_cells() / 2, spec.extent_cells() / 2],
            [spec.extent_cells() - 1, spec.extent_cells() - 1],
        ];
        let columns = positions.map(|[x, z]| {
            let c = first
                .column_hint(x, z)
                .expect("valid canonical atlas column");
            (c.ground, c.water, c.bedrock, c.surface, c.subsoil, c.rock)
        });
        drop(first);
        let start = Instant::now();
        let second = Terrain::prepare_atlas(1, grid, 10).expect("seed1 warm cached load");
        let warm = start.elapsed();
        assert_eq!(identity, second.identity());
        assert_eq!(spec, second.spec());
        for ([x, z], expected) in positions.into_iter().zip(columns) {
            let c = second.column_hint(x, z).expect("stable warm atlas column");
            assert_eq!(
                (c.ground, c.water, c.bedrock, c.surface, c.subsoil, c.rock),
                expected
            );
        }
        eprintln!("seed1 cache existed={existed}; first_compile_load_seconds={:.3}; warm_reload_seconds={:.3}; identity={identity:?}; columns={columns:?}",cold.as_secs_f64(),warm.as_secs_f64());
    }
    #[test]
    fn cache_metadata_rejects_wrong_seed_or_duplicate_identity() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("atlas-identity-{}-{nonce}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let summary = "seed=42\nside=1025\nextent_m=1000000\nsteps=320\ndt_years=10000\nclimate_model=ocean_transport_v1\nwind_degrees=25\nlatitude_at_z0=35\nboundary_humidity=0.65\nmodel=coupled_evolution\ngeography=seeded_atlas_v1\nirregular=true\n";
        fs::write(dir.join("summary.txt"), summary).unwrap();
        assert!(validate(&dir, 42).is_ok());
        assert!(validate(&dir, 99).is_err());
        fs::write(dir.join("summary.txt"), format!("{summary}side=513\n")).unwrap();
        assert!(validate(&dir, 42).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
