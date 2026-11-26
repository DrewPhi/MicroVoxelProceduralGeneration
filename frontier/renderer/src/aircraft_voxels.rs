//! Conservative triangle/box overlap on the model-local 6.25 cm lattice.
//! Airframe dimensions are in metres independently of the terrain grid.
use super::{cabin::EDGE, Vertex};
use frontier_world::{Cell, Material};
use glam::{IVec3, Vec3};
use std::collections::HashMap;

pub(super) type Voxels = HashMap<IVec3, [f32; 3]>;

pub(super) fn rasterize(skin: &[Vertex]) -> Voxels {
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
    // Paint the tapering rear fuselage on the occupied cells. Read the small
    // F7 identifier left-to-right from either side without floating decals.
    let paint = |p: IVec3, base| {
        if (-82..-33).contains(&p.z) && p.x.abs() >= 2 {
            let along = if p.x > 0 { p.z + 60 } else { -p.z - 50 };
            let row = 6 - p.y;
            if (0..7).contains(&along) && (0..5).contains(&row) {
                let badge = [0b1110111, 0b1000001, 0b1100010, 0b1000010, 0b1000100];
                if badge[row as usize] & (1 << (6 - along)) != 0 {
                    return super::BLACK;
                }
            }
            let stripe = (-p.z - 33) / 8 - 6;
            if p.y == stripe + 2 {
                return super::PINSTRIPE;
            }
            if (stripe - 1..=stripe + 1).contains(&p.y) {
                return super::ACCENT;
            }
        }
        base
    };
    cells
        .into_iter()
        .map(|(p, (_, color))| (p, paint(p, color)))
        .collect()
}

pub(super) fn mesh(cells: Voxels) -> Vec<Vertex> {
    let mesh = crate::vegetation_mesher::mesh_colored_cells(
        cells.into_iter().map(|(p, color)| {
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
        let mesh = super::super::geometry();
        let vertices = &mesh.vertices[..mesh.opaque_count as usize];
        assert!(vertices
            .iter()
            .any(|v| v.part == super::super::Part::Prop as u32));
        for v in vertices.iter().filter(|v| v.display == 0) {
            assert!(v.position.iter().all(|p| (p / EDGE).fract() == 0.));
            assert_eq!(v.normal.iter().map(|n| n.abs()).sum::<f32>(), 1.);
            assert!(v.normal.iter().all(|n| n.fract() == 0.));
        }
        // Windscreen, rear window, three fixed panes and one door pane
        // per side, plus the blurred propeller disc.
        assert_eq!(mesh.panes.len(), 11);
        for paint in [
            super::super::WHITE,
            super::super::ACCENT,
            super::super::BLACK,
        ] {
            assert!(vertices.iter().any(|v| v.color == paint));
        }
    }
}
