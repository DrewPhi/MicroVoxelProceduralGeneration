//! Integration tests have a separate process so they can select a custom grid.
use frontier_world::{configure_voxel_grid, raycast, Cell, FbmParams, Material, VoxelGrid, World};

#[test]
fn decimal_grid_is_shared_by_terrain_edits_and_ray_queries() {
    let grid = VoxelGrid::new(0.03).unwrap();
    configure_voxel_grid(grid).unwrap();
    assert!(configure_voxel_grid(VoxelGrid::DEFAULT).is_err());
    let mut world = World::fbm(7, FbmParams::default()).unwrap();
    let column = world.column(-57, 29);
    assert_eq!(
        column.height_m(),
        column.height_cells() as f64 * grid.size_m()
    );

    let edited = Cell {
        x: -57,
        y: column.height_cells() + 5,
        z: 29,
    };
    world.set(edited, Material::Wood);
    assert_eq!(Cell::from_meters(edited.center_m()), edited);
    assert_eq!(world.material(edited), Material::Wood);

    let face_y = (edited.y + 1) as f64 * grid.size_m();
    let [x, _, z] = edited.center_m();
    let hit = raycast(&world, [x, face_y + 0.5, z], [0., -1., 0.], 0.6).unwrap();
    assert_eq!(hit.cell, edited);
    assert!((hit.distance - 0.5).abs() < 1e-12);
    let face_hit = raycast(&world, [x, face_y, z], [0., -1., 0.], 0.1).unwrap();
    assert_eq!(face_hit.cell, edited);
    assert_eq!(face_hit.distance, 0.);
}
