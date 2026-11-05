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
