//! Exact rectangle triangulation with a conforming boundary graph.
//!
//! Greenfield implementation. Every source corner splits every incident edge,
//! including edges of perpendicular faces. Independent tile boundaries retain
//! unit lattice segments. Convex rectangle fans preserve every boundary segment;
//! no Delaunay library or floating-point geometric predicate is needed.
use super::{GeometryTile, ProxyTriangle, MAX_NATIVE_QUADS};
use frontier_world::VOXEL_SIZE;
use std::collections::HashMap;

type Point = [i64; 3];
type Line = (usize, Point);
fn line(a: Point, b: Point) -> (Line, usize) {
    let axis = (0..3).find(|&i| a[i] != b[i]).unwrap();
    let mut fixed = a;
    fixed[axis] = 0;
    ((axis, fixed), axis)
}
fn point(p: [f32; 3]) -> Result<Point, String> {
    let mut out = [0; 3];
    for i in 0..3 {
        let n = p[i] as f64 / VOXEL_SIZE;
        if !n.is_finite() || n.abs() > (1i64 << 40) as f64 || n != n.round() {
            return Err("native corner is outside exact lattice".into());
        }
        out[i] = n as i64;
    }
    Ok(out)
}
/// None means compact output would exceed the unit baseline's byte count.
/// Cancellation never publishes a partially constructed record stream.
pub(super) fn triangulate(
    tile: &GeometryTile,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Option<Vec<ProxyTriangle>>, String> {
    if tile.mesh.vertices.len() % 4 != 0 {
        return Err("native quad packing requires canonical rectangles".into());
    }
    let mut rectangles = Vec::with_capacity(tile.mesh.vertices.len() / 4);
    let mut lines: HashMap<Line, Vec<i64>> = HashMap::new();
    let mut unit_faces = 0usize;
    for (index, q) in tile.mesh.vertices.chunks_exact(4).enumerate() {
        if index % 128 == 0 && cancelled() {
            return Err("adaptive geometry cancelled".into());
        }
        let corners = [
            point(q[0].position)?,
            point(q[1].position)?,
            point(q[2].position)?,
            point(q[3].position)?,
        ];
        let axis = q[0]
            .normal
            .iter()
            .position(|&n| n != 0.)
            .ok_or("native quad has no normal")?;
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let mut expected = corners[0];
        expected[u] = corners[2][u];
        let mut expected_v = corners[0];
        expected_v[v] = corners[2][v];
        if corners[2][u] <= corners[0][u]
            || corners[2][v] <= corners[0][v]
            || corners[1] != expected
            || corners[3] != expected_v
            || corners[2][axis] != corners[0][axis]
        {
            return Err("invalid native rectangle".into());
        }
        let area = usize::try_from((corners[2][u] - corners[0][u]).abs())
            .ok()
            .and_then(|a| a.checked_mul((corners[2][v] - corners[0][v]).unsigned_abs() as usize))
            .ok_or("native tessellation size overflow")?;
        unit_faces = unit_faces
            .checked_add(area)
            .ok_or("native tessellation size overflow")?;
        // A perpendicular face's corner may touch an edge without owning
        // any collinear edge. Index each corner on all three supporting lines.
        for corner in corners {
            for dim in 0..3 {
                let mut fixed = corner;
                fixed[dim] = 0;
                lines.entry((dim, fixed)).or_default().push(corner[dim]);
            }
        }
        rectangles.push(corners);
    }
    for values in lines.values_mut() {
        values.sort_unstable();
        values.dedup();
    }
    let max_triangles =
        unit_faces.min(MAX_NATIVE_QUADS).saturating_mul(32) / std::mem::size_of::<ProxyTriangle>();
    let mut triangles = Vec::new();
    for (index, (q, corners)) in tile
        .mesh
        .vertices
        .chunks_exact(4)
        .zip(rectangles)
        .enumerate()
    {
        if index % 32 == 0 && cancelled() {
            return Err("adaptive geometry cancelled".into());
        }
        let mut boundary = Vec::new();
        for j in 0..4 {
            let a = corners[j];
            let b = corners[(j + 1) % 4];
            let (key, dim) = line(a, b);
            let low = a[dim].min(b[dim]);
            let high = a[dim].max(b[dim]);
            // Local x/z boundaries are independent of neighbor build order.
            let on_boundary = [0, 2]
                .into_iter()
                .any(|d| d != dim && (a[d] == 0 || a[d] == tile.key.width_cells as i64));
            let mut cuts: Vec<i64> = if on_boundary {
                if high - low
                    >= max_triangles.saturating_sub(triangles.len() + boundary.len()) as i64
                {
                    return Ok(None);
                }
                let mut cuts = Vec::new();
                for n in low..=high {
                    if (n - low) % 256 == 0 && cancelled() {
                        return Err("adaptive geometry cancelled".into());
                    }
                    cuts.push(n);
                }
                cuts
            } else {
                let values = &lines[&key];
                let start = values.partition_point(|&n| n < low);
                let end = values.partition_point(|&n| n <= high);
                if end - start >= max_triangles.saturating_sub(triangles.len() + boundary.len()) {
                    return Ok(None);
                }
                values[start..end].to_vec()
            };
            if a[dim] > b[dim] {
                cuts.reverse();
            }
            cuts.pop(); // Next side owns its starting corner.
            for n in cuts {
                let mut p = a;
                p[dim] = n;
                boundary.push(p);
            }
        }
        if triangles.len().saturating_add(boundary.len()) >= max_triangles {
            return Ok(None);
        }
        let center: [f32; 3] = std::array::from_fn(|d| {
            ((corners[0][d] + corners[2][d]) as f64 * (VOXEL_SIZE * 0.5)) as f32
        });
        if (0..3).any(|d| {
            center[d] as f64 != (corners[0][d] + corners[2][d]) as f64 * (VOXEL_SIZE * 0.5)
        }) {
            return Ok(None);
        }
        let profile_index = tile
            .face_profile_indices
            .get(index * 4)
            .copied()
            .unwrap_or(0);
        if q[0].normal.iter().any(|&n| n < 0.) {
            boundary.reverse();
        }
        // Reverse negative faces: source corners use positive-axis ordering.
        for j in 0..boundary.len() {
            if j % 256 == 0 && cancelled() {
                return Err("adaptive geometry cancelled".into());
            }
            let position = |p: Point| p.map(|n| (n as f64 * VOXEL_SIZE) as f32);
            triangles.push(ProxyTriangle {
                positions: [
                    center,
                    position(boundary[j]),
                    position(boundary[(j + 1) % boundary.len()]),
                ],
                normal: q[0].normal,
                color: q[0].color,
                profile_index,
            });
        }
    }
    if cancelled() {
        return Err("adaptive geometry cancelled".into());
    }
    triangles.shrink_to_fit();
    Ok(Some(triangles))
}

#[cfg(test)]
mod tests {
    use super::super::{build_surface, SourceSamples, TileKey};
    use super::*;
    use crate::mesh::Vertex;
    use frontier_world::{FbmParams, Material, World};
    fn tile() -> GeometryTile {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let key = TileKey {
            min: [0, 0],
            width_cells: 8,
            step_cells: 1,
            edge_steps: [1; 4],
            source_revision: world.revision(),
        };
        let mut tile = build_surface(&world, key, || false, &mut SourceSamples::new(0)).unwrap();
        tile.mesh.vertices.clear();
        tile.mesh.indices.clear();
        tile.face_profile_indices.clear();
        tile.key.width_cells = 1024;
        tile
    }
    fn rectangle(
        tile: &mut GeometryTile,
        axis: usize,
        negative: bool,
        anchor: Point,
        extent: [i64; 2],
        profile: u32,
    ) {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let mut normal = [0.; 3];
        normal[axis] = if negative { -1. } else { 1. };
        for [du, dv] in [[0, 0], [extent[0], 0], extent, [0, extent[1]]] {
            let mut p = anchor;
            p[u] += du;
            p[v] += dv;
            tile.mesh.vertices.push(Vertex {
                position: p.map(|n| (n as f64 * VOXEL_SIZE) as f32),
                normal,
                color: Material::Stone.color(),
            });
            tile.face_profile_indices.push(profile);
        }
    }
    fn closed_box(split: bool, size: [i64; 3]) -> GeometryTile {
        let mut tile = tile();
        let min = [4, 4, 4];
        for axis in 0..3 {
            for negative in [false, true] {
                let mut anchor = min;
                if !negative {
                    anchor[axis] += size[axis];
                }
                let u = (axis + 1) % 3;
                let v = (axis + 2) % 3;
                if split && axis == 0 && !negative {
                    rectangle(&mut tile, axis, negative, anchor, [size[u] / 2, size[v]], 1);
                    anchor[u] += size[u] / 2;
                    rectangle(
                        &mut tile,
                        axis,
                        negative,
                        anchor,
                        [size[u] - size[u] / 2, size[v]],
                        2,
                    );
                } else {
                    rectangle(&mut tile, axis, negative, anchor, [size[u], size[v]], 3);
                }
            }
        }
        tile
    }
    #[test]
    fn compact_cliff_is_exact_conforming_for_all_orientations() {
        let tile = closed_box(true, [80, 800, 80]);
        let triangles = triangulate(&tile, || false).unwrap().unwrap();
        assert!(triangles.len() < 100);
        // Every triangle points outwards and keeps its source pigment/profile.
        let mut area = 0f64;
        for t in &triangles {
            let p = t.positions.map(|p| glam::Vec3::from(p).as_dvec3());
            let cross = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(cross.dot(glam::Vec3::from(t.normal).as_dvec3()) > 0.);
            assert_eq!(t.color, Material::Stone.color());
            assert!((1..=3).contains(&t.profile_index));
            area += cross.length() * 0.5;
        }
        let expected = 2. * (80. * 800. + 80. * 80. + 800. * 80.) * VOXEL_SIZE * VOXEL_SIZE;
        assert_eq!(area, expected);
        // A closed shell has every geometric segment twice, with opposite
        // orientation. This detects unmatched partitions and missing faces.
        let mut edges: HashMap<([u32; 3], [u32; 3]), (i32, u32)> = HashMap::new();
        for t in &triangles {
            for j in 0..3 {
                let a = t.positions[j].map(f32::to_bits);
                let b = t.positions[(j + 1) % 3].map(f32::to_bits);
                let (key, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                let e = edges.entry(key).or_default();
                e.0 += sign;
                e.1 += 1;
            }
        }
        assert!(edges
            .values()
            .all(|&(signed, count)| signed == 0 && count == 2));
        assert!(triangles.len() * 64 < 2 * (80 * 800 + 80 * 80 + 800 * 80) * 32 / 100);
    }
    #[test]
    fn every_original_unit_face_has_matching_triangle_coverage_and_profile() {
        let tile = closed_box(true, [8, 20, 8]);
        let triangles = triangulate(&tile, || false).unwrap().unwrap();
        for (index, q) in tile.mesh.vertices.chunks_exact(4).enumerate() {
            let axis = q[0].normal.iter().position(|&n| n != 0.).unwrap();
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            let lo = point(q[0].position).unwrap();
            let hi = point(q[2].position).unwrap();
            for a in lo[u]..hi[u] {
                for b in lo[v]..hi[v] {
                    let p = [(a as f64 + 0.5) * VOXEL_SIZE, (b as f64 + 0.5) * VOXEL_SIZE];
                    let mut covered = false;
                    for t in &triangles {
                        if t.normal != q[0].normal
                            || t.positions[0][axis] != q[0].position[axis]
                            || t.profile_index != tile.face_profile_indices[index * 4]
                        {
                            continue;
                        }
                        let p2 = t.positions.map(|p| [p[u] as f64, p[v] as f64]);
                        let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
                            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
                        };
                        let signs = [
                            cross(p2[0], p2[1], p),
                            cross(p2[1], p2[2], p),
                            cross(p2[2], p2[0], p),
                        ];
                        if signs.iter().all(|&v| v >= 0.) || signs.iter().all(|&v| v <= 0.) {
                            covered = true;
                            break;
                        }
                    }
                    assert!(covered, "unit face missing at {lo:?} {a} {b}");
                }
            }
        }
    }
    #[test]
    fn cancellation_does_not_mutate_canonical_input() {
        let tile = closed_box(true, [80, 800, 80]);
        let original = tile.mesh.vertices.len();
        let mut calls = 0;
        assert_eq!(
            triangulate(&tile, || {
                calls += 1;
                calls >= 3
            })
            .unwrap_err(),
            "adaptive geometry cancelled"
        );
        assert_eq!(tile.mesh.vertices.len(), original);
        assert!(tile.proxy_triangles.is_empty());
    }
    #[test]
    fn independent_tile_boundaries_keep_identical_unit_segments() {
        for axis in [0, 2] {
            let mut tile = tile();
            let other = if axis == 0 { 2 } else { 0 };
            let mut p = [4, 4, 4];
            p[axis] = 0;
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            rectangle(
                &mut tile,
                axis,
                false,
                p,
                [if u == 1 { 800 } else { 80 }, if v == 1 { 800 } else { 80 }],
                1,
            );
            // A face at a tile boundary must subdivide every perimeter edge,
            // even when the neighboring tile introduces different corners.
            let triangles = triangulate(&tile, || false).unwrap().unwrap();
            for t in triangles {
                let a = t.positions[1];
                let b = t.positions[2];
                let changed = (0..3).find(|&d| a[d] != b[d]).unwrap();
                assert!(changed == 1 || changed == other);
                assert_eq!((a[changed] - b[changed]).abs(), VOXEL_SIZE as f32);
            }
        }
    }
    #[test]
    fn malformed_and_overflowing_native_rectangles_fail_without_panic() {
        let mut tile = tile();
        rectangle(&mut tile, 1, false, [4, 4, 4], [1, 1], 0);
        tile.mesh.vertices[2].position = tile.mesh.vertices[0].position;
        assert!(triangulate(&tile, || false).is_err());
        tile.mesh.vertices.clear();
        tile.face_profile_indices.clear();
        rectangle(&mut tile, 1, false, [0, 0, 0], [1i64 << 40, 1i64 << 40], 0);
        assert!(triangulate(&tile, || false).is_err());
    }
    #[test]
    fn tiny_faces_keep_the_smaller_unit_record_path() {
        let mut tile = tile();
        rectangle(&mut tile, 1, false, [4, 4, 4], [1, 1], 0);
        assert!(triangulate(&tile, || false).unwrap().is_none());
    }
}
