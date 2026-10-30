use microvoxel::{
    character::{CharacterRecipe, Clip, Pose, PoseCache},
    materials::{MaterialRegistry, Voxel},
    render::surface::NORMALS,
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};
fn setup() -> (
    CharacterRecipe,
    MaterialRegistry,
    microvoxel::character::CharacterPalette,
) {
    let recipe = CharacterRecipe::default();
    let mut registry = MaterialRegistry::default();
    let palette = recipe.register_palette(&mut registry).unwrap();
    (recipe, registry, palette)
}
#[test]
fn resolution_adds_real_cells_and_features_at_a_stable_physical_size() {
    let (recipe, materials, palette) = setup();
    let mut previous = 0;
    for resolution in [8, 16, 32, 64] {
        let f = recipe
            .generate(resolution, Pose::new(Clip::Idle, 0), &palette)
            .unwrap();
        assert!(f.solid_count() > previous * 3);
        previous = f.solid_count();
        let max_y = f.occupied().map(|(p, _)| p[1] + 1).max().unwrap();
        assert!(
            (f64::from(max_y) / f64::from(resolution) - 1.75).abs() <= 1. / f64::from(resolution)
        );
        assert!(f.memory_bytes() <= 4 * 1024 * 1024);
        let names: BTreeSet<_> = f
            .occupied()
            .map(|(_, v)| materials.get(v.material).unwrap().name.as_str())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("dark")));
        if resolution >= 32 {
            assert!(names.iter().any(|n| n.ends_with("eye_white")));
            assert!(names.iter().any(|n| n.ends_with("cheek")));
        }
        assert!(materials
            .definitions()
            .iter()
            .all(|m| m.properties.is_empty()));
    }
    assert!(recipe
        .generate(0, Pose::new(Clip::Idle, 0), &palette)
        .is_err());
    assert!(recipe
        .generate(256, Pose::new(Clip::Idle, 0), &palette)
        .is_err());
    assert_eq!(std::mem::size_of::<Voxel>(), 4);
}
#[test]
fn every_native_pose_is_connected_and_whole_cells_with_supporting_feet() {
    let (recipe, _, palette) = setup();
    for clip in Clip::ALL {
        for frame in 0..clip.frames() {
            for heading in 0..16 {
                let pose = Pose::new(clip, frame).facing(heading);
                let f = recipe.generate(16, pose, &palette).unwrap();
                let occupied: BTreeSet<_> = f.occupied().map(|(p, _)| p).collect();
                let first = *occupied.first().unwrap();
                let mut visited = BTreeSet::from([first]);
                let mut queue = VecDeque::from([first]);
                while let Some(p) = queue.pop_front() {
                    for n in NORMALS {
                        let next = std::array::from_fn(|a| p[a] + n[a]);
                        if occupied.contains(&next) && visited.insert(next) {
                            queue.push_back(next);
                        }
                    }
                }
                assert_eq!(visited.len(), occupied.len(), "disconnected {pose:?}");
                if clip != Clip::Jump {
                    assert_eq!(occupied.iter().map(|p| p[1]).min(), Some(0));
                }
                assert!(occupied.iter().all(|p| p[0] > f.origin[0]
                    && p[0] < f.origin[0] + f.size[0] as i32 - 1
                    && p[2] > f.origin[2]
                    && p[2] < f.origin[2] + f.size[2] as i32 - 1));
            }
        }
    }
}
#[test]
fn meshing_culls_internal_faces_and_retains_integer_grid_and_cell_colors() {
    let (recipe, materials, palette) = setup();
    let f = recipe
        .generate(16, Pose::new(Clip::Walk, 3), &palette)
        .unwrap();
    let surface = f.surface(&materials, [0; 3]).unwrap();
    let expected: usize = f
        .occupied()
        .map(|(p, _)| {
            NORMALS
                .iter()
                .filter(|n| f.voxel(std::array::from_fn(|a| p[a] + n[a])).is_air())
                .count()
        })
        .sum();
    assert_eq!(surface.unit_faces, expected);
    assert_eq!(surface.colors.len(), expected);
    assert!(surface.quads.len() < expected / 2);
    // FaceCollector preserves one palette value for each unit face regardless
    // of which larger rectangle it belongs to.
    let colors: BTreeSet<_> = f
        .occupied()
        .map(|(_, v)| {
            let c = microvoxel::painting::base_color(v, &materials);
            u32::from_le_bytes([c[0], c[1], c[2], 255])
        })
        .collect();
    assert!(surface.colors.iter().all(|v| colors.contains(v)));
    for q in &surface.quads {
        assert!(q.face < 6 && q.size.iter().all(|n| *n > 0));
    }
}
#[test]
fn cached_poses_are_deterministic_canonicalized_and_bounded() {
    let (recipe, _, palette) = setup();
    let single = recipe
        .generate(16, Pose::new(Clip::Walk, 0), &palette)
        .unwrap()
        .memory_bytes();
    let mut cache = PoseCache::new(recipe, palette, 16, single * 2);
    let a = cache.frame(Pose::new(Clip::Walk, 0)).unwrap();
    let b = cache.frame(Pose::new(Clip::Walk, 12).facing(16)).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    for i in 1..12 {
        cache.frame(Pose::new(Clip::Walk, i)).unwrap();
        assert!(cache.memory_bytes() <= single * 2);
    }
    let regenerated = cache.frame(Pose::new(Clip::Walk, 0)).unwrap();
    assert_eq!(*a, *regenerated);
    assert!(!Arc::ptr_eq(&a, &regenerated));
    assert_eq!(regenerated.voxel([999, 999, 999]), Voxel::AIR);
}

#[test]
fn first_person_arm_is_a_subset_of_the_same_collision_body() {
    let (recipe, materials, palette) = setup();
    for heading in 0..16 {
        let pose = Pose::new(Clip::Walk, 3).facing(heading).holding_arm(true);
        let f = recipe.generate(16, pose, &palette).unwrap();
        let arm = f.right_arm(heading);
        assert!(arm.solid_count() > 20 && arm.solid_count() < f.solid_count() / 3);
        for (p, v) in arm.occupied() {
            assert_eq!(f.voxel(p), v);
            // Arm ownership is anatomical, including raised cells above the shoulder.
        }
        assert!(!arm.surface(&materials, [0; 3]).unwrap().quads.is_empty());
    }
}

#[test]
fn looking_poses_keep_connected_heads_clear_eyes_and_owned_arms() {
    let (recipe, materials, palette) = setup();
    for yaw in [-6, 0, 6] {
        for pitch in [-12, -6, 0, 6, 12] {
            let f = recipe
                .generate(
                    16,
                    Pose::new(Clip::Idle, 0)
                        .holding_arm(true)
                        .looking(yaw, pitch),
                    &palette,
                )
                .unwrap();
            let cells: BTreeSet<_> = f.occupied().map(|(p, _)| p).collect();
            let first = *cells.first().unwrap();
            let mut seen = BTreeSet::from([first]);
            let mut queue = VecDeque::from([first]);
            while let Some(p) = queue.pop_front() {
                for n in NORMALS {
                    let next = std::array::from_fn(|a| p[a] + n[a]);
                    if cells.contains(&next) && seen.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
            assert_eq!(
                seen.len(),
                cells.len(),
                "head/arm disconnected: yaw {yaw}, pitch {pitch}"
            );
            // Fixed eye anchor stays inside head occupancy even at extreme pitch.
            for x in -1..=0 {
                for y in 23..=24 {
                    for z in -1..=0 {
                        assert!(
                            !f.voxel([x, y, z]).is_air(),
                            "eye not protected at {yaw},{pitch}"
                        );
                    }
                }
            }
            let arm = f.right_arm(0);
            assert!(arm.solid_count() > 12);
            for (p, v) in arm.occupied() {
                assert_eq!(f.voxel(p), v);
                assert!(!materials.get(v.material).unwrap().name.ends_with("hair"));
            }
            assert!(cells.iter().all(|p| p[0] > f.origin[0]
                && p[0] < f.origin[0] + f.size[0] as i32 - 1
                && p[2] > f.origin[2]
                && p[2] < f.origin[2] + f.size[2] as i32 - 1));
        }
    }
}

#[test]
fn pose_cache_retains_independent_look_angles() {
    let (recipe, _, palette) = setup();
    let mut cache = PoseCache::new(recipe, palette, 16, 1024 * 1024);
    let plain = cache.frame(Pose::new(Clip::Idle, 0)).unwrap();
    let look = Pose::new(Clip::Idle, 0).looking(3, 2);
    let turned = cache.frame(look).unwrap();
    assert_ne!(*plain, *turned);
    assert!(Arc::ptr_eq(&turned, &cache.frame(look).unwrap()));
}
