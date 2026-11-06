//! End-to-end numerical reproducibility across acceleration and worker counts.
use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
#[test]
fn deterministic_buffers_and_complete_center_provenance() {
    let root = std::env::temp_dir().join(format!(
        "frontier-reference-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for (name, threads, plain) in [("fast", "2", false), ("plain", "1", true)] {
        let out = root.join(name);
        let mut command = Command::new(env!("CARGO_BIN_EXE_frontier-reference"));
        command.args([
            "--output",
            out.to_str().unwrap(),
            "--width",
            "8",
            "--height",
            "4",
            "--spp",
            "4",
            "--sample-seed",
            "19",
            "--seed",
            "7",
            "--threads",
            threads,
        ]);
        if plain {
            command.arg("--plain");
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    for name in [
        "linear_rgb",
        "coverage",
        "depth",
        "normals",
        "materials",
        "center_coverage",
        "center_depth",
    ] {
        let file = format!("{name}.f32");
        assert_eq!(
            fs::read(root.join("fast").join(&file)).unwrap(),
            fs::read(root.join("plain").join(file)).unwrap(),
            "{name}"
        );
    }
    let metadata = fs::read_to_string(root.join("fast/center_manifest.json")).unwrap();
    for key in [
        "source_version",
        "seed",
        "edit_revision",
        "voxel_size_m",
        "native_min_cell",
        "native_size_cells",
        "source_fnv1a64",
        "canonical_source_fnv1a64",
        "git_dirty",
    ] {
        assert!(metadata.contains(&format!("\"{key}\"")), "missing {key}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn landscape_pixel_crop_matches_full_render_bytes() {
    let root = std::env::temp_dir().join(format!(
        "frontier-landscape-crop-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for (name, crop) in [("full", None), ("crop", Some("3,2,6,4"))] {
        let out = root.join(name);
        let mut command = Command::new(env!("CARGO_BIN_EXE_landscape"));
        command.args([
            "--output",
            out.to_str().unwrap(),
            "--width",
            "12",
            "--height",
            "8",
            "--extent",
            "8",
            "--center",
            "0,0",
            "--eye",
            "0,94.8875,0",
            "--target",
            "0,92,-1",
            "--far",
            "32",
            "--spp",
            "4",
            "--terrain-recipe",
            "legacy",
            "--threads",
            "2",
        ]);
        if let Some(crop) = crop {
            command.args(["--crop", crop]);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    for (name, channels) in [("linear_rgb", 3), ("coverage", 1), ("depth", 1)] {
        let full = fs::read(root.join(format!("full/{name}.f32"))).unwrap();
        let actual = fs::read(root.join(format!("crop/{name}.f32"))).unwrap();
        let expected: Vec<u8> = (2..6)
            .flat_map(|y| {
                full[(y * 12 + 3) * channels * 4..(y * 12 + 9) * channels * 4]
                    .iter()
                    .copied()
            })
            .collect();
        assert_eq!(actual, expected, "{name}");
    }
    let coverage = fs::read(root.join("crop/coverage.f32")).unwrap();
    assert!(coverage
        .chunks_exact(4)
        .any(|b| f32::from_le_bytes(b.try_into().unwrap()) > 0.));
    let manifest = fs::read_to_string(root.join("crop/manifest.json")).unwrap();
    for field in [
        "\"width\":6",
        "\"height\":4",
        "\"viewport_width\":12",
        "\"viewport_height\":8",
        "\"pixel_crop\":[3, 2, 6, 4]",
        "finite_domain_miss_samples",
        "incomplete oracle coverage",
    ] {
        assert!(manifest.contains(field), "missing {field}");
    }
    let rejected = Command::new(env!("CARGO_BIN_EXE_landscape"))
        .args(["--width", "12", "--height", "8", "--crop", "11,0,2,1"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("pixel crop"));
    fs::remove_dir_all(root).unwrap();
}
