use microvoxel::{
    generation::{survey::Survey, Generator, Terrain},
    materials::Appearance,
    painting::{base_color, decode_rgb565, encode_rgb565},
    CellPos, Edit, MaterialId, Voxel, World,
};

fn fixture(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "microvoxel-survey-{label}-{}.mvearth",
        std::process::id()
    ));
    let mut bytes = b"MVEARTH1".to_vec();
    for value in [4u32, 4, 1, 4] {
        bytes.extend(value.to_le_bytes());
    }
    for value in [536975f64, 3665052., 1107.] {
        bytes.extend(value.to_le_bytes());
    }
    for value in [1u32, 1] {
        bytes.extend(value.to_le_bytes());
    }
    for i in 0..16 {
        bytes.extend((10f32 + (i / 4) as f32 * 0.25).to_le_bytes());
    }
    for i in 0..16 {
        bytes.extend(
            encode_rgb565(if i % 2 == 0 { [255, 0, 0] } else { [0, 255, 0] })
                .0
                .to_le_bytes(),
        );
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn measured_cells_runs_filtered_color_and_edits_share_the_baseline() {
    let path = fixture("roundtrip");
    let generator = Terrain::from_survey(&path).unwrap();
    assert_eq!(generator.spec().grid.ratio(), (1, 4));
    let survey = generator.survey().unwrap();
    assert_eq!(survey.spawn, [1, 1]);
    assert_eq!(survey.projected_origin, [536975., 3665052.]);
    assert_eq!(survey.elevation_offset, 1107.);
    let range = generator.terrain_summary([0., 0.], 1.).unwrap();
    assert_eq!([range.min_m, range.max_m], [10., 10.75]);
    let mut world = World::new(generator);
    for z in 0..4 {
        for x in 0..4 {
            let c = world.surface_column(x, z).unwrap();
            assert_eq!(c.ground, 40 + z);
            let runs = world.column_runs(x, z, 0..50).unwrap();
            for y in 0..50 {
                let v = world.voxel_at(CellPos::new(x, y, z)).known().unwrap();
                assert_eq!(
                    v.material,
                    runs.iter()
                        .find(|r| r.start <= y && y < r.end)
                        .unwrap()
                        .material
                );
            }
        }
    }
    assert!(world.voxel_at(CellPos::new(4, 0, 0)).known().is_none());
    let coarse = world.display_appearance_at(CellPos::new(0, 39, 0), MaterialId::SURVEY, 2);
    let rgb = decode_rgb565(coarse);
    assert!(rgb[0] > 170 && rgb[1] > 170 && rgb[2] == 0, "{rgb:?}");
    let from = CellPos::new(0, 39, 0);
    let to = CellPos::new(2, 45, 2);
    let voxel = world.voxel_at(from).known().unwrap();
    world
        .apply(&[Edit::set(from, Voxel::AIR), Edit::set(to, voxel)])
        .unwrap();
    assert_eq!(
        base_color(world.voxel_at(to).known().unwrap(), world.materials()),
        [255, 0, 0]
    );
    let save = path.with_extension("mvw");
    world.save_edits(&save).unwrap();
    let mut restored = World::new(Terrain::from_survey(&path).unwrap());
    restored.load_edits(&save).unwrap();
    assert_eq!(restored.voxel_at(to).known(), Some(voxel));
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[56..60].copy_from_slice(&11f32.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();
    assert!(World::new(Terrain::from_survey(&path).unwrap())
        .load_edits(&save)
        .is_err());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(save).unwrap();
}

#[test]
fn malformed_or_missing_survey_samples_are_rejected() {
    let path = fixture("invalid");
    let original = std::fs::read(&path).unwrap();
    for invalid in [0f32, f32::NAN, -9999., 2048.] {
        let mut bytes = original.clone();
        bytes[56..60].copy_from_slice(&invalid.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(Survey::load(&path).is_err());
    }
    std::fs::write(&path, &original[..original.len() - 1]).unwrap();
    assert!(Survey::load(&path).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn photographed_colors_survive_compact_voxel_storage() {
    for value in 0..=u16::MAX {
        assert_eq!(
            encode_rgb565(decode_rgb565(Appearance(value))),
            Appearance(value)
        );
    }
}
