//! Independent triangle oracle: renderer geometry versus canonical voxel DDA.
use frontier_renderer::mesh::{extract, Mesh};
use frontier_world::{raycast, Cell, Material, World, VOXEL_SIZE};

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

// Moller-Trumbore triangle intersections; deliberately independent of voxel DDA.
fn mesh_hit(
    mesh: &Mesh,
    origin: [f64; 3],
    direction: [f64; 3],
    max: f64,
) -> Option<(f64, [f32; 3], [f32; 3])> {
    let len = dot(direction, direction).sqrt();
    let d = direction.map(|v| v / len);
    let base = [mesh.origin.x, mesh.origin.y, mesh.origin.z].map(|v| v as f64 * VOXEL_SIZE);
    let o = sub(origin, base);
    let mut closest = None;
    for tri in mesh.indices.chunks_exact(3) {
        let vertices = [tri[0], tri[1], tri[2]].map(|i| &mesh.vertices[i as usize]);
        let a = vertices[0].position.map(f64::from);
        let e1 = sub(vertices[1].position.map(f64::from), a);
        let e2 = sub(vertices[2].position.map(f64::from), a);
        let p = cross(d, e2);
        let determinant = dot(e1, p);
        if determinant.abs() < 1e-12 {
            continue;
        }
        let tvec = sub(o, a);
        let u = dot(tvec, p) / determinant;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(tvec, e1);
        let v = dot(d, q) / determinant;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let distance = dot(e2, q) / determinant;
        if distance < 0.0 || distance > max {
            continue;
        }
        assert!(dot(cross(e1, e2), vertices[0].normal.map(f64::from)) > 0.0);
        if closest.as_ref().is_none_or(|(best, _, _)| distance < *best) {
            closest = Some((distance, vertices[0].normal, vertices[0].color));
        }
    }
    closest
}

fn compare(world: &World, mesh: &Mesh, o: [f64; 3], d: [f64; 3], max: f64) {
    let voxel = raycast(world, o, d, max);
    let triangle = mesh_hit(mesh, o, d, max);
    assert_eq!(
        voxel.is_some(),
        triangle.is_some(),
        "origin={o:?} direction={d:?}, voxel={voxel:?}, triangle={triangle:?}"
    );
    if let (Some(v), Some((distance, normal, color))) = (voxel, triangle) {
        assert!(
            (v.distance - distance).abs() < 1e-9,
            "{v:?} vs triangle distance {distance}"
        );
        assert_eq!(v.normal.map(|n| n as f32), normal);
        assert_eq!(v.material.color(), color);
    }
}

#[test]
fn cave_rim_native_surface_matches_world() {
    let world = World::new(42);
    let mesh = extract(
        |c| world.material(c),
        Cell::from_meters([-5.0, 1.0, -1.0]),
        [64, 80, 32],
    );
    assert!(!mesh.indices.is_empty());
    // Rays start inside the tunnel and hit its roof, floor, and side wall within
    // this crop. Non-axis directions avoid edge-only triangle contacts.
    for i in 0..31 {
        let angle = (i as f64 + 0.173) * std::f64::consts::TAU / 31.0;
        compare(
            &world,
            &mesh,
            [-2.985, 3.019, 0.017],
            [angle.cos(), angle.sin(), 0.013],
            2.0,
        );
    }
}

#[test]
fn bridge_and_edited_hole_match_world() {
    let mut world = World::new(42);
    // Through-hole in the bridge deck, with retained neighboring wood faces.
    for x in 63..66 {
        for y in 64..68 {
            for z in -25..-22 {
                world.set(Cell { x, y, z }, Material::Air);
            }
        }
    }
    let mesh = extract(
        |c| world.material(c),
        Cell::from_meters([2.0, 3.5, -2.0]),
        [80, 16, 16],
    );
    for x in [3.24, 3.87, 4.018, 4.41, 5.23, 6.77] {
        compare(&world, &mesh, [x, 4.73, -1.473], [0.0, -1.0, 0.0], 1.2);
    }
    assert!(raycast(&world, [4.018, 4.73, -1.473], [0.0, -1.0, 0.0], 1.2).is_none());
    for x in [3.24, 4.41, 5.23] {
        compare(&world, &mesh, [x, 3.72, -1.473], [0.0, 1.0, 0.0], 0.8);
    }
}

#[test]
fn edited_native_cube_preserves_local_origin_at_100km() {
    let mut world = World::new(9);
    let base = Cell::from_meters([100_000.0, 100.0, -100_000.0]);
    for x in 0..6 {
        for y in 0..5 {
            for z in 0..4 {
                world.set(
                    Cell {
                        x: base.x + x,
                        y: base.y + y,
                        z: base.z + z,
                    },
                    if x < 3 {
                        Material::Wood
                    } else {
                        Material::Stone
                    },
                );
            }
        }
    }
    // One channel makes both added and removed edit ownership observable.
    for x in 0..6 {
        world.set(
            Cell {
                x: base.x + x,
                y: base.y + 2,
                z: base.z + 1,
            },
            Material::Air,
        );
    }
    let mesh = extract(|c| world.material(c), base, [6, 5, 4]);
    assert_eq!(mesh.origin, base);
    for y in [0.5, 1.5, 2.5, 3.5, 4.5] {
        for z in [0.5, 1.5, 2.5, 3.5] {
            let origin = [
                100_000.0 - 0.5,
                100.0 + y * VOXEL_SIZE,
                -100_000.0 + z * VOXEL_SIZE,
            ];
            compare(&world, &mesh, origin, [1.0, 0.0, 0.0], 1.5);
        }
    }
}
