use microvoxel::{
    generation::{Generator, GeneratorIdentity},
    materials::{Appearance, MaterialId, Voxel},
    render::brick_trace::{BrickTrace, Trace},
    spec::WorldSpec,
    world::{Edit, EditAction, EditError, Lookup, World},
    Bounds, CellPos, GridScale,
};

struct Finite;
impl Generator for Finite {
    fn identity(&self) -> GeneratorIdentity {
        GeneratorIdentity {
            seed: 91,
            version: 1,
            definition: 0xabc,
        }
    }
    fn voxel(&self, p: CellPos) -> Option<Voxel> {
        if !(-8..8).contains(&p.x) || !(-8..8).contains(&p.y) || !(-8..8).contains(&p.z) {
            return None;
        }
        Some(if p.y <= 0 {
            Voxel::new(MaterialId::ROCK, Appearance(0x1234))
        } else {
            Voxel::AIR
        })
    }
}
fn b(min: [i64; 3], max: [i64; 3]) -> Bounds {
    Bounds {
        min: CellPos::new(min[0], min[1], min[2]),
        max: CellPos::new(max[0], max[1], max[2]),
    }
}

#[test]
fn snapshot_bulk_is_y_fastest_and_matches_material_runs_including_unknown() {
    let world = World::new(Finite);
    let bounds = b([-2, -2, -1], [1, 2, 1]);
    let bulk = world.fill_voxels(bounds).unwrap();
    let mut i = 0;
    for z in bounds.min.z..bounds.max.z {
        for x in bounds.min.x..bounds.max.x {
            let runs = world.read_snapshot().column_runs(x, z, -2..2).unwrap();
            assert!(runs.is_some());
            for y in -2..2 {
                let p = CellPos::new(x, y, z);
                assert_eq!(bulk[i], world.voxel_at(p));
                let run_material = runs
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|r| r.start <= y && y < r.end)
                    .map(|r| r.material);
                assert_eq!(run_material, bulk[i].known().map(|v| v.material));
                i += 1;
            }
        }
    }
    assert_eq!(i, bulk.len());
    assert_eq!(world.read_snapshot().column_runs(8, 0, 0..2).unwrap(), None);
}

#[test]
fn snapshot_isolation_air_reset_and_invalid_batch_are_atomic() {
    let mut world = World::new(Finite);
    let p = CellPos::new(-1, -1, -1);
    let saved = world.read_snapshot();
    world
        .apply_actions(&[EditAction::Set(Edit::set(p, Voxel::AIR))])
        .unwrap();
    assert_eq!(
        saved.voxel_at(p),
        Lookup::Known(Voxel::new(MaterialId::ROCK, Appearance(0x1234)))
    );
    assert_eq!(world.voxel_at(p), Lookup::Known(Voxel::AIR));
    let revision = world.revision();
    let q = CellPos::new(-2, -1, -1);
    let invalid = [
        EditAction::Set(Edit::set(q, Voxel::new(MaterialId::SOIL, Appearance(0)))),
        EditAction::Set(Edit::set(CellPos::new(20, 0, 0), Voxel::AIR)),
    ];
    assert_eq!(
        world.apply_actions(&invalid).unwrap_err(),
        EditError::Unknown(CellPos::new(20, 0, 0))
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(
        world.voxel_at(q),
        Lookup::Known(Voxel::new(MaterialId::ROCK, Appearance(0x1234)))
    );
    world
        .apply_actions(&[EditAction::Reset {
            pos: p,
            expected: Some(Voxel::AIR),
        }])
        .unwrap();
    assert_eq!(world.voxel_at(p), saved.voxel_at(p));
}

#[test]
fn ownership_pages_use_physical_scale_on_all_signed_axes() {
    let spec = WorldSpec {
        grid: GridScale::new(1, 1000).unwrap(),
        ..WorldSpec::default()
    };
    assert_eq!(spec.page3(CellPos::new(-1, -64_000, 64_000)), [-1, -1, 1]);
    assert_eq!(spec.page3(CellPos::new(-64_001, 64_001, -1)), [-2, 1, -1]);
    assert_eq!(
        spec.page3(CellPos::new(i64::MAX, 0, i64::MIN))[0],
        i64::MAX / 64_000
    );
}

#[test]
fn brick_trace_handles_zero_axes_corner_ties_unknown_and_budget() {
    let world = World::new(Finite);
    let bounds = b([-8, -8, -8], [10, 8, 8]);
    let grid = BrickTrace::build(&world.read_snapshot(), bounds).unwrap();
    assert_eq!(
        grid.trace([0.5, 1.5, 0.5], [0., -1., 0.], 4., 20),
        Trace::Hit {
            pos: CellPos::new(0, 0, 0),
            material: MaterialId::ROCK
        }
    );
    assert_eq!(
        grid.trace([0.5, 1.5, 0.5], [0., 0., 0.], 4., 20),
        Trace::Incomplete
    );
    assert_eq!(
        grid.trace([0.5, 1.5, 0.5], [0., -1., 0.], 4., 1),
        Trace::Incomplete
    );
    // Exact corner ties advance both axes; a cell touched only at the corner is skipped.
    assert_eq!(
        grid.trace([-0.5, 1.5, 0.5], [1., -1., 0.], 4., 20),
        Trace::Hit {
            pos: CellPos::new(0, 0, 0),
            material: MaterialId::ROCK
        }
    );
    // The index covers the whole requested box; source unknowns remain visible as unknown.
    assert_eq!(
        grid.trace([7.5, 1.5, 0.5], [1., 0., 0.], 2., 20),
        Trace::Unknown(CellPos::new(8, 1, 0))
    );
}
