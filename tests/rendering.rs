use microvoxel::{
    generation::FlatTerrain,
    grid::CellPos,
    materials::{Appearance, MaterialId, Voxel},
    painting,
    render::{
        lod::{self, Layout, MeshKey, TileKey},
        surface::{FaceCollector, AXES, NORMALS},
        terrain::{Sampler, TerrainMesh},
    },
    Edit, World,
};
use std::sync::Arc;
fn key(level: u8, x: u32, z: u32, neighbors: [u8; 4], revision: u64) -> MeshKey {
    MeshKey {
        tile: TileKey { level, x, z },
        neighbors,
        revision,
    }
}
fn mesh(world: Arc<World<FlatTerrain>>, key: MeshKey) -> TerrainMesh {
    Sampler::new(world).mesh(key).unwrap()
}

#[test]
fn differently_colored_faces_merge_without_losing_any_color() {
    let mut faces = FaceCollector::default();
    for z in 0..32 {
        for x in 0..32 {
            faces.add(0, [x, 5, z], [x as u8, z as u8, 130]).unwrap();
        }
    }
    let surface = faces.finish([0, 0, 0], 1);
    assert_eq!(surface.unit_faces, 1024);
    assert_eq!(surface.quads.len(), 1);
    assert_eq!(surface.bytes(), 32 + 4096);
    let q = &surface.quads[0];
    assert_eq!(q.size, [32, 32]);
    // +Y's U axis is Z and V is X. Every cell survives in the atlas.
    for x in 0..32 {
        for z in 0..32 {
            assert_eq!(
                surface.colors[x * 32 + z].to_le_bytes(),
                [x as u8, z as u8, 130, 255]
            );
        }
    }
}
#[test]
fn native_surface_is_exact_including_cavities_and_edited_appearance() {
    let mut world = World::new(FlatTerrain {
        height: 32,
        water: 32,
        seed: 42,
    });
    let hole = CellPos::new(325, 10, 326);
    let tinted = CellPos::new(325, 31, 326);
    world
        .apply(&[
            Edit::set(hole, Voxel::AIR),
            Edit::set(tinted, Voxel::new(MaterialId::GRASS, Appearance(12345))),
        ])
        .unwrap();
    let world = Arc::new(world);
    let m = mesh(
        Arc::clone(&world),
        key(13, 10, 10, [13; 4], world.revision()),
    );
    assert_eq!(m.faces(), 1030);
    assert_eq!(m.quads(), 7);
    for part in &m.parts {
        assert_eq!(part.step, 1);
        for q in &part.surface.quads {
            let [axis, ua, va] = AXES[q.face as usize];
            let n = NORMALS[q.face as usize];
            for v in 0..q.size[1] {
                for u in 0..q.size[0] {
                    let mut p =
                        std::array::from_fn::<_, 3, _>(|i| m.origin[i] + q.origin[i] as i64);
                    p[ua] += u as i64;
                    p[va] += v as i64;
                    if n[axis] > 0 {
                        p[axis] -= 1;
                    }
                    let voxel = world
                        .voxel_at(CellPos::new(p[0], p[1], p[2]))
                        .known()
                        .unwrap();
                    assert!(!voxel.is_air());
                    let expected = painting::base_color(voxel, world.materials());
                    assert_eq!(
                        &part.surface.colors[(q.color_offset + v * q.size[0] + u) as usize]
                            .to_le_bytes()[..3],
                        &expected
                    );
                }
            }
        }
    }
}
#[test]
fn flat_terrain_is_one_rectangle_even_with_individual_voxel_colors() {
    let world = Arc::new(World::new(FlatTerrain {
        height: 32,
        water: 32,
        seed: 42,
    }));
    let m = mesh(world, key(13, 10, 10, [13; 4], 0));
    assert_eq!(m.quads(), 1);
    assert_eq!(m.faces(), 1024);
    assert!(m.parts[0].surface.colors.windows(2).any(|p| p[0] != p[1]));
}
#[test]
fn coarse_fine_boundary_is_closed_at_fine_heights_in_both_directions() {
    for height in [31, 33] {
        let mut world = World::new(FlatTerrain {
            height: 31,
            water: 31,
            seed: 42,
        });
        if height == 33 {
            let mut edits = Vec::new();
            for z in 640..704 {
                for x in 672..704 {
                    for y in 31..33 {
                        edits.push(Edit::set(
                            CellPos::new(x, y, z),
                            Voxel::new(MaterialId::GRASS, Appearance(1)),
                        ));
                    }
                }
            }
            world.apply(&edits).unwrap();
        }
        let revision = world.revision();
        let world = Arc::new(world);
        let coarse = mesh(
            Arc::clone(&world),
            key(12, 11, 10, [13, 12, 12, 12], revision),
        );
        let fine = mesh(world, key(13, 21, 20, [13, 12, 13, 13], revision));
        let boundary_faces = |m: &TerrainMesh, face: u32| -> Vec<[i64; 4]> {
            let mut result = Vec::new();
            for part in &m.parts {
                for q in &part.surface.quads {
                    if q.face != face {
                        continue;
                    }
                    let p = std::array::from_fn::<_, 3, _>(|i| m.origin[i] + q.origin[i] as i64);
                    if p[0] != 704 {
                        continue;
                    }
                    let [_, u, v] = AXES[face as usize];
                    let mut end = p;
                    end[u] += q.size[0] as i64 * part.step;
                    end[v] += q.size[1] as i64 * part.step;
                    result.push([p[1], end[1], p[2], end[2]]);
                }
            }
            result
        };
        let coarse_faces = boundary_faces(&coarse, 2);
        let fine_faces = boundary_faces(&fine, 3);
        if height == 31 {
            assert_eq!(coarse_faces, vec![[31, 32, 640, 704]]);
            assert!(fine_faces.is_empty());
        } else {
            assert!(coarse_faces.is_empty());
            assert_eq!(fine_faces, vec![[32, 33, 640, 672]]);
        }
        for part in coarse.parts.iter().chain(&fine.parts) {
            for q in &part.surface.quads {
                for p in q.origin {
                    assert_eq!(p as i64 % part.step, 0);
                }
            }
        }
    }
}
#[test]
fn all_lod_geometry_stays_axis_aligned_and_coarse_grid_aligned() {
    let world = Arc::new(World::new(FlatTerrain {
        height: 73,
        water: 81,
        seed: 42,
    }));
    for level in 9..=13 {
        let m = mesh(Arc::clone(&world), key(level, 10, 10, [level; 4], 0));
        for part in &m.parts {
            for q in &part.surface.quads {
                assert_eq!(
                    NORMALS[q.face as usize].iter().filter(|&&v| v != 0).count(),
                    1
                );
                assert!(q.size.iter().all(|&v| v > 0));
                for coord in q.origin {
                    assert_eq!(coord as i64 % part.step, 0);
                }
            }
        }
    }
}
#[test]
fn empty_baselines_stay_empty_at_every_lod() {
    let world = Arc::new(World::new(FlatTerrain {
        height: 0,
        water: 0,
        seed: 1,
    }));
    for level in 10..=13 {
        assert_eq!(
            mesh(Arc::clone(&world), key(level, 10, 10, [level; 4], 0)).faces(),
            0
        );
    }
}
#[test]
fn lod_cover_is_complete_disjoint_balanced_and_native_around_the_camera() {
    let eye = [9316., 359., 5037.];
    let layout = lod::select(eye, 720, 75f64.to_radians(), &Layout::root(), 384);
    assert!(layout.tiles.len() <= 384, "{} tiles", layout.tiles.len());
    for key in &layout.tiles {
        let mut ancestor = key.parent();
        while let Some(p) = ancestor {
            assert!(!layout.tiles.contains(&p));
            ancestor = p.parent();
        }
        for level in layout.neighbors(*key) {
            assert!(level.abs_diff(key.level) <= 1);
        }
    }
    for z in (0..lod::EXTENT).step_by(997) {
        for x in (0..lod::EXTENT).step_by(991) {
            assert!(layout.at(x, z).is_some());
        }
    }
    assert!(layout.at(-1, 0).is_none());
    assert!(layout.at(lod::EXTENT, 0).is_none());
    assert_eq!(
        layout
            .at((eye[0] * 16.) as i64, (eye[2] * 16.) as i64)
            .unwrap()
            .level,
        lod::FINEST
    );
}
#[test]
fn boundary_neighbors_and_revisions_are_part_of_mesh_identity() {
    let world = Arc::new(World::new(FlatTerrain {
        height: 32,
        water: 32,
        seed: 1,
    }));
    let mut sampler = Sampler::new(world);
    assert!(sampler.mesh(key(13, 10, 10, [10; 4], 0)).is_err());
    assert!(sampler.mesh(key(13, 10, 10, [13; 4], 1)).is_err());
    assert_ne!(
        key(12, 10, 10, [12; 4], 0),
        key(12, 10, 10, [13, 12, 12, 12], 0)
    );
}
