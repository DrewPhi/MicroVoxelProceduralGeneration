//! Fixed-boundary, unedited heightfield experiment. This is not a volume handoff.
//! Native owns the open 64m square. A two-metre approximate collar replaces the
//! crop's exterior-facing walls and joins the explicitly cut-out static proxy.
use crate::{
    column_mesh,
    mesh::{Mesh, Vertex},
};
use frontier_world::{Cell, FbmTerrain, World, VOXEL_SIZE};

const INNER: f64 = 32.;
const OUTER: f64 = 34.;
const CELLS: i64 = 512;

pub fn build(world: &World, levels: u32) -> Result<Mesh, String> {
    if !(crate::terrain::MIN_LEVELS..=crate::terrain::MAX_LEVELS).contains(&levels) {
        return Err("hybrid view levels must be 7..11".into());
    }
    let source = world.fbm_terrain().ok_or("hybrid requires FBM")?;
    let native = column_mesh::build(
        world,
        Cell {
            x: -CELLS,
            y: 0,
            z: -CELLS,
        },
        [1024, 1024],
    )?;
    eprintln!(
        "hybrid canonical column snapshot: {} sampled columns",
        native.heights.sampled_columns()
    );
    let mut mesh = native.mesh;
    // Column extraction emits contiguous quads. Boundary side walls would be
    // dangling geometry after changing the exterior owner to the collar.
    let original_indices = std::mem::take(&mut mesh.indices);
    for (q, vertices) in mesh.vertices.chunks_exact(4).enumerate() {
        let boundary = [0, 2].into_iter().any(|axis| {
            vertices[0].normal[axis] != 0.
                && vertices
                    .iter()
                    .all(|v| v.position[axis] == 0. || v.position[axis] == 64.)
        });
        if !boundary {
            mesh.indices
                .extend_from_slice(&original_indices[q * 6..q * 6 + 6]);
        }
    }
    // Preserve local origin while adding world-space proxy and collar vertices.
    for level in 1..levels {
        let step = 0.5 * (1u32 << level) as f64;
        for z in -64..64 {
            for x in -64..64 {
                let hole = if level == 1 { 34 } else { 32 };
                if (-hole..hole).contains(&x) && (-hole..hole).contains(&z) {
                    continue;
                }
                let point = |mut gx: i32, mut gz: i32| {
                    if gx.abs() == 64 && gz % 2 != 0 {
                        gz -= 1;
                    }
                    if gz.abs() == 64 && gx % 2 != 0 {
                        gx -= 1;
                    }
                    let (wx, wz) = (gx as f64 * step, gz as f64 * step);
                    [wx, source.height_m(wx, wz), wz]
                };
                let a = point(x, z);
                let b = point(x, z + 1);
                let c = point(x + 1, z + 1);
                let d = point(x + 1, z);
                // Corner triangulation matches collapsed edge topology.
                if x == 63 && z == 63 {
                    triangle(&mut mesh, source, [a, b, c]);
                    triangle(&mut mesh, source, [a, c, d]);
                } else {
                    triangle(&mut mesh, source, [a, b, d]);
                    triangle(&mut mesh, source, [d, b, c]);
                }
            }
        }
    }
    // Parameter runs counter-clockwise seen from above, inner square to outer.
    // Union breakpoints: native column changes AND actual outer grid vertices.
    let mut breaks: Vec<f64> = (0..=1024).map(|i| i as f64 / 1024.).collect();
    breaks.extend((0..=68).map(|i| i as f64 / 68.));
    breaks.sort_by(f64::total_cmp);
    breaks.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    for side in 0..4 {
        let edge = |radius: f64, t: f64| -> [f64; 2] {
            let v = -radius + 2. * radius * t;
            match side {
                0 => [v, -radius],
                1 => [radius, v],
                2 => [-v, radius],
                _ => [-radius, -v],
            }
        };
        let height = |t: f64| {
            let p = edge(INNER, t);
            let x = (p[0] / VOXEL_SIZE).floor().clamp(-512., 511.) as i64;
            let z = (p[1] / VOXEL_SIZE).floor().clamp(-512., 511.) as i64;
            native.heights.height(x, z).unwrap() as f64 * VOXEL_SIZE
        };
        let outer = |t: f64| {
            let p = edge(OUTER, t);
            let tangent = if side % 2 == 0 { p[0] } else { p[1] };
            let low = tangent.floor();
            let f = tangent - low;
            let (a, b) = if side % 2 == 0 {
                ([low, p[1]], [low + 1., p[1]])
            } else {
                ([p[0], low], [p[0], low + 1.])
            };
            [
                p[0],
                source.height_m(a[0], a[1]) * (1. - f) + source.height_m(b[0], b[1]) * f,
                p[1],
            ]
        };
        for window in breaks.windows(2) {
            let (t0, t1) = (window[0], window[1]);
            let h = height((t0 + t1) * 0.5);
            let (p, q) = (edge(INNER, t0), edge(INNER, t1));
            let a = [p[0], h, p[1]];
            let b = [q[0], h, q[1]];
            let (c, d) = (outer(t1), outer(t0));
            triangle(&mut mesh, source, [a, b, d]);
            triangle(&mut mesh, source, [b, c, d]);
            if t1 < 1. {
                let next = height((t1 + 1e-9).min(1.));
                if next != h {
                    // Actual step riser, not a downward skirt. Shared outer
                    // endpoint makes this wedge vanish exactly at the proxy.
                    let tri = [b, [q[0], next, q[1]], c];
                    // Normal must face the lower segment (the exterior air).
                    triangle(&mut mesh, source, tri);
                }
            }
        }
    }
    eprintln!("hybrid: fixed native x/z=[-32,32)m; approximate collar to ±34m; static proxy ±{}m; {} triangles; no caves, edits, moving ownership, or certified proxy error", 0.5*(1u32<<(levels-1)) as f64*64.,mesh.indices.len()/3);
    Ok(mesh)
}

fn triangle(mesh: &mut Mesh, source: &FbmTerrain, p: [[f64; 3]; 3]) {
    let a = glam::DVec3::from_array(p[0]);
    let b = glam::DVec3::from_array(p[1]);
    let c = glam::DVec3::from_array(p[2]);
    let n = (b - a).cross(c - a);
    if n.length_squared() < 1e-20 {
        return;
    }
    let normal = n.normalize().as_vec3().to_array();
    let base = mesh.vertices.len() as u32;
    for point in p {
        mesh.vertices.push(Vertex {
            position: [
                (point[0] - mesh.origin.x as f64 * VOXEL_SIZE) as f32,
                (point[1] - mesh.origin.y as f64 * VOXEL_SIZE) as f32,
                (point[2] - mesh.origin.z as f64 * VOXEL_SIZE) as f32,
            ],
            normal,
            color: source.surface_color(point[0], point[2], point[1]),
        });
    }
    mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    fn fixture() -> &'static (World, Mesh) {
        static FIXTURE: std::sync::OnceLock<(World, Mesh)> = std::sync::OnceLock::new();
        FIXTURE.get_or_init(|| {
            let w = World::fbm(7, Default::default()).unwrap();
            let m = build(&w, 7).unwrap();
            (w, m)
        })
    }
    fn hits(mesh: &Mesh, origin: DVec3, direction: DVec3) -> Vec<f64> {
        let offset = DVec3::new(
            mesh.origin.x as f64,
            mesh.origin.y as f64,
            mesh.origin.z as f64,
        ) * VOXEL_SIZE;
        let point =
            |i: u32| DVec3::from_array(mesh.vertices[i as usize].position.map(f64::from)) + offset;
        let mut result = Vec::new();
        for t in mesh.indices.chunks_exact(3) {
            let a = point(t[0]);
            let e1 = point(t[1]) - a;
            let e2 = point(t[2]) - a;
            let p = direction.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-12 {
                continue;
            }
            let s = origin - a;
            let u = s.dot(p) / det;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = s.cross(e1);
            let v = direction.dot(q) / det;
            if v < 0. || u + v > 1. {
                continue;
            }
            let distance = e2.dot(q) / det;
            if distance >= 0. {
                result.push(distance);
            }
        }
        result.sort_by(f64::total_cmp);
        result
    }
    #[test]
    fn native_interior_depth_matches_canonical_dda_at_negative_and_positive_edges() {
        let (world, mesh) = fixture();
        for (x, z) in [
            (-31.723, -12.163),
            (31.713, 13.173),
            (-11.219, -31.731),
            (12.177, 31.711),
            (0.023, 0.019),
        ] {
            let origin = DVec3::new(x, world.height_m(x, z) + 2., z);
            for direction in [DVec3::NEG_Y, DVec3::new(0.1, -1., 0.08).normalize()] {
                let expected =
                    frontier_world::raycast(world, origin.to_array(), direction.to_array(), 4.)
                        .unwrap();
                let found = hits(mesh, origin, direction);
                assert!(!found.is_empty(), "missing native ray {origin:?}");
                assert!(
                    (found[0] - expected.distance).abs() < 1e-5,
                    "native ray depth {:?} vs {}",
                    found,
                    expected.distance
                );
            }
        }
    }
    #[test]
    fn collar_and_proxy_cutout_have_one_top_owner_including_grid_breaks_and_corners() {
        let (world, mesh) = fixture();
        for (x, z) in [
            (32.031, 1.237),
            (33.371, 1.237),
            (34.001, 1.237),
            (-32.031, 1.237),
            (-33.371, -1.237),
            (-34.001, -1.237),
            (1.237, 32.031),
            (1.237, 33.371),
            (1.237, 34.001),
            (1.237, -32.031),
            (1.237, -33.371),
            (1.237, -34.001),
            (33.123, 33.171),
            (-33.123, 33.171),
            (-33.123, -33.171),
            (33.123, -33.171),
            (33.999, 0.999),
            (34.001, 1.001),
        ] {
            let origin = DVec3::new(x, world.height_m(x, z) + 100., z);
            let found = hits(mesh, origin, DVec3::NEG_Y);
            assert_eq!(
                found.len(),
                1,
                "gap/double top ownership at ({x},{z}): {found:?}"
            );
        }
    }
    #[test]
    fn native_collar_and_proxy_edges_close_at_actual_triangle_breaks() {
        let (world, mesh) = fixture();
        for side in 0..4 {
            for tangent in [-31.971_f64, -1.013, 0.987, 12.031, 31.979] {
                for radius in [32., 34.] {
                    let (x, z) = match side {
                        0 => (tangent, -radius),
                        1 => (radius, tangent),
                        2 => (tangent, radius),
                        _ => (-radius, tangent),
                    };
                    let expected = if radius == 32. {
                        world.height_m(
                            x.clamp(-31.999999, 31.999999),
                            z.clamp(-31.999999, 31.999999),
                        )
                    } else {
                        let lo = tangent.floor();
                        let f = tangent - lo;
                        let (a, b) = if side % 2 == 0 {
                            ((lo, z), (lo + 1., z))
                        } else {
                            ((x, lo), (x, lo + 1.))
                        };
                        world.height_m(a.0, a.1) * (1. - f) + world.height_m(b.0, b.1) * f
                    };
                    for epsilon in [-0.00001, 0.00001] {
                        let (rx, rz) = if side % 2 == 0 {
                            (x, z + epsilon)
                        } else {
                            (x + epsilon, z)
                        };
                        let origin = DVec3::new(rx, expected + 100., rz);
                        let found = hits(mesh, origin, DVec3::NEG_Y);
                        assert_eq!(
                            found.len(),
                            1,
                            "edge owner mismatch side={side} radius={radius} tangent={tangent}"
                        );
                        assert!(
                            (origin.y - found[0] - expected).abs() < 0.001,
                            "edge depth mismatch side={side} radius={radius} tangent={tangent}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn proxy_outer_edges_share_exact_parent_anchor_heights() {
        let (world, mesh) = fixture();
        for radius in [64., 128., 256., 512., 1024.] {
            let step = radius / 32.;
            let z = 0.371 * step;
            let expected =
                world.height_m(radius, 0.) * (1. - 0.371) + world.height_m(radius, step) * 0.371;
            for dx in [-0.0001, 0.0001] {
                let origin = DVec3::new(radius + dx, expected + 100., z);
                let found = hits(mesh, origin, DVec3::NEG_Y);
                assert_eq!(found.len(), 1, "ring gap/double at {radius}");
                assert!(
                    (origin.y - found[0] - expected).abs() < 0.01,
                    "ring seam depth mismatch at {radius}"
                );
            }
        }
    }
}
