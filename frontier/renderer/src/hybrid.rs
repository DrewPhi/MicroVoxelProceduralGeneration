//! Unedited heightfield ownership experiments; this is not a volume handoff.
//! Moving coverage uses `build_collar`: native owns the open 64m square and a
//! two-metre collar joins cached native edges to actual filtered GPU samples.
//! `build` remains the fixed, unfiltered CPU reference used by older depth tests.
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

/// Build only the bounded geometric collar from cached exact native columns and
/// the actual filtered GPU page. No canonical source is sampled on this path.
pub(crate) fn build_collar(
    source: &FbmTerrain,
    center: [f64; 2],
    terrain: &crate::terrain::TerrainData,
    height: impl Fn(i64, i64) -> Option<i64>,
) -> Result<Mesh, String> {
    if center != terrain.collar_center() || center.iter().any(|v| v % 8. != 0.) {
        return Err("native and level-one terrain centers must match the same 8m lattice".into());
    }
    let mut mesh = Mesh {
        vertices: vec![],
        indices: vec![],
        origin: Cell {
            x: (center[0] / VOXEL_SIZE) as i64,
            y: 0,
            z: (center[1] / VOXEL_SIZE) as i64,
        },
        exposed_native_faces: 0,
    };
    let mut breaks: Vec<f64> = (0..=1024).map(|i| i as f64 / 1024.).collect();
    breaks.extend((0..=68).map(|i| i as f64 / 68.));
    breaks.sort_by(f64::total_cmp);
    breaks.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    for side in 0..4 {
        let edge = |r: f64, t: f64| {
            let v = -r + 2. * r * t;
            let p = match side {
                0 => [v, -r],
                1 => [r, v],
                2 => [-v, r],
                _ => [-r, -v],
            };
            [p[0] + center[0], p[1] + center[1]]
        };
        let native = |t: f64| -> Result<f64, String> {
            let p = edge(INNER, t);
            let min = center.map(|c| ((c - INNER) / VOXEL_SIZE) as i64);
            let x = (p[0] / VOXEL_SIZE)
                .floor()
                .clamp(min[0] as f64, (min[0] + 1023) as f64) as i64;
            let z = (p[1] / VOXEL_SIZE)
                .floor()
                .clamp(min[1] as f64, (min[1] + 1023) as f64) as i64;
            height(x, z)
                .map(|h| h as f64 * VOXEL_SIZE)
                .ok_or_else(|| format!("missing cached native collar column ({x},{z})"))
        };
        let outer = |t: f64| -> Result<([f64; 3], [f32; 3], [f32; 3]), String> {
            let p = edge(OUTER, t);
            let tangent = if side % 2 == 0 { p[0] } else { p[1] };
            let low = tangent.floor();
            let f = tangent - low;
            let (a, b) = if side % 2 == 0 {
                ([low, p[1]], [low + 1., p[1]])
            } else {
                ([p[0], low], [p[0], low + 1.])
            };
            let a = terrain
                .collar_sample(a[0], a[1])
                .ok_or("collar endpoint outside level-one page")?;
            let b = terrain
                .collar_sample(b[0], b[1])
                .ok_or("collar endpoint outside level-one page")?;
            let mix = |a: [f32; 3], b: [f32; 3]| {
                std::array::from_fn(|i| a[i] * (1. - f as f32) + b[i] * f as f32)
            };
            Ok((
                [p[0], a.0 * (1. - f) + b.0 * f, p[1]],
                mix(a.1, b.1),
                mix(a.2, b.2),
            ))
        };
        for w in breaks.windows(2) {
            let (t0, t1) = (w[0], w[1]);
            let h = native((t0 + t1) * 0.5)?;
            let (p, q) = (edge(INNER, t0), edge(INNER, t1));
            let a = (
                [p[0], h, p[1]],
                [0., 1., 0.],
                source.surface_color(p[0], p[1], h),
            );
            let b = (
                [q[0], h, q[1]],
                [0., 1., 0.],
                source.surface_color(q[0], q[1], h),
            );
            let (c, d) = (outer(t1)?, outer(t0)?);
            collar_triangle(&mut mesh, [a, b, d]);
            collar_triangle(&mut mesh, [b, c, d]);
            if t1 < 1. {
                let next = native((t1 + 1e-9).min(1.))?;
                if next != h {
                    let points = [b.0, [q[0], next, q[1]], c.0];
                    let n = (glam::DVec3::from(points[1]) - glam::DVec3::from(points[0]))
                        .cross(glam::DVec3::from(points[2]) - glam::DVec3::from(points[0]));
                    if n.length_squared() > 1e-20 {
                        let normal = n.normalize().as_vec3().to_array();
                        let high = h.max(next);
                        let top = source.surface_color(q[0], q[1], high);
                        let mut cuts = vec![h, next];
                        cuts.extend(
                            [high - VOXEL_SIZE, high - 12. * VOXEL_SIZE]
                                .into_iter()
                                .filter(|v| *v > h.min(next) && *v < high),
                        );
                        cuts.sort_by(f64::total_cmp);
                        if next < h {
                            cuts.reverse();
                        }
                        for segment in cuts.windows(2) {
                            let depth = high - (segment[0] + segment[1]) * 0.5;
                            let color = if top == frontier_world::Material::Stone.color() {
                                top
                            } else if depth < VOXEL_SIZE {
                                top
                            } else if depth < 12. * VOXEL_SIZE {
                                frontier_world::Material::Soil.color()
                            } else {
                                frontier_world::Material::Stone.color()
                            };
                            collar_triangle(
                                &mut mesh,
                                [
                                    ([q[0], segment[0], q[1]], normal, color),
                                    ([q[0], segment[1], q[1]], normal, color),
                                    c,
                                ],
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(mesh)
}
fn collar_triangle(mesh: &mut Mesh, points: [([f64; 3], [f32; 3], [f32; 3]); 3]) {
    let a = glam::DVec3::from(points[0].0);
    let b = glam::DVec3::from(points[1].0);
    let c = glam::DVec3::from(points[2].0);
    if (b - a).cross(c - a).length_squared() < 1e-20 {
        return;
    }
    let base = mesh.vertices.len() as u32;
    for (point, normal, color) in points {
        mesh.vertices.push(Vertex {
            position: [
                (point[0] - mesh.origin.x as f64 * VOXEL_SIZE) as f32,
                point[1] as f32,
                (point[2] - mesh.origin.z as f64 * VOXEL_SIZE) as f32,
            ],
            normal,
            color,
        });
    }
    mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
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
    fn moving_collar_uses_actual_filtered_gpu_endpoints_and_cached_columns() {
        let world = World::fbm(
            7,
            frontier_world::FbmParams {
                octaves: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let source = world.fbm_terrain().unwrap();
        let mut max_height_error = 0f64;
        for center in [[0., 0.], [8., 0.], [8., 8.], [-8., -16.], [65536., -65536.]] {
            let terrain = crate::terrain::TerrainData::at_center(source, center);
            let mesh = build_collar(source, center, &terrain, |x, z| {
                Some((world.column(x, z).height_m() / VOXEL_SIZE) as i64)
            })
            .unwrap();
            assert!(
                mesh.indices.len() < 80_000,
                "collar remains a small background job"
            );
            for v in &mesh.vertices {
                let x = v.position[0] as f64 + center[0];
                let z = v.position[2] as f64 + center[1];
                let radius = v.position[0].abs().max(v.position[2].abs());
                if radius == 34. && x.fract() == 0. && z.fract() == 0. {
                    let expected = terrain.collar_sample(x, z).unwrap();
                    max_height_error =
                        max_height_error.max((v.position[1] as f64 - expected.0).abs());
                    // Ignore thin step wedges, which keep a hard native riser.
                    if v.normal[1] > 0. {
                        assert_eq!(v.normal, expected.1);
                        assert_eq!(v.color, expected.2);
                    }
                }
            }
            for tri in mesh.indices.chunks_exact(3) {
                let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| &mesh.vertices[i as usize]);
                if a.position[0] == b.position[0]
                    && a.position[2] == b.position[2]
                    && a.normal[1].abs() < 1e-6
                    && a.position[1] != b.position[1]
                    && c.position[0].abs().max(c.position[2].abs()) == 34.
                {
                    let min = center.map(|v| ((v - 32.) / VOXEL_SIZE) as i64);
                    let x = ((center[0] + a.position[0] as f64
                        - a.normal[0] as f64 * VOXEL_SIZE * 0.25)
                        / VOXEL_SIZE)
                        .floor()
                        .clamp(min[0] as f64, (min[0] + 1023) as f64)
                        as i64;
                    let z = ((center[1] + a.position[2] as f64
                        - a.normal[2] as f64 * VOXEL_SIZE * 0.25)
                        / VOXEL_SIZE)
                        .floor()
                        .clamp(min[1] as f64, (min[1] + 1023) as f64)
                        as i64;
                    let y = (((a.position[1] + b.position[1]) as f64 * 0.5) / VOXEL_SIZE).floor()
                        as i64;
                    assert_eq!(
                        a.color,
                        world.material(Cell { x, y, z }).color(),
                        "canonical inner riser material at{x},{y},{z}"
                    );
                }
            }
            for side in 0..4 {
                for tangent in [-31.971, -1.013, 0.987, 12.031, 31.979] {
                    let p = match side {
                        0 => [tangent, -34.],
                        1 => [34., tangent],
                        2 => [tangent, 34.],
                        _ => [-34., tangent],
                    };
                    let (x, z) = (p[0] + center[0], p[1] + center[1]);
                    let lo = if side % 2 == 0 { x.floor() } else { z.floor() };
                    let f = if side % 2 == 0 { x - lo } else { z - lo };
                    let a = if side % 2 == 0 {
                        terrain.collar_sample(lo, z)
                    } else {
                        terrain.collar_sample(x, lo)
                    }
                    .unwrap();
                    let b = if side % 2 == 0 {
                        terrain.collar_sample(lo + 1., z)
                    } else {
                        terrain.collar_sample(x, lo + 1.)
                    }
                    .unwrap();
                    let expected = a.0 * (1. - f) + b.0 * f;
                    let (rx, rz) = if side % 2 == 0 {
                        (x, z + if side == 0 { 1e-5 } else { -1e-5 })
                    } else {
                        (x + if side == 1 { -1e-5 } else { 1e-5 }, z)
                    };
                    let found = hits(&mesh, DVec3::new(rx, expected + 100., rz), DVec3::NEG_Y);
                    assert_eq!(found.len(), 1, "collar owner side={side} center={center:?}");
                    let error = (100. - found[0]).abs();
                    max_height_error = max_height_error.max(error);
                    assert!(error < 0.001, "outer depth mismatch {error}m");
                }
            }
        }
        eprintln!("moving collar 5 centers: endpoint/adjacent depth max={max_height_error:.8}m; <= {:.5}px at34m/1080p60deg; exact outer normals/colors", max_height_error*935.307/34.);
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
