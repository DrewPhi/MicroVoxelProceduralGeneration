//! Conservative triangle/box overlap on the model-local 6.25 cm lattice.
//! Airframe dimensions are in metres independently of the terrain grid.
use super::Vertex;
use frontier_world::{Cell, Material};
use glam::{IVec3, Vec3};
use std::collections::HashMap;

const EDGE: f32 = 0.0625;

pub(super) fn rasterize(skin: &[Vertex]) -> Vec<Vertex> {
    let mut cells = HashMap::<IVec3, (f32, [f32; 3])>::new();
    for face in skin.chunks_exact(3) {
        let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(face[i].position) / EDGE);
        let edges = [b - a, c - b, a - c];
        let normal = edges[0].cross(c - a).normalize_or_zero();
        if normal == Vec3::ZERO {
            continue;
        }
        // The candidate box already intersects the triangle's X, Y and Z
        // extents. Its face normal and its nine edge-cross-axis directions
        // supply the remaining separating axes for triangle/box overlap.
        let mut planes = [(Vec3::ZERO, 0., 0.); 10];
        for (slot, axis) in planes.iter_mut().zip(
            std::iter::once(normal).chain(
                edges
                    .into_iter()
                    .flat_map(|edge| [Vec3::X, Vec3::Y, Vec3::Z].map(|v| edge.cross(v))),
            ),
        ) {
            let radius = 0.5 * axis.abs().element_sum();
            let p = [a.dot(axis), b.dot(axis), c.dot(axis)];
            *slot = (
                axis,
                p[0].min(p[1]).min(p[2]) - radius,
                p[0].max(p[1]).max(p[2]) + radius,
            );
        }
        let lo = a.min(b).min(c).floor().as_ivec3();
        let hi = a.max(b).max(c).floor().as_ivec3();
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    let cell = IVec3::new(x, y, z);
                    let center = cell.as_vec3() + Vec3::splat(0.5);
                    if planes.iter().any(|&(axis, lo, hi)| {
                        let p = axis.dot(center);
                        p < lo || p > hi
                    }) {
                        continue;
                    }
                    let separation = normal.dot(center - a).abs();
                    let entry = cells.entry(cell).or_insert((f32::INFINITY, face[0].color));
                    if separation < entry.0 {
                        *entry = (separation, face[0].color);
                    }
                }
            }
        }
    }
    let mesh = crate::vegetation_mesher::mesh_colored_cells(
        cells.into_iter().map(|(p, (_, color))| {
            (
                Cell {
                    x: i64::from(p.x),
                    y: i64::from(p.y),
                    z: i64::from(p.z),
                },
                Material::Stone as u8,
                color,
            )
        }),
        1,
        1,
        EDGE,
        || false,
    );
    mesh.indices
        .into_iter()
        .map(|index| {
            let v = &mesh.vertices[index as usize];
            Vertex {
                position: v.position,
                normal: v.normal,
                color: v.color,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn airframe_and_propeller_have_axis_faces_and_retain_the_livery() {
        let (vertices, prop) = super::super::geometry();
        assert!(prop > 0 && (prop as usize) < vertices.len());
        for part in [&vertices[..prop as usize], &vertices[prop as usize..]] {
            for v in part {
                assert!(v.position.iter().all(|p| (p / EDGE).fract() == 0.));
                assert_eq!(v.normal.iter().map(|n| n.abs()).sum::<f32>(), 1.);
                assert!(v.normal.iter().all(|n| n.fract() == 0.));
            }
        }
        for paint in [
            super::super::WHITE,
            super::super::GLASS,
            super::super::ORANGE,
            super::super::BLACK,
        ] {
            assert!(vertices.iter().any(|v| v.color == paint));
        }
    }
}
