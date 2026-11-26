use super::*;

fn ground(x: i64, z: i64) -> PlantRecord {
    PlantRecord {
        species: Species::Grass,
        variant: u8::MAX,
        seed: 7,
        root: Cell { x, y: 0, z },
        rotation: 0,
    }
}
fn override_cell(world: &mut World, c: Cell) {
    world.set(
        c,
        if world.material(c) == Material::Air {
            Material::Stone
        } else {
            Material::Air
        },
    );
}

#[test]
fn local_edit_id_ignores_distant_edits_revision_and_tracks_negative_tile_edges() {
    let p = ground(-128, -128);
    let local = Cell {
        x: -1,
        y: 700,
        z: -1,
    };
    let outside = Cell {
        x: 0,
        y: 700,
        z: -1,
    };
    let distant = Cell {
        x: 10000,
        y: 700,
        z: 10000,
    };
    assert_eq!(plant_edit_id(&p, &[outside, distant], 99), 0);
    let first = plant_edit_id(&p, &[local], 1);
    assert_ne!(first, 0);
    assert_eq!(first, plant_edit_id(&p, &[local, outside, distant], 999));
    assert_ne!(
        first,
        plant_edit_id(&p, &[Cell { x: -2, ..local }, local], 1)
    );
    let tree = PlantRecord {
        species: Species::Rowan,
        variant: 0,
        root: Cell {
            x: -20,
            y: 500,
            z: -20,
        },
        ..p
    };
    for variant in [
        0,
        vegetation::living_flora::CONIFER,
        vegetation::living_flora::SHRUB,
    ] {
        let tree = PlantRecord { variant, ..tree };
        let (radius, height) = tree.extent_cells();
        let edge = Cell {
            x: tree.root.x + radius,
            y: tree.root.y + height,
            z: tree.root.z,
        };
        assert_ne!(plant_edit_id(&tree, &[edge], 1), 0);
        assert_eq!(
            plant_edit_id(
                &tree,
                &[Cell {
                    x: edge.x + 1,
                    ..edge
                }],
                1
            ),
            0
        );
        assert_eq!(
            plant_edit_id(
                &tree,
                &[Cell {
                    y: edge.y + 1,
                    ..edge
                }],
                1
            ),
            0
        );
    }
}

#[test]
fn indexed_edit_ids_match_flat_sets_at_negative_boundaries() {
    let edits: Vec<_> = (0..1000i64)
        .map(|i| Cell {
            x: (i * 37) % 1024 - 512,
            y: i % 300,
            z: (i * 71) % 1024 - 512,
        })
        .collect();
    let index = PlantEdits::new(edits.iter().copied());
    for x in [-513, -128, -1, 0, 127, 128, 511] {
        for z in [-513, -128, -1, 0, 127, 128, 511] {
            let p = ground(x, z);
            assert_eq!(index.id(&p), plant_edit_id(&p, &edits, 99));
            let tree = PlantRecord {
                species: Species::Rowan,
                variant: 0,
                ..p
            };
            assert_eq!(index.id(&tree), plant_edit_id(&tree, &edits, 99));
        }
    }
}

#[test]
fn local_edit_snapshot_retains_placement_templates_jobs_and_hysteresis() {
    let world = World::new(7).with_vegetation();
    let mut stream = VegetationStream::new(world.clone(), VegetationConfig::default());
    stream.background_paused = true;
    let tile = Tile {
        species: 0,
        x: -1,
        z: -1,
    };
    stream
        .tiles
        .insert(tile, PlacedPlants::new(vec![ground(-128, -128)], 12, false));
    stream.pending_tiles.insert(Tile {
        species: 2,
        x: 9,
        z: 9,
    });
    let near = ground(-128, -128);
    let far = ground(256, 256);
    let near_key = ground_key(&near, 8, 0);
    let far_key = ground_key(&far, 8, 0);
    let mesh = Arc::new(VegetationMesh::default());
    stream.meshes.insert(far_key, (mesh.clone(), 12));
    stream.previous_lods.insert(tree_identity(far_key), 4);
    let epoch = stream.epoch.load(Ordering::Relaxed);
    let near_token = Arc::new(AtomicBool::new(false));
    let far_token = Arc::new(AtomicBool::new(false));
    for (key, p, token) in [
        (near_key, near, near_token.clone()),
        (far_key, far, far_token.clone()),
    ] {
        stream.pending_meshes.insert(key);
        stream.mesh_sources.insert(key, p);
        stream
            .mesh_cancellation
            .lock()
            .unwrap()
            .insert((epoch, key), token);
    }
    let mut edited = world.clone();
    override_cell(
        &mut edited,
        Cell {
            x: -1,
            y: 700,
            z: -1,
        },
    );
    stream.set_world(edited);
    assert_eq!(stream.epoch.load(Ordering::Relaxed), epoch);
    assert!(stream.tiles.contains_key(&tile));
    assert_eq!(stream.pending_tiles.len(), 1);
    assert!(Arc::ptr_eq(&stream.meshes[&far_key].0, &mesh));
    assert_eq!(stream.previous_lods[&tree_identity(far_key)], 4);
    assert!(!stream.pending_meshes.contains(&near_key));
    assert!(stream.pending_meshes.contains(&far_key));
    assert!(near_token.load(Ordering::Relaxed));
    assert!(!far_token.load(Ordering::Relaxed));
    // Reverting to an older snapshot also has a local footprint.
    stream.set_world(world);
    assert!(stream.tiles.contains_key(&tile));
    assert_eq!(stream.epoch.load(Ordering::Relaxed), epoch);
}

#[test]
fn queued_local_edit_results_reject_changed_keys_and_reuse_unaffected_content() {
    let world = World::new(7);
    let mut stream = VegetationStream::new(
        world.clone(),
        VegetationConfig {
            grass_radius_m: 0.,
            flower_radius_m: 0.,
            tree_radius_m: 0.,
            max_tiles: 0,
            max_triangles: 0,
            ..Default::default()
        },
    );
    stream.background_paused = true;
    // Establish the camera epoch before installing synthetic queued jobs.
    stream.update([0.; 3], 720, 1., world.revision());
    let generation = stream.epoch.load(Ordering::Relaxed);
    let near = ground(-128, -128);
    let far = ground(256, 256);
    let near_key = ground_key(&near, 8, 0);
    let far_key = ground_key(&far, 8, 0);
    stream.mesh_sources.insert(near_key, near);
    stream.mesh_sources.insert(far_key, far);
    stream.pending_meshes.extend([near_key, far_key]);
    let mut edited = world.clone();
    override_cell(
        &mut edited,
        Cell {
            x: -1,
            y: 700,
            z: -1,
        },
    );
    let revision = edited.revision();
    stream.set_world(edited);
    let (send, rx) = mpsc::sync_channel(2);
    stream.rx = rx;
    let mesh = Arc::new(VegetationMesh {
        woodland_volume: None,
        vertices: vec![],
        indices: vec![0; 3],
    });
    send.send((
        generation,
        Done::Mesh(near_key, world.revision(), mesh.clone()),
    ))
    .unwrap();
    send.send((
        generation,
        Done::Mesh(far_key, world.revision(), mesh.clone()),
    ))
    .unwrap();
    stream.update([0.; 3], 720, 1., revision);
    assert!(!stream.meshes.contains_key(&near_key));
    assert!(Arc::ptr_eq(&stream.meshes[&far_key].0, &mesh));
}

#[test]
fn cancellation_tokens_are_released_on_every_worker_exit() {
    let registry: MeshCancellation = Arc::new(Mutex::new(BTreeMap::new()));
    let key = (8, ground_key(&ground(0, 0), 8, 0));
    let token = Arc::new(AtomicBool::new(false));
    registry.lock().unwrap().insert(key, token.clone());
    let guard = MeshCancellationGuard {
        registry: registry.clone(),
        key: Some(key),
        token: Some(token.clone()),
    };
    token.store(true, Ordering::Relaxed);
    drop(guard);
    assert!(registry.lock().unwrap().is_empty());
}

#[test]
fn aba_edit_undo_redo_reuses_matching_content_and_rejects_obsolete_completion() {
    let base = World::new(7);
    let mut stream = VegetationStream::new(
        base.clone(),
        VegetationConfig {
            grass_radius_m: 0.,
            flower_radius_m: 0.,
            tree_radius_m: 0.,
            max_tiles: 0,
            max_triangles: 0,
            ..Default::default()
        },
    );
    stream.background_paused = true;
    stream.update([0.; 3], 720, 1., base.revision());
    let generation = stream.epoch.load(Ordering::Relaxed);
    let p = ground(-128, -128);
    let cell = Cell {
        x: -1,
        y: 700,
        z: -1,
    };
    let base_key = ground_key(&p, 8, 0);
    let mut first_edit = base.clone();
    override_cell(&mut first_edit, cell);
    let edited_key = ground_key(&p, 8, PlantEdits::new(first_edit.edit_cells()).id(&p));
    assert_ne!(edited_key.edit_id, 0);
    stream.set_world(first_edit.clone());
    stream.pending_meshes.insert(edited_key);
    stream.mesh_sources.insert(edited_key, p);

    // Undo removes the override map, rather than writing another override that
    // happens to contain the base terrain material. Late B must not publish in A.
    stream.set_world(base.clone());
    let (send, rx) = mpsc::sync_channel(3);
    stream.rx = rx;
    let edited_mesh = Arc::new(VegetationMesh {
        woodland_volume: None,
        vertices: vec![],
        indices: vec![0; 3],
    });
    send.send((
        generation,
        Done::Mesh(edited_key, first_edit.revision(), edited_mesh.clone()),
    ))
    .unwrap();
    stream.update([0.; 3], 720, 1., base.revision());
    assert!(!stream.meshes.contains_key(&edited_key));

    // Reapply the same edit from the base snapshot. Revision is new; immutable
    // masked mesh content is identical, so an old B completion is reusable.
    let mut redo = base.clone();
    override_cell(&mut redo, cell);
    assert_ne!(redo.revision(), first_edit.revision());
    assert_eq!(
        PlantEdits::new(redo.edit_cells()).id(&p),
        edited_key.edit_id
    );
    stream.set_world(redo.clone());
    stream.pending_meshes.insert(edited_key);
    stream.mesh_sources.insert(edited_key, p);
    let base_mesh = Arc::new(VegetationMesh {
        woodland_volume: None,
        vertices: vec![],
        indices: vec![0; 6],
    });
    send.send((
        generation,
        Done::Mesh(base_key, base.revision(), base_mesh.clone()),
    ))
    .unwrap();
    send.send((
        generation,
        Done::Mesh(edited_key, first_edit.revision(), edited_mesh.clone()),
    ))
    .unwrap();
    stream.update([0.; 3], 720, 1., redo.revision());
    assert!(!stream.meshes.contains_key(&base_key));
    assert!(Arc::ptr_eq(&stream.meshes[&edited_key].0, &edited_mesh));
    assert!(!stream.pending_meshes.contains(&edited_key));
    assert_eq!(
        stream.content_epoch, 1,
        "local ABA must not masquerade as a source change"
    );

    // A second undo must still reject the newer B result and accept A again.
    stream.set_world(base.clone());
    stream.pending_meshes.insert(base_key);
    stream.mesh_sources.insert(base_key, p);
    send.send((
        generation,
        Done::Mesh(
            edited_key,
            redo.revision(),
            Arc::new(VegetationMesh::default()),
        ),
    ))
    .unwrap();
    send.send((
        generation,
        Done::Mesh(base_key, base.revision(), base_mesh.clone()),
    ))
    .unwrap();
    stream.update([0.; 3], 720, 1., base.revision());
    assert!(Arc::ptr_eq(&stream.meshes[&base_key].0, &base_mesh));
    assert!(
        Arc::ptr_eq(&stream.meshes[&edited_key].0, &edited_mesh),
        "obsolete completion must not overwrite cached B content"
    );
}

#[test]
fn aba_old_worker_guard_cannot_remove_replacement_cancellation_token() {
    let registry: MeshCancellation = Arc::new(Mutex::new(BTreeMap::new()));
    let identity = (8, ground_key(&ground(-128, -128), 8, 17));
    let original = Arc::new(AtomicBool::new(false));
    registry.lock().unwrap().insert(identity, original.clone());
    let old_worker = MeshCancellationGuard {
        registry: registry.clone(),
        key: Some(identity),
        token: Some(original.clone()),
    };
    original.store(true, Ordering::Relaxed);
    // Undo/redo restores the same content key within the same camera epoch.
    let replacement = Arc::new(AtomicBool::new(false));
    registry
        .lock()
        .unwrap()
        .insert(identity, replacement.clone());
    let new_worker = MeshCancellationGuard {
        registry: registry.clone(),
        key: Some(identity),
        token: Some(replacement.clone()),
    };
    drop(old_worker);
    assert!(Arc::ptr_eq(
        &registry.lock().unwrap()[&identity],
        &replacement
    ));
    assert!(!replacement.load(Ordering::Relaxed));
    replacement.store(true, Ordering::Relaxed);
    drop(new_worker);
    assert!(registry.lock().unwrap().is_empty());
}
