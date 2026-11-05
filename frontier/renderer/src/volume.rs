//! Bounded prototype: exact volume stays resident at every distance. This is
//! deliberately a finite smoke fixture, not scalable distant-volume streaming.
use crate::mesh::{self, Mesh};
use frontier_world::{Cell, World, VOXEL_SIZE};

pub const HALF_CELLS: i64 = 128;
pub fn bounds(world: &World) -> Result<(Cell, [usize; 3]), String> {
    let base = world
        .topology_base_cells()
        .ok_or("volume requires topology source")?;
    let mut lo = base - 80;
    let mut hi = base + 24;
    for z in -HALF_CELLS - 1..=HALF_CELLS {
        for x in -HALF_CELLS - 1..=HALF_CELLS {
            let h = (world.column(x, z).height_m() / VOXEL_SIZE) as i64;
            lo = lo.min(h - 2);
            hi = hi.max(h + 2);
        }
    }
    for c in world.edit_cells().filter(|c| world.volume_owned(*c)) {
        lo = lo.min(c.y - 1);
        hi = hi.max(c.y + 2);
    }
    if hi - lo > 2048 {
        return Err("bounded topology exceeds 2048-cell vertical memory budget".into());
    }
    Ok((
        Cell {
            x: -HALF_CELLS,
            y: lo,
            z: -HALF_CELLS,
        },
        [256, (hi - lo) as usize, 256],
    ))
}
pub fn build(world: &World) -> Result<Mesh, String> {
    let (min, size) = bounds(world)?;
    // Include the neighboring solid cells, not merely a sampling halo: removing
    // an owner-edge cell exposes a face belonging to its unedited outside neighbor.
    // That boundary face is also owned by the volume, while outside tops are clipped.
    let min = Cell {
        x: min.x - 1,
        y: min.y,
        z: min.z - 1,
    };
    let size = [size[0] + 2, size[1], size[2] + 2];
    let columns: Vec<_> = (-130..=129)
        .flat_map(|z| (-130..=129).map(move |x| world.column(x, z)))
        .collect();
    let extracted = mesh::extract(
        |c| columns[(c.z + 130) as usize * 260 + (c.x + 130) as usize].material(c.y),
        min,
        size,
    );
    let mut out = Mesh {
        vertices: vec![],
        indices: vec![],
        origin: min,
        exposed_native_faces: 0,
    };
    let lo = VOXEL_SIZE as f32;
    let hi = 257. * VOXEL_SIZE as f32;
    for face in extracted.vertices.chunks_exact(4) {
        if [0, 2].into_iter().any(|axis| {
            face[0].normal[axis] != 0.
                && (face[0].position[axis] < lo || face[0].position[axis] > hi)
        }) {
            continue;
        }
        let mut face = face.to_vec();
        for v in &mut face {
            for axis in [0, 2] {
                v.position[axis] = v.position[axis].clamp(lo, hi);
            }
        }
        let a = glam::Vec3::from(face[0].position);
        let b = glam::Vec3::from(face[1].position);
        let c = glam::Vec3::from(face[2].position);
        let cross = (b - a).cross(c - a);
        if cross.length_squared() == 0. {
            continue;
        }
        out.exposed_native_faces +=
            (cross.length() / (VOXEL_SIZE * VOXEL_SIZE) as f32).round() as usize;
        let base = out.vertices.len() as u32;
        let indices = if cross.dot(glam::Vec3::from(face[0].normal)) > 0. {
            [0, 1, 2, 0, 2, 3]
        } else {
            [0, 2, 1, 0, 3, 2]
        };
        out.indices.extend(indices.map(|i| base + i));
        out.vertices.extend(face);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{raycast, FbmParams, Material};
    use glam::DVec3;
    fn mesh_hit(mesh: &Mesh, o: DVec3, d: DVec3) -> Option<(f64, [f32; 3])> {
        let offset = DVec3::new(
            mesh.origin.x as f64,
            mesh.origin.y as f64,
            mesh.origin.z as f64,
        ) * VOXEL_SIZE;
        let point =
            |i: u32| DVec3::from(mesh.vertices[i as usize].position.map(f64::from)) + offset;
        let mut best: Option<(f64, [f32; 3])> = None;
        for tri in mesh.indices.chunks_exact(3) {
            let a = point(tri[0]);
            let e1 = point(tri[1]) - a;
            let e2 = point(tri[2]) - a;
            let p = d.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-12 {
                continue;
            }
            let s = o - a;
            let u = s.dot(p) / det;
            if !(0. ..=1.).contains(&u) {
                continue;
            }
            let q = s.cross(e1);
            let v = d.dot(q) / det;
            if v < 0. || u + v > 1. {
                continue;
            }
            let t = e2.dot(q) / det;
            if t >= 0. && best.as_ref().is_none_or(|b| t < b.0) {
                best = Some((t, mesh.vertices[tri[0] as usize].color));
            }
        }
        best
    }
    #[test]
    fn cave_bridge_edits_and_moving_rays_match_canonical_exact_mesh() {
        let mut w = World::fbm_topology(
            7,
            FbmParams {
                octaves: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let b = w.topology_base_cells().unwrap() as f64 * VOXEL_SIZE;
        let cave = [-3., b - 2., -5.8];
        assert_eq!(
            w.material(Cell::from_meters([-3., b - 2., 0.])),
            Material::Air
        );
        assert_eq!(
            w.material(Cell::from_meters([4., b + 1.1, -1.5])),
            Material::Wood
        );
        let mut rays = vec![
            (cave, [0., 0., 1.]),
            ([4., b - 2., -1.5], [0., 1., 0.]),
            ([-3., b - 2., -6.8], [0., 0., 1.]),
        ];
        for camera_x in [-30., -9., -7.9, 0., 7.9, 9., 30., 100.] {
            let o = DVec3::new(camera_x, b + 12., -20.);
            let target = DVec3::new(4.03125, b + 1.125, -1.46875);
            rays.push((o.to_array(), (target - o).normalize().to_array()));
        }
        let added = Cell::from_meters([0.03125, b + 4., 0.03125]);
        for material in [Material::Air, Material::Wood, Material::Air] {
            w.set(added, material);
            let deck = Cell::from_meters([4.03125, b + 1.1, -1.46875]);
            w.set(
                deck,
                if material == Material::Wood {
                    Material::Air
                } else {
                    Material::Wood
                },
            );
            // Boundary edit exposes an outside neighbor wall; this face must be
            // retained even though its solid cell belongs to the outside owner.
            let edge = Cell {
                x: 127,
                y: (w.column(127, 0).height_m() / VOXEL_SIZE) as i64 - 1,
                z: 0,
            };
            w.set(edge, Material::Air);
            let m = build(&w).unwrap();
            eprintln!(
                "bounded volume: {} triangles, {} mesh bytes, revision {}",
                m.indices.len() / 3,
                m.vertices.len() * std::mem::size_of::<crate::mesh::Vertex>() + m.indices.len() * 4,
                w.revision()
            );
            let edge_o = [
                127.5 * VOXEL_SIZE,
                (edge.y as f64 + 0.5) * VOXEL_SIZE,
                0.5 * VOXEL_SIZE,
            ];
            let edge_d = DVec3::X;
            if let Some(exact) = raycast(&w, edge_o, edge_d.to_array(), 1.) {
                let got = mesh_hit(&m, DVec3::from(edge_o), edge_d)
                    .expect("outside neighbor exposed by boundary edit");
                assert!((got.0 - exact.distance).abs() < 1e-7);
            }
            for (o, d) in rays
                .iter()
                .copied()
                .chain([([0.03125, b + 7., 0.03125], [0., -1., 0.])])
            {
                let d = DVec3::from(d).normalize();
                let exact = raycast(&w, o, d.to_array(), 256.);
                let hit = mesh_hit(&m, DVec3::from(o), d);
                if let Some(exact) = exact.filter(|h| w.volume_owned(h.cell)) {
                    let got = hit.expect("volume-owned solid must have exact mesh coverage");
                    assert!(
                        (got.0 - exact.distance).abs() < 1e-7,
                        "ray {o:?} {d:?}: {got:?} vs {exact:?}"
                    );
                    assert_eq!(got.1, exact.material.color());
                }
            }
            // Each emitted face has canonical solid behind and air ahead,
            // including bottom and owner boundaries: no phantom crop caps.
            for quad in m.vertices.chunks_exact(4) {
                let center = quad.iter().fold(DVec3::ZERO, |a, v| {
                    a + DVec3::from(v.position.map(f64::from))
                }) / 4.
                    + DVec3::new(m.origin.x as f64, m.origin.y as f64, m.origin.z as f64)
                        * VOXEL_SIZE;
                let n = DVec3::from(quad[0].normal.map(f64::from)) * VOXEL_SIZE * 0.25;
                assert_ne!(
                    w.material(Cell::from_meters((center - n).to_array())),
                    Material::Air
                );
                assert_eq!(
                    w.material(Cell::from_meters((center + n).to_array())),
                    Material::Air
                );
            }
        }
    }
}
