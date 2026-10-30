//! Surface extraction consumes world cells. It never assigns materials by face.
use crate::{
    generation::Generator,
    grid::{Bounds, CellPos},
    materials::{MaterialRegistry, Voxel},
    world::{Lookup, World},
};

const MAX_FACES: usize = 120_000;
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
    pub normal: [i8; 4],
}
#[derive(Clone, Debug)]
pub struct Mesh {
    pub origin: CellPos,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}
struct Builder<'a> {
    cell_size: f32,
    materials: &'a MaterialRegistry,
    mesh: Mesh,
}
impl<'a> Builder<'a> {
    fn new(origin: CellPos, materials: &'a MaterialRegistry, cell_size: f32) -> Self {
        Self {
            cell_size,
            materials,
            mesh: Mesh {
                origin,
                vertices: Vec::new(),
                indices: Vec::new(),
            },
        }
    }
    fn quad(&mut self, corners: [[i64; 3]; 4], voxel: Voxel) -> Result<(), String> {
        if self.mesh.vertices.len() / 4 >= MAX_FACES {
            return Err("tile surface budget exceeded".into());
        }
        let a = corners[0];
        let b = corners[1];
        let c = corners[2];
        let ab = [
            (b[0] - a[0]) as f64,
            (b[1] - a[1]) as f64,
            (b[2] - a[2]) as f64,
        ];
        let ac = [
            (c[0] - a[0]) as f64,
            (c[1] - a[1]) as f64,
            (c[2] - a[2]) as f64,
        ];
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let length = n.iter().map(|v| v * v).sum::<f64>().sqrt();
        if length == 0.0 {
            return Ok(());
        }
        let normal = [
            (n[0] / length * 127.0).round() as i8,
            (n[1] / length * 127.0).round() as i8,
            (n[2] / length * 127.0).round() as i8,
            0,
        ];
        let rgb = crate::painting::base_color(voxel, self.materials);
        let color = [rgb[0], rgb[1], rgb[2], 255];
        let first = self.mesh.vertices.len() as u32;
        let origin = [self.mesh.origin.x, self.mesh.origin.y, self.mesh.origin.z];
        for corner in corners {
            self.mesh.vertices.push(Vertex {
                position: std::array::from_fn(|i| (corner[i] - origin[i]) as f32 * self.cell_size),
                color,
                normal,
            });
        }
        self.mesh.indices.extend_from_slice(&[
            first,
            first + 1,
            first + 2,
            first,
            first + 2,
            first + 3,
        ]);
        Ok(())
    }
}
const FACES: [[[i64; 3]; 4]; 6] = [
    [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]], // up
    [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]], // down
    [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]], // -X
    [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]], // +X
    [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]], // -Z
    [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]], // +Z
];
const NEIGHBORS: [[i64; 3]; 6] = [
    [0, 1, 0],
    [0, -1, 0],
    [-1, 0, 0],
    [1, 0, 0],
    [0, 0, -1],
    [0, 0, 1],
];

/// General, bounded 3D mesher: supports cavities, custom cells, and all water faces.
pub fn mesh_bounds<G: Generator>(world: &World<G>, bounds: Bounds) -> Result<Mesh, String> {
    if bounds.volume().ok_or("invalid mesh bounds")? > 1_000_000 {
        return Err("mesh volume budget exceeded".into());
    }
    let mut builder = Builder::new(
        bounds.min,
        world.materials(),
        world.spec().grid.meters() as f32,
    );
    for z in bounds.min.z..bounds.max.z {
        for x in bounds.min.x..bounds.max.x {
            for y in bounds.min.y..bounds.max.y {
                let p = CellPos::new(x, y, z);
                let Some(voxel) = world.voxel_at(p).known() else {
                    continue;
                };
                if voxel.is_air() {
                    continue;
                }
                for (face, delta) in NEIGHBORS.iter().enumerate() {
                    let Some(neighbor) = p.offset(*delta) else {
                        continue;
                    };
                    if !matches!(world.voxel_at(neighbor),Lookup::Known(v) if v.is_air()) {
                        continue;
                    }
                    let corners = FACES[face].map(|c| [x + c[0], y + c[1], z + c[2]]);
                    builder.quad(corners, voxel)?;
                }
            }
        }
    }
    Ok(builder.mesh)
}
