//! Exact surface extraction for an explicitly unedited FBM heightfield.
//! Work and storage depend on column count and exposed spans, never a vertical prism.
//! Caves, bridges and edits require the volume path and are rejected here.
use crate::mesh::{Mesh, Vertex};
use frontier_world::{Cell, Material, World, VOXEL_SIZE};

const MAX_HALO_COLUMNS: usize = 4_200_000;
const MAX_QUADS: usize = 4_000_000;
const MAX_RELIEF_CELLS: i64 = 65_536;
const MAX_LOCAL_CELLS: i64 = 16_777_216;

/// Canonical integer tops including a one-column halo. Reusable by boundary geometry.
pub struct ColumnHeights {
    pub min: [i64; 2],
    pub size: [usize; 2],
    values: Vec<i64>,
}
impl ColumnHeights {
    pub fn height(&self, x: i64, z: i64) -> Option<i64> {
        let dx = x as i128 - self.min[0] as i128 + 1;
        let dz = z as i128 - self.min[1] as i128 + 1;
        if dx < 0 || dz < 0 || dx >= (self.size[0] + 2) as i128 || dz >= (self.size[1] + 2) as i128
        {
            return None;
        }
        Some(self.values[dz as usize * (self.size[0] + 2) + dx as usize])
    }
    pub fn sampled_columns(&self) -> usize {
        self.values.len()
    }
}
pub struct ColumnMesh {
    pub mesh: Mesh,
    pub heights: ColumnHeights,
}

#[derive(Clone, Copy, PartialEq)]
struct Top {
    height: i64,
    material: Material,
}
#[derive(Clone, Copy, PartialEq)]
struct Span {
    lo: i64,
    hi: i64,
    material: Material,
    start: usize,
    end: usize,
}

/// Extract all exposed top/side surfaces owned by x/z=[min,min+size).
/// `min.y` is only the local vertex origin: it does not truncate the world.
/// Canonical neighbors in the halo cull internal faces at the finite crop edge;
/// there are no bottom faces or artificial crop caps. An exposed face whose solid
/// cell lies outside this rectangle is intentionally not owned by this mesh.
/// Limits bound scratch storage (~230 MiB) and mesh data (672 MB); excessive
/// relief or output returns an error rather than clipping or changing the source.
pub fn build(world: &World, min: Cell, size: [usize; 2]) -> Result<ColumnMesh, String> {
    if world.fbm_terrain().is_none() {
        return Err(
            "height-column extraction requires World::fbm; volume fixtures are unsupported".into(),
        );
    }
    if world.revision() != 0 {
        return Err(
            "height-column extraction rejects edited worlds; use native volume extraction".into(),
        );
    }
    if size.contains(&0) || size.iter().any(|&n| n > MAX_LOCAL_CELLS as usize) {
        return Err(
            "height-column rectangle must be nonempty and fit exact local f32 coordinates".into(),
        );
    }
    let halo = [size[0] + 2, size[1] + 2];
    let count = halo[0]
        .checked_mul(halo[1])
        .filter(|&n| n <= MAX_HALO_COLUMNS)
        .ok_or("height-column rectangle exceeds column memory budget")?;
    for (base, width) in [(min.x, size[0]), (min.z, size[1])] {
        base.checked_sub(1)
            .ok_or("column halo coordinate underflow")?;
        base.checked_add(width as i64)
            .ok_or("column halo coordinate overflow")?;
        // WorldColumn::height_m and canonical material centers use f64 world coordinates.
        if (base as i128).abs() + width as i128 + 1 >= (1_i128 << 53) {
            return Err("column coordinates exceed canonical f64 world addressing range".into());
        }
    }
    let mut columns = Vec::with_capacity(count);
    let mut values = Vec::with_capacity(count);
    let (mut low, mut high) = (i64::MAX, i64::MIN);
    for z in 0..halo[1] {
        for x in 0..halo[0] {
            let c = world.column(min.x + x as i64 - 1, min.z + z as i64 - 1);
            let h = (c.height_m() / VOXEL_SIZE) as i64;
            low = low.min(h);
            high = high.max(h);
            if (h as i128 - min.y as i128).abs() > MAX_LOCAL_CELLS as i128 {
                return Err(
                    "height-column vertical coordinate exceeds exact local f32 range".into(),
                );
            }
            values.push(h);
            columns.push(c);
        }
    }
    if high as i128 - low as i128 > MAX_RELIEF_CELLS as i128 {
        return Err("height-column relief exceeds bounded extraction budget".into());
    }
    let heights = ColumnHeights {
        min: [min.x, min.z],
        size,
        values,
    };
    let index = |x: usize, z: usize| z * halo[0] + x;
    let mut mesh = Mesh {
        vertices: vec![],
        indices: vec![],
        origin: min,
        exposed_native_faces: 0,
    };
    let mut tops = Vec::with_capacity(size[0] * size[1]);
    for z in 0..size[1] {
        for x in 0..size[0] {
            let i = index(x + 1, z + 1);
            let h = heights.values[i];
            tops.push(Some(Top {
                height: h,
                material: columns[i].material(h - 1),
            }));
        }
    }
    // Greedy equal-height/equal-material top rectangles.
    for z in 0..size[1] {
        let mut x = 0;
        while x < size[0] {
            let Some(top) = tops[z * size[0] + x] else {
                x += 1;
                continue;
            };
            let mut w = 1;
            while x + w < size[0] && tops[z * size[0] + x + w] == Some(top) {
                w += 1;
            }
            let mut d = 1;
            while z + d < size[1] && (x..x + w).all(|xx| tops[(z + d) * size[0] + xx] == Some(top))
            {
                d += 1;
            }
            for zz in z..z + d {
                tops[zz * size[0] + x..zz * size[0] + x + w].fill(None);
            }
            quad(
                &mut mesh,
                1,
                true,
                [x as i64, top.height - min.y, z as i64],
                [d as i64, w as i64],
                top.material,
            )?;
            x += w;
        }
    }
    drop(tops);
    // Each edge exposes at most three material spans, independent of its relief.
    // Equal spans in consecutive columns merge along the side's horizontal axis.
    for axis in [0, 2] {
        for positive in [false, true] {
            let (layers, tangent) = if axis == 0 {
                (size[0], size[1])
            } else {
                (size[1], size[0])
            };
            for layer in 0..layers {
                let mut active: [Option<Span>; 3] = [None; 3];
                for t in 0..=tangent {
                    let mut next: [Option<Span>; 3] = [None; 3];
                    let mut span_count = 0;
                    if t < tangent {
                        let (x, z) = if axis == 0 {
                            (layer + 1, t + 1)
                        } else {
                            (t + 1, layer + 1)
                        };
                        let i = index(x, z);
                        let neighbor = if axis == 0 {
                            index(if positive { x + 1 } else { x - 1 }, z)
                        } else {
                            index(x, if positive { z + 1 } else { z - 1 })
                        };
                        let h = heights.values[i];
                        let lo = heights.values[neighbor];
                        if lo < h {
                            let mut start = lo;
                            for end in [h - 12, h - 1, h] {
                                if end > start {
                                    next[span_count] = Some(Span {
                                        lo: start,
                                        hi: end,
                                        material: columns[i].material(start),
                                        start: t,
                                        end: t + 1,
                                    });
                                    span_count += 1;
                                    start = end;
                                }
                            }
                        }
                    }
                    for old in active.into_iter().flatten() {
                        if let Some(new) = next.iter_mut().flatten().find(|s| {
                            s.lo == old.lo && s.hi == old.hi && s.material == old.material
                        }) {
                            new.start = old.start;
                        } else {
                            let plane = (layer + usize::from(positive)) as i64;
                            let (p, ext) = if axis == 0 {
                                (
                                    [plane, old.lo - min.y, old.start as i64],
                                    [old.hi - old.lo, (old.end - old.start) as i64],
                                )
                            } else {
                                (
                                    [old.start as i64, old.lo - min.y, plane],
                                    [(old.end - old.start) as i64, old.hi - old.lo],
                                )
                            };
                            quad(&mut mesh, axis, positive, p, ext, old.material)?;
                        }
                    }
                    active = next;
                }
            }
        }
    }
    Ok(ColumnMesh { mesh, heights })
}

fn quad(
    mesh: &mut Mesh,
    axis: usize,
    positive: bool,
    p: [i64; 3],
    extent: [i64; 2],
    material: Material,
) -> Result<(), String> {
    if mesh.vertices.len() / 4 >= MAX_QUADS {
        return Err("height-column mesh exceeds output memory budget".into());
    }
    // Cap Vec growth explicitly so the output budget also bounds capacity.
    if mesh.vertices.capacity() < mesh.vertices.len() + 4 {
        let target = (mesh.vertices.capacity().max(256) * 2).min(MAX_QUADS * 4);
        mesh.vertices
            .try_reserve_exact(target - mesh.vertices.len())
            .map_err(|_| "height-column vertex allocation failed")?;
    }
    if mesh.indices.capacity() < mesh.indices.len() + 6 {
        let target = (mesh.indices.capacity().max(384) * 2).min(MAX_QUADS * 6);
        mesh.indices
            .try_reserve_exact(target - mesh.indices.len())
            .map_err(|_| "height-column index allocation failed")?;
    }
    let u = (axis + 1) % 3;
    let v = (axis + 2) % 3;
    let mut normal = [0.; 3];
    normal[axis] = if positive { 1. } else { -1. };
    let base = mesh.vertices.len() as u32;
    for (du, dv) in [
        (0, 0),
        (extent[0], 0),
        (extent[0], extent[1]),
        (0, extent[1]),
    ] {
        let mut position = p;
        position[u] += du;
        position[v] += dv;
        mesh.vertices.push(Vertex {
            position: position.map(|n| (n as f64 * VOXEL_SIZE) as f32),
            normal,
            color: material.color(),
        });
    }
    let corners = if positive {
        [0, 1, 2, 0, 2, 3]
    } else {
        [0, 2, 1, 0, 3, 2]
    };
    mesh.indices.extend(corners.map(|n| base + n));
    mesh.exposed_native_faces = mesh
        .exposed_native_faces
        .checked_add(
            (extent[0] as usize)
                .checked_mul(extent[1] as usize)
                .ok_or("face area overflow")?,
        )
        .ok_or("face count overflow")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::FbmParams;
    use std::collections::BTreeSet;

    // Expand geometric quads to canonical unit face identities, independently
    // of the extraction algorithm and its chosen greedy rectangle partition.
    fn faces(m: &Mesh) -> BTreeSet<([i64; 3], usize, bool, [u32; 3])> {
        let mut out = BTreeSet::new();
        for q in m.vertices.chunks_exact(4) {
            let axis = q[0].normal.iter().position(|&n| n != 0.).unwrap();
            let positive = q[0].normal[axis] > 0.;
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            let p: [[i64; 3]; 4] = std::array::from_fn(|i| {
                q[i].position
                    .map(|n| (n as f64 / VOXEL_SIZE).round() as i64)
            });
            let cross = (glam::Vec3::from(q[1].position) - glam::Vec3::from(q[0].position))
                .cross(glam::Vec3::from(q[2].position) - glam::Vec3::from(q[0].position));
            assert!(
                cross.dot(glam::Vec3::from(q[0].normal)) * if positive { 1. } else { -1. } > 0.
            );
            for a in p[0][u]..p[2][u] {
                for b in p[0][v]..p[2][v] {
                    let mut c = p[0];
                    c[u] = a;
                    c[v] = b;
                    c[axis] -= i64::from(positive);
                    c[0] += m.origin.x;
                    c[1] += m.origin.y;
                    c[2] += m.origin.z;
                    assert!(
                        out.insert((c, axis, positive, q[0].color.map(f32::to_bits))),
                        "duplicate face"
                    );
                }
            }
        }
        assert_eq!(out.len(), m.exposed_native_faces);
        out
    }

    #[test]
    fn exact_faces_materials_and_normals_match_dense_world_queries() {
        for min in [
            Cell {
                x: -43,
                y: -700,
                z: -57,
            },
            Cell {
                x: 1_600_000,
                y: 0,
                z: -1_600_000,
            },
        ] {
            for params in [
                FbmParams::default(),
                FbmParams {
                    horizontal_scale: 16.,
                    amplitude: 40.,
                    ..Default::default()
                },
                FbmParams {
                    horizontal_scale: 16.,
                    amplitude: 1000.,
                    ..Default::default()
                },
            ] {
                let world = World::fbm(7, params).unwrap();
                let out = build(&world, min, [11, 9]).unwrap();
                let low = *out.heights.values.iter().min().unwrap();
                let high = *out.heights.values.iter().max().unwrap();
                let dense_min = Cell {
                    x: min.x,
                    y: low - 1,
                    z: min.z,
                };
                let dense = crate::mesh::extract(
                    |c| world.material(c),
                    dense_min,
                    [11, (high - low + 2) as usize, 9],
                );
                assert_eq!(faces(&out.mesh), faces(&dense));
                if params.horizontal_scale == 16. && params.amplitude == 1000. {
                    for material in [Material::Grass, Material::Soil, Material::Stone] {
                        assert!(
                            out.mesh
                                .vertices
                                .iter()
                                .any(|v| v.color == material.color()),
                            "missing exposed {material:?}"
                        );
                    }
                }
                assert!(out.mesh.vertices.iter().all(|v| v.normal != [0., -1., 0.]));
                assert_eq!(out.heights.sampled_columns(), 13 * 11);
                for z in min.z - 1..=min.z + 9 {
                    for x in min.x - 1..=min.x + 11 {
                        assert_eq!(
                            out.heights.height(x, z),
                            Some((world.column(x, z).height_m() / VOXEL_SIZE) as i64)
                        );
                    }
                }
                assert!(out.heights.height(min.x - 2, min.z).is_none());
            }
        }
    }

    #[test]
    fn flat_crop_has_one_top_quad_without_bottom_or_crop_caps() {
        let world = World::fbm(
            11,
            FbmParams {
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let out = build(
            &world,
            Cell {
                x: -123,
                y: -17,
                z: 29,
            },
            [63, 41],
        )
        .unwrap();
        assert_eq!(out.mesh.vertices.len(), 4);
        assert_eq!(out.mesh.indices.len(), 6);
        assert_eq!(out.mesh.exposed_native_faces, 63 * 41);
        assert_eq!(out.heights.sampled_columns(), 65 * 43);
        assert_eq!(out.mesh.vertices[0].color, Material::Grass.color());
    }

    fn ray(m: &Mesh, o: glam::DVec3, d: glam::DVec3) -> Option<(f64, [f32; 3], [f32; 3])> {
        let shift =
            glam::DVec3::new(m.origin.x as f64, m.origin.y as f64, m.origin.z as f64) * VOXEL_SIZE;
        let mut best = None;
        for t in m.indices.chunks_exact(3) {
            let a = &m.vertices[t[0] as usize];
            let p = glam::DVec3::from(glam::Vec3::from(a.position)) + shift;
            let b = glam::DVec3::from(glam::Vec3::from(m.vertices[t[1] as usize].position)) + shift;
            let c = glam::DVec3::from(glam::Vec3::from(m.vertices[t[2] as usize].position)) + shift;
            let e = b - p;
            let f = c - p;
            let h = d.cross(f);
            let det = e.dot(h);
            if det.abs() < 1e-12 {
                continue;
            }
            let s = o - p;
            let u = s.dot(h) / det;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = s.cross(e);
            let v = d.dot(q) / det;
            if v < 0. || u + v > 1. {
                continue;
            }
            let distance = f.dot(q) / det;
            if distance >= 0. && best.is_none_or(|(old, _, _)| distance < old) {
                best = Some((distance, a.color, a.normal));
            }
        }
        best
    }

    #[test]
    fn interior_rays_match_canonical_dda_depth_normal_and_material() {
        let world = World::fbm(
            29,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 12.,
                ..Default::default()
            },
        )
        .unwrap();
        let out = build(
            &world,
            Cell {
                x: -24,
                y: 0,
                z: -21,
            },
            [47, 43],
        )
        .unwrap();
        let high = *out.heights.values.iter().max().unwrap() as f64 * VOXEL_SIZE;
        for z in -16..15 {
            for x in -19..18 {
                let o = glam::DVec3::new(
                    (x as f64 + 0.27) * VOXEL_SIZE,
                    high + 2.,
                    (z as f64 + 0.39) * VOXEL_SIZE,
                );
                for direction in [[0., -1., 0.], [0.013, -1., -0.009]] {
                    let d = glam::DVec3::from(direction).normalize();
                    let expected =
                        frontier_world::raycast(&world, o.to_array(), d.to_array(), 100.).unwrap();
                    let got = ray(&out.mesh, o, d).unwrap();
                    assert!((got.0 - expected.distance).abs() < 1e-7);
                    assert_eq!(got.1, expected.material.color());
                    assert_eq!(got.2, expected.normal.map(|n| n as f32));
                }
            }
        }
    }

    #[test]
    fn rejects_unsupported_sources_edits_overflow_and_memory_requests() {
        assert!(build(&World::new(0), Cell::default(), [1, 1]).is_err());
        let mut world = World::fbm(0, FbmParams::default()).unwrap();
        for size in [[0, 1], [usize::MAX, 1], [4096, 4096]] {
            assert!(build(&world, Cell::default(), size).is_err());
        }
        assert!(build(
            &world,
            Cell {
                x: i64::MAX,
                ..Default::default()
            },
            [1, 1]
        )
        .is_err());
        assert!(build(
            &world,
            Cell {
                y: i64::MIN,
                ..Default::default()
            },
            [1, 1]
        )
        .is_err());
        world.set(
            Cell {
                x: 0,
                y: 100_000,
                z: 0,
            },
            Material::Wood,
        );
        assert!(build(&world, Cell::default(), [1, 1])
            .err()
            .unwrap()
            .contains("edited"));
        let extreme = World::fbm(
            7,
            FbmParams {
                horizontal_scale: 16.,
                amplitude: 100_000.,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(build(
            &extreme,
            Cell {
                x: -128,
                y: 0,
                z: -128
            },
            [256, 256]
        )
        .err()
        .unwrap()
        .contains("relief"));
    }

    /// Reproducible CPU-only startup benchmark, intentionally excluded from normal tests.
    #[test]
    #[ignore = "release 64m column extraction measurement"]
    fn measure_64m_native_crop() {
        let world = World::fbm(7, FbmParams::default()).unwrap();
        let start = std::time::Instant::now();
        let out = build(
            &world,
            Cell {
                x: -512,
                y: 0,
                z: -512,
            },
            [1024, 1024],
        )
        .unwrap();
        eprintln!("64m exact native columns seed=7: {:.3}s; {} sampled columns; {} exposed faces; {} quads; {} vertex/index bytes; {} height cache bytes",start.elapsed().as_secs_f64(),out.heights.sampled_columns(),out.mesh.exposed_native_faces,out.mesh.indices.len()/6,out.mesh.vertices.len()*std::mem::size_of::<Vertex>()+out.mesh.indices.len()*4,out.heights.values.len()*8);
        assert_eq!(out.heights.sampled_columns(), 1026 * 1026);
    }
}
