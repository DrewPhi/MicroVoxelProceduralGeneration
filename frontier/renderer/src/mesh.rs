//! Exact native-cell surface extraction. The bounded fixture is deliberately not a LOD.
use bytemuck::{Pod, Zeroable};
use frontier_world::{Cell, Material, VOXEL_SIZE};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub normal: [f32; 3],
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Integer origin retained in CPU precision; vertex positions are local meters.
    pub origin: Cell,
    pub exposed_native_faces: usize,
}

/// Samples a one-cell halo so crop boundaries never invent faces inside solid terrain.
/// Greedy rectangles only join equal materials and normals: no voxel is approximated.
pub fn extract(mut sample: impl FnMut(Cell) -> Material, min: Cell, size: [usize; 3]) -> Mesh {
    let halo = [size[0] + 2, size[1] + 2, size[2] + 2];
    let index = |p: [usize; 3]| (p[2] * halo[1] + p[1]) * halo[0] + p[0];
    let mut cells = vec![Material::Air; halo.iter().product()];
    for z in 0..halo[2] {
        for y in 0..halo[1] {
            for x in 0..halo[0] {
                cells[index([x, y, z])] = sample(Cell {
                    x: min.x + x as i64 - 1,
                    y: min.y + y as i64 - 1,
                    z: min.z + z as i64 - 1,
                });
            }
        }
    }
    let at = |p: [usize; 3]| cells[index([p[0] + 1, p[1] + 1, p[2] + 1])];
    let mut mesh = Mesh {
        vertices: vec![],
        indices: vec![],
        origin: min,
        exposed_native_faces: 0,
    };
    for axis in 0..3 {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        for positive in [false, true] {
            for layer in 0..size[axis] {
                let mut mask = vec![None; size[u] * size[v]];
                for j in 0..size[v] {
                    for i in 0..size[u] {
                        let mut p = [0; 3];
                        p[axis] = layer;
                        p[u] = i;
                        p[v] = j;
                        let material = at(p);
                        let mut neighbor = [p[0] + 1, p[1] + 1, p[2] + 1];
                        if positive {
                            neighbor[axis] += 1;
                        } else {
                            neighbor[axis] -= 1;
                        }
                        if material != Material::Air && cells[index(neighbor)] == Material::Air {
                            mask[j * size[u] + i] = Some(material);
                            mesh.exposed_native_faces += 1;
                        }
                    }
                }
                for j in 0..size[v] {
                    let mut i = 0;
                    while i < size[u] {
                        let Some(material) = mask[j * size[u] + i] else {
                            i += 1;
                            continue;
                        };
                        let mut width = 1;
                        while i + width < size[u] && mask[j * size[u] + i + width] == Some(material)
                        {
                            width += 1;
                        }
                        let mut height = 1;
                        'rows: while j + height < size[v] {
                            for x in i..i + width {
                                if mask[(j + height) * size[u] + x] != Some(material) {
                                    break 'rows;
                                }
                            }
                            height += 1;
                        }
                        for y in j..j + height {
                            for x in i..i + width {
                                mask[y * size[u] + x] = None;
                            }
                        }
                        let mut p = [0.; 3];
                        p[axis] = (layer + usize::from(positive)) as f32;
                        p[u] = i as f32;
                        p[v] = j as f32;
                        let mut normal = [0.; 3];
                        normal[axis] = if positive { 1. } else { -1. };
                        let base = mesh.vertices.len() as u32;
                        for (du, dv) in [
                            (0., 0.),
                            (width as f32, 0.),
                            (width as f32, height as f32),
                            (0., height as f32),
                        ] {
                            let mut position = p;
                            position[u] += du;
                            position[v] += dv;
                            for value in &mut position {
                                *value *= VOXEL_SIZE as f32;
                            }
                            mesh.vertices.push(Vertex {
                                position,
                                color: material.color(),
                                normal,
                            });
                        }
                        let corners = if positive {
                            [0, 1, 2, 0, 2, 3]
                        } else {
                            [0, 2, 1, 0, 3, 2]
                        };
                        mesh.indices.extend(corners.map(|c| base + c));
                        i += width;
                    }
                }
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn solid_block_has_six_greedy_quads_and_exact_native_area() {
        let m = extract(
            |p| {
                if (0..3).contains(&p.x) && (0..2).contains(&p.y) && (0..4).contains(&p.z) {
                    Material::Stone
                } else {
                    Material::Air
                }
            },
            Cell { x: 0, y: 0, z: 0 },
            [3, 2, 4],
        );
        assert_eq!(m.indices.len(), 36);
        assert_eq!(m.exposed_native_faces, 2 * (3 * 2 + 2 * 4 + 3 * 4));
        let area: f32 = m
            .indices
            .chunks_exact(3)
            .map(|t| {
                let a = glam::Vec3::from(m.vertices[t[0] as usize].position);
                let b = glam::Vec3::from(m.vertices[t[1] as usize].position);
                let c = glam::Vec3::from(m.vertices[t[2] as usize].position);
                let cross = (b - a).cross(c - a);
                assert!(cross.dot(glam::Vec3::from(m.vertices[t[0] as usize].normal)) > 0.);
                cross.length() / 2.
            })
            .sum();
        assert!((area - m.exposed_native_faces as f32 * (VOXEL_SIZE as f32).powi(2)).abs() < 1e-6);
    }
    #[test]
    fn halo_does_not_cap_a_solid_crop() {
        let m = extract(
            |_| Material::Stone,
            Cell {
                x: -1000000,
                y: 0,
                z: 0,
            },
            [2; 3],
        );
        assert!(m.indices.is_empty());
    }
    #[test]
    fn holes_and_material_seams_preserve_face_coverage() {
        let sample = |p: Cell| {
            if p.x >= 0 && p.x < 3 && p.y >= 0 && p.y < 3 && p.z == 0 && !(p.x == 1 && p.y == 1) {
                if p.x == 0 {
                    Material::Wood
                } else {
                    Material::Stone
                }
            } else {
                Material::Air
            }
        };
        let m = extract(sample, Cell { x: 0, y: 0, z: 0 }, [3, 3, 1]);
        assert_eq!(m.exposed_native_faces, 32); // front/back 16, outer perimeter 12, inner hole 4
        let area: f32 = m
            .indices
            .chunks_exact(3)
            .map(|t| {
                let p = |i| glam::Vec3::from(m.vertices[t[i] as usize].position);
                (p(1) - p(0)).cross(p(2) - p(0)).length() / 2.
            })
            .sum();
        assert!((area / (VOXEL_SIZE as f32).powi(2) - 32.).abs() < 1e-5);
    }
}
