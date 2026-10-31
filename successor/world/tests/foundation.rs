use microvoxel::{
    generation::{FlatTerrain, Generator, GeneratorIdentity, ProceduralTerrain},
    grid::{Bounds, CellPos, ChunkPos, CHUNK_VOLUME},
    materials::{Appearance, MaterialId, MaterialKind, MaterialRegistry, Voxel},
    meshing::mesh_bounds,
    painting,
    queries::{raycast, sweep_point, Trace},
    render::{
        lod::{MeshKey, TileKey, FINEST, SIDE},
        terrain::Sampler,
    },
    simulation::{ActiveChunks, Clock, IdAllocator},
    storage::Brick,
    world::{Edit, EditError, Lookup, World},
};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
fn flat(height: i64, water: i64) -> World<FlatTerrain> {
    World::new(FlatTerrain {
        height,
        water,
        seed: 42,
    })
}
fn block(material: MaterialId) -> Voxel {
    Voxel::new(material, Appearance(0x8040))
}
fn bounds(min: [i64; 3], max: [i64; 3]) -> Bounds {
    Bounds {
        min: CellPos::new(min[0], min[1], min[2]),
        max: CellPos::new(max[0], max[1], max[2]),
    }
}
static PATH_ID: AtomicU64 = AtomicU64::new(0);
fn save_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "microvoxel-{}-{}.mvw",
        std::process::id(),
        PATH_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn compact_values_have_no_per_cell_property_allocation() {
    assert_eq!(std::mem::size_of::<Voxel>(), 4);
    let materials = MaterialRegistry::default();
    assert!(materials
        .definitions()
        .iter()
        .all(|d| d.properties.is_empty()));
    let mut registry = materials;
    let id = registry
        .register("future material".into(), MaterialKind::Solid)
        .unwrap();
    assert!(registry.get(id).unwrap().properties.is_empty());
}
#[test]
fn negative_coordinates_roundtrip_through_chunk_indexing() {
    for x in [-65, -33, -32, -31, -1, 0, 1, 31, 32, 33, 65] {
        for y in [-33, -1, 0, 32] {
            for z in [-33, -1, 0, 33] {
                let p = CellPos::new(x, y, z);
                assert_eq!(p.chunk().cell(p.local_index()), Some(p));
            }
        }
    }
    assert_eq!(CellPos::new(-1, 0, 0).chunk().x, -1);
    assert!(CellPos::new(i64::MAX, 0, 0).offset([1, 0, 0]).is_none());
}
struct Bounded;
impl Generator for Bounded {
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: 0,
            version: 1,
            definition: 123,
        }
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        (p.x >= 0).then_some(Voxel::AIR)
    }
}
#[test]
fn unknown_is_not_air_and_transactions_are_atomic() {
    let mut world = World::new(Bounded);
    let valid = CellPos::new(0, 0, 0);
    let unknown = CellPos::new(-1, 0, 0);
    assert_eq!(world.voxel_at(unknown), Lookup::Unknown);
    let edits = [
        Edit::set(valid, block(MaterialId::ROCK)),
        Edit::set(unknown, block(MaterialId::ROCK)),
    ];
    assert_eq!(
        world.apply(&edits).unwrap_err(),
        EditError::Unknown(unknown)
    );
    assert_eq!(world.voxel_at(valid), Lookup::Known(Voxel::AIR));
    assert_eq!(world.revision(), 0);
    assert_eq!(world.storage().override_count(), 0);
}
#[test]
fn conflicts_and_duplicate_edits_never_partially_commit() {
    let mut world = flat(8, 8);
    let a = CellPos::new(0, 1, 0);
    let b = CellPos::new(1, 1, 0);
    let before = world.voxel_at(a);
    let bad = Edit {
        pos: b,
        value: Voxel::AIR,
        expected: Some(Voxel::AIR),
    };
    assert_eq!(
        world.apply(&[Edit::set(a, Voxel::AIR), bad]).unwrap_err(),
        EditError::Conflict(b)
    );
    assert_eq!(world.voxel_at(a), before);
    assert_eq!(
        world
            .apply(&[
                Edit::set(a, Voxel::AIR),
                Edit::set(a, block(MaterialId::WATER))
            ])
            .unwrap_err(),
        EditError::Duplicate(a)
    );
}
#[test]
fn air_tombstones_survive_and_restoring_baseline_releases_storage() {
    let mut world = flat(8, 8);
    let p = CellPos::new(2, 3, 4);
    let baseline = world.voxel_at(p).known().unwrap();
    world.apply(&[Edit::set(p, Voxel::AIR)]).unwrap();
    assert!(world.voxel_at(p).known().unwrap().is_air());
    assert_eq!(world.storage().override_count(), 1);
    world.apply(&[Edit::set(p, baseline)]).unwrap();
    assert_eq!(world.storage().override_count(), 0);
    assert_eq!(world.storage().chunk_count(), 0);
}
#[test]
fn invalidation_is_local_and_tint_changes_do_not_dirty_geometry() {
    let mut world = flat(8, 8);
    let p = CellPos::new(31, 3, 2);
    let old = world.voxel_at(p).known().unwrap();
    let change = world
        .apply(&[Edit::set(
            p,
            Voxel::new(old.material, Appearance(old.appearance.0 ^ 1)),
        )])
        .unwrap();
    assert!(change.geometry.is_empty());
    assert_eq!(change.appearance.len(), 1);
    let change = world.apply(&[Edit::set(p, Voxel::AIR)]).unwrap();
    assert_eq!(change.geometry.len(), 2);
    assert!(change.geometry.contains(&ChunkPos { x: 0, y: 0, z: 0 }));
    assert!(change.geometry.contains(&ChunkPos { x: 1, y: 0, z: 0 }));
}
#[test]
fn snapshots_are_immutable_and_appearance_moves_with_a_voxel() {
    let mut world = flat(32, 32);
    let p = CellPos::new(3, 4, 5);
    let value = world.voxel_at(p).known().unwrap();
    let snapshot = world.snapshot(p.chunk()).unwrap();
    let target = CellPos::new(50, 40, 50);
    world
        .apply(&[Edit::set(p, Voxel::AIR), Edit::set(target, value)])
        .unwrap();
    assert_eq!(snapshot.at(p), Some(value));
    assert_eq!(world.voxel_at(target), Lookup::Known(value));
    assert_eq!(
        painting::base_color(value, world.materials()),
        painting::base_color(world.voxel_at(target).known().unwrap(), world.materials())
    );
}
#[test]
fn dense_edits_compact_to_uniform_and_restore_to_implicit() {
    let mut world = flat(64, 64);
    let key = ChunkPos::default();
    let baseline: Vec<_> = (0..CHUNK_VOLUME)
        .map(|i| {
            let p = key.cell(i as u16).unwrap();
            (p, world.voxel_at(p).known().unwrap())
        })
        .collect();
    let edits: Vec<_> = baseline
        .iter()
        .map(|(p, _)| Edit::set(*p, Voxel::AIR))
        .collect();
    world.apply(&edits).unwrap();
    assert_eq!(world.storage().override_count(), CHUNK_VOLUME);
    assert_eq!(world.storage().memory_bytes(), 4);
    world
        .apply(&[Edit::set(baseline[0].0, baseline[0].1)])
        .unwrap();
    assert_eq!(world.storage().override_count(), CHUNK_VOLUME - 1);
    let restore: Vec<_> = baseline.iter().map(|(p, v)| Edit::set(*p, *v)).collect();
    world.apply(&restore).unwrap();
    assert_eq!(world.storage().chunk_count(), 0);
}
#[test]
fn storage_transitions_preserve_values() {
    let mut brick = Brick::default();
    for i in 0..3000u16 {
        brick.set(i, Some(block(MaterialId::SAND)));
    }
    brick.compact();
    assert!(matches!(brick, Brick::Dense { .. }));
    for i in 0..2900u16 {
        brick.set(i, None);
    }
    brick.compact();
    assert!(matches!(brick, Brick::Sparse(_)));
    assert_eq!(brick.len(), 100);
    assert_eq!(brick.get(2999), Some(block(MaterialId::SAND)));
    assert_eq!(brick.get(0), None);
}
#[test]
fn saves_roundtrip_edits_and_reject_incompatible_or_corrupt_baselines() {
    let path = save_path();
    let mut world = flat(8, 12);
    let edits = [
        Edit::set(CellPos::new(-1, 3, 0), Voxel::AIR),
        Edit::set(CellPos::new(32, 15, 0), block(MaterialId::GRAVEL)),
    ];
    world.apply(&edits).unwrap();
    world.save_edits(&path).unwrap();
    let mut loaded = flat(8, 12);
    loaded.load_edits(&path).unwrap();
    for edit in edits {
        assert_eq!(loaded.voxel_at(edit.pos), Lookup::Known(edit.value));
    }
    assert_eq!(loaded.revision(), world.revision());
    assert!(flat(9, 12).load_edits(&path).is_err());
    let mut wrong_seed = World::new(FlatTerrain {
        height: 8,
        water: 12,
        seed: 43,
    });
    assert!(wrong_seed.load_edits(&path).is_err());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.pop();
    std::fs::write(&path, &bytes).unwrap();
    let count = loaded.storage().override_count();
    assert!(loaded.load_edits(&path).is_err());
    assert_eq!(loaded.storage().override_count(), count);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn color_is_equal_on_all_faces_of_solid_and_water_cells() {
    for material in [MaterialId::GRASS, MaterialId::WATER, MaterialId::ROCK] {
        let mut world = flat(0, 0);
        let p = CellPos::new(0, 0, 0);
        let value = block(material);
        world.apply(&[Edit::set(p, value)]).unwrap();
        let mesh = mesh_bounds(&world, bounds([-1, -1, -1], [2, 2, 2])).unwrap();
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        let color = painting::base_color(value, world.materials());
        assert!(mesh.vertices.iter().all(|v| v.color[..3] == color));
        let normals: BTreeSet<_> = mesh.vertices.iter().map(|v| v.normal).collect();
        assert_eq!(normals.len(), 6);
    }
}
#[test]
fn adjacent_materials_hide_internal_faces_and_cavities_get_surfaces() {
    let mut world = flat(0, 0);
    world
        .apply(&[
            Edit::set(CellPos::new(0, 0, 0), block(MaterialId::ROCK)),
            Edit::set(CellPos::new(1, 0, 0), block(MaterialId::WATER)),
        ])
        .unwrap();
    assert_eq!(
        mesh_bounds(&world, bounds([-1, -1, -1], [3, 2, 2]))
            .unwrap()
            .indices
            .len()
            / 6,
        10
    );
    let mut cube = flat(0, 0);
    let mut edits = Vec::new();
    for x in 0..3 {
        for y in 0..3 {
            for z in 0..3 {
                edits.push(Edit::set(CellPos::new(x, y, z), block(MaterialId::ROCK)));
            }
        }
    }
    cube.apply(&edits).unwrap();
    let before = mesh_bounds(&cube, bounds([-1, -1, -1], [4, 4, 4]))
        .unwrap()
        .indices
        .len()
        / 6;
    cube.apply(&[Edit::set(CellPos::new(1, 1, 1), Voxel::AIR)])
        .unwrap();
    let after = mesh_bounds(&cube, bounds([-1, -1, -1], [4, 4, 4]))
        .unwrap()
        .indices
        .len()
        / 6;
    assert_eq!(after, before + 6);
}
#[test]
fn column_profiles_and_native_tiles_preserve_internal_holes() {
    let mut world = flat(16, 20);
    let p = CellPos::new(3, 5, 3);
    world.apply(&[Edit::set(p, Voxel::AIR)]).unwrap();
    let runs = world.column_runs(3, 3, 0..24).unwrap();
    assert_eq!(
        runs.iter()
            .find(|r| r.start <= 5 && 5 < r.end)
            .unwrap()
            .material,
        MaterialId::AIR
    );
    assert_eq!(
        world
            .voxel_at(CellPos::new(3, 18, 3))
            .known()
            .unwrap()
            .material,
        MaterialId::WATER
    );
    let revision = world.revision();
    let mut sampler = Sampler::new(Arc::new(world));
    let mesh = sampler
        .mesh(MeshKey {
            corner_levels: None,
            tile: TileKey {
                level: FINEST,
                x: 0,
                z: 0,
            },
            neighbors: [FINEST; 4],
            revision,
        })
        .unwrap();
    // A complete one-cell cavity contributes six real exposed faces.
    assert_eq!(mesh.faces(), 32 * 32 + 6);
    assert!(mesh
        .parts
        .iter()
        .flat_map(|part| &part.surface.quads)
        .any(|q| q.face == 0 && q.origin == [3, 5, 3] && q.size == [1, 1]));
}
#[test]
fn water_is_full_cells_and_rays_and_sweeps_agree() {
    let world = flat(4, 8);
    let hit = raycast(&world, [0.03125, 0.75, 0.03125], [0.0, -1.0, 0.0], 2.0);
    let Trace::Hit(hit) = hit else {
        panic!("missing water hit")
    };
    assert_eq!(hit.cell, CellPos::new(0, 7, 0));
    assert_eq!(hit.voxel.material, MaterialId::WATER);
    assert_eq!(hit.distance, 0.25);
    assert_eq!(
        sweep_point(&world, CellPos::new(0, 8, 0), [0, -1, 0]).position,
        CellPos::new(0, 8, 0)
    );
}
#[test]
fn fast_grid_motion_cannot_tunnel_or_cut_solid_corners() {
    let mut world = flat(0, 0);
    world
        .apply(&[Edit::set(CellPos::new(3, 5, 0), block(MaterialId::ROCK))])
        .unwrap();
    let sweep = sweep_point(&world, CellPos::new(0, 5, 0), [10, 0, 0]);
    assert!(sweep.blocked);
    assert_eq!(sweep.position, CellPos::new(2, 5, 0));
    world
        .apply(&[Edit::set(CellPos::new(1, 1, 0), block(MaterialId::ROCK))])
        .unwrap();
    let sweep = sweep_point(&world, CellPos::new(0, 1, 0), [1, 0, 1]);
    assert!(sweep.blocked);
    assert_eq!(sweep.position, CellPos::new(0, 1, 0));
    assert!(matches!(
        raycast(&World::new(Bounded), [-0.1, 1.0, 0.0], [1.0, 0.0, 0.0], 1.0),
        Trace::Unknown(_)
    ));
}
#[test]
fn registries_can_extend_appearance_without_filling_physical_properties() {
    let mut registry = MaterialRegistry::default();
    let id = registry
        .register("custom".into(), MaterialKind::Solid)
        .unwrap();
    registry.get_mut(id).unwrap().palette = [[200, 40, 30], [200, 40, 30]];
    let mut world = World::with_materials(
        FlatTerrain {
            height: 0,
            water: 0,
            seed: 0,
        },
        registry,
    );
    let v = Voxel::new(id, Appearance(0x8080));
    world.apply(&[Edit::set(CellPos::default(), v)]).unwrap();
    let rgb = painting::base_color(v, world.materials());
    assert!(rgb[0] > 190 && rgb[1] < 50);
    assert!(world.materials().get(id).unwrap().properties.is_empty());
}
#[test]
fn scheduling_is_deterministic_bounded_and_camera_independent() {
    let mut active = ActiveChunks::default();
    for i in (0..5).rev() {
        active.wake(ChunkPos { x: i, y: 0, z: 0 });
        active.wake(ChunkPos { x: i, y: 0, z: 0 });
    }
    assert_eq!(active.len(), 5);
    assert_eq!(
        active.take(2).iter().map(|p| p.x).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(active.len(), 3);
    let mut clock = Clock::default();
    assert_eq!(clock.advance().unwrap().0, 1);
    let mut ids = IdAllocator::default();
    assert_ne!(ids.allocate(), ids.allocate());
}
#[test]
fn native_world_values_do_not_depend_on_lod_or_generation_order() {
    let world = Arc::new(World::new(ProceduralTerrain::new(42)));
    let points = [
        CellPos::new(149057, 5721, 80587),
        CellPos::new(200, 600, 200),
        CellPos::new(160000, 0, 0),
    ];
    let before: Vec<_> = points.iter().map(|p| world.voxel_at(*p)).collect();
    let mut sampler = Sampler::new(Arc::clone(&world));
    let native = sampler
        .mesh(MeshKey {
            corner_levels: None,
            tile: TileKey {
                level: FINEST,
                x: 149057 / SIDE as u32,
                z: 80587 / SIDE as u32,
            },
            neighbors: [FINEST; 4],
            revision: world.revision(),
        })
        .unwrap();
    let proxy = sampler
        .mesh(MeshKey {
            corner_levels: None,
            tile: TileKey {
                level: 3,
                x: 4,
                z: 2,
            },
            neighbors: [3; 4],
            revision: world.revision(),
        })
        .unwrap();
    assert_eq!(native.parts[0].step, 1);
    assert!(proxy
        .parts
        .iter()
        .flat_map(|p| &p.surface.quads)
        .any(|q| q.constant_color && q.size.iter().any(|v| *v > 32)));
    for (i, p) in points.iter().enumerate().rev() {
        assert_eq!(world.voxel_at(*p), before[i]);
    }
    assert_eq!(before[2], Lookup::Unknown);
}
