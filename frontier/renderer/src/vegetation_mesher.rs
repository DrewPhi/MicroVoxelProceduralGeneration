//! Exact vegetation surface extraction using contiguous aggregation and faces.
//!
//! Greenfield storage optimization of the contract in `vegetation.rs` (same
//! Frontier workspace); no external mesher or legacy runtime is used.
//! Input-order sums, material-order reduction, face order, and greedy rectangle
//! order deliberately match the reference, including its floating-point math.
use crate::vegetation::{VegetationMesh, VegetationVertex};
use frontier_world::{Cell, Material, VOXEL_SIZE};
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
};

// Coordinates are trusted, locally generated keys. A small integer hasher
// avoids SipHash's string/adversarial-input overhead in stencil lookups.
// Integer avalanche: SplitMix64 finalizer by Sebastiano Vigna (2015), public
// domain, https://prng.di.unimi.it/splitmix.c. Only the scalar finalizer is
// adapted; coordinate combination and all meshing/storage code are local.
#[derive(Default)]
struct CoordinateHasher(u64);
impl Hasher for CoordinateHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write_u64(&mut self, mut value: u64) {
        value ^= value >> 30;
        value = value.wrapping_mul(0xbf58476d1ce4e5b9);
        value ^= value >> 27;
        value = value.wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        self.0 = self.0.rotate_left(21).wrapping_add(value);
    }
    fn write_i64(&mut self, value: i64) {
        self.write_u64(value as u64);
    }
    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_ne_bytes(word));
        }
    }
}
type Map<K, V> = HashMap<K, V, BuildHasherDefault<CoordinateHasher>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Paint {
    material: u8,
    color: [u32; 3],
    ao: u8,
}
struct Palette {
    material: u8,
    count: usize,
    sum: [f64; 3],
}
struct Group {
    cell: [i64; 3],
    // Most cells have one material. Keep that accumulator inline and allocate
    // only when distinct materials actually meet in an aggregate cell.
    first: Palette,
    others: Vec<Palette>,
}
impl Palette {
    fn new(material: u8) -> Self {
        Self {
            material,
            count: 0,
            sum: [0.; 3],
        }
    }
    fn add(&mut self, color: [f32; 3]) {
        self.count += 1;
        for j in 0..3 {
            self.sum[j] += color[j] as f64;
        }
    }
}
impl Group {
    fn paint(mut self) -> Paint {
        if self.others.is_empty() {
            return Paint {
                material: self.first.material,
                color: self
                    .first
                    .sum
                    .map(|v| ((v / self.first.count as f64) as f32).to_bits()),
                ao: 255,
            };
        }
        self.others.push(self.first);
        self.others.sort_unstable_by_key(|p| p.material);
        let mut count = 0;
        let mut sum = [0.; 3];
        let mut majority = (0, 0);
        for p in self.others {
            if p.count > majority.0 {
                majority = (p.count, p.material);
            }
            count += p.count;
            for j in 0..3 {
                sum[j] += p.sum[j];
            }
        }
        Paint {
            material: majority.1,
            color: sum.map(|v| ((v / count as f64) as f32).to_bits()),
            ao: 255,
        }
    }
}

// Dense lookup is bounded independently of world coordinates. Sparse scenes
// use 8^3 occupancy bricks, so a pair of distant plants cannot allocate the gap.
enum Occupancy {
    Dense {
        min: [i64; 3],
        size: [usize; 3],
        bits: Vec<u8>,
    },
    Bricks(Map<[i64; 3], [u64; 8]>),
}
impl Occupancy {
    fn new(groups: &[Group], cancel: &impl Fn() -> bool) -> Option<Self> {
        let mut min = groups[0].cell;
        let mut max = min;
        for (i, g) in groups.iter().enumerate() {
            if i % 1024 == 0 && cancel() {
                return None;
            }
            for d in 0..3 {
                min[d] = min[d].min(g.cell[d]);
                max[d] = max[d].max(g.cell[d]);
            }
        }
        let size = (0..3)
            .map(|d| max[d].checked_sub(min[d])?.checked_add(1)?.try_into().ok())
            .collect::<Option<Vec<usize>>>();
        if let Some(size) = size {
            if let Some(volume) = size[0]
                .checked_mul(size[1])
                .and_then(|v| v.checked_mul(size[2]))
            {
                if volume <= 16 * 1024 * 1024 && volume <= groups.len().saturating_mul(32) {
                    let size = [size[0], size[1], size[2]];
                    let mut bits = vec![0; volume];
                    for (i, g) in groups.iter().enumerate() {
                        if i % 1024 == 0 && cancel() {
                            return None;
                        }
                        let p = std::array::from_fn::<_, 3, _>(|d| (g.cell[d] - min[d]) as usize);
                        bits[(p[0] * size[1] + p[1]) * size[2] + p[2]] = 1;
                    }
                    return Some(Self::Dense { min, size, bits });
                }
            }
        }
        let mut bricks: Map<[i64; 3], [u64; 8]> = Map::default();
        for (i, g) in groups.iter().enumerate() {
            if i % 1024 == 0 && cancel() {
                return None;
            }
            let c = g.cell;
            let key = c.map(|v| v.div_euclid(8));
            let bit =
                ((c[0].rem_euclid(8) * 8 + c[1].rem_euclid(8)) * 8 + c[2].rem_euclid(8)) as usize;
            bricks.entry(key).or_insert([0; 8])[bit / 64] |= 1 << (bit % 64);
        }
        Some(Self::Bricks(bricks))
    }
    #[inline]
    fn contains(&self, c: [i64; 3]) -> bool {
        match self {
            Self::Dense { min, size, bits } => {
                let mut p = [0; 3];
                for d in 0..3 {
                    let Some(offset) = c[d].checked_sub(min[d]) else {
                        return false;
                    };
                    if offset < 0 || offset as u64 >= size[d] as u64 {
                        return false;
                    }
                    p[d] = offset as usize;
                }
                bits[(p[0] * size[1] + p[1]) * size[2] + p[2]] != 0
            }
            Self::Bricks(bricks) => {
                let key = c.map(|v| v.div_euclid(8));
                let bit = ((c[0].rem_euclid(8) * 8 + c[1].rem_euclid(8)) * 8 + c[2].rem_euclid(8))
                    as usize;
                bricks
                    .get(&key)
                    .is_some_and(|b| b[bit / 64] & (1 << (bit % 64)) != 0)
            }
        }
    }
}
fn visibility(blocks: &Occupancy, c: [i64; 3], d: usize, sign: i64, step: i64) -> u8 {
    let mut exterior = c;
    exterior[d] += sign;
    let u = (d + 1) % 3;
    let v = (d + 2) % 3;
    let mut contact = 0.;
    for (a, b) in [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (-1, -1),
        (1, -1),
        (-1, 1),
    ] {
        let mut q = exterior;
        q[u] += a;
        q[v] += b;
        if blocks.contains(q) {
            contact += 1.;
        }
    }
    let mut canopy = 0.;
    if d != 1 || sign > 0 {
        for r in [2, 4, 8, 16] {
            let mut q = c;
            q[d] += sign * (r / 2).max(1);
            q[1] += r;
            if blocks.contains(q) {
                canopy += 1.;
            }
        }
    }
    let strength = 1. / (step as f32).sqrt();
    let visibility = 1. - strength * (0.22 * contact / 8. + 0.12 * canopy / 4.);
    ((visibility * 15.).round().clamp(0., 15.) as u8) * 17
}
#[derive(Clone, Copy)]
struct Face {
    plane: (usize, i64, i64),
    start: [i64; 2],
    paint: Paint,
}

pub(crate) fn mesh_cells_scaled(
    cells: impl Iterator<Item = (Cell, u8, [f32; 3])>,
    stride: u8,
    vertical_stride: u8,
    cancel: impl Fn() -> bool,
) -> VegetationMesh {
    let step = i64::from(stride.max(1));
    let steps = [step, i64::from(vertical_stride.max(1)), step];
    let mut lookup = Map::default();
    let mut groups: Vec<Group> = Vec::new();
    for (i, (c, material, color)) in cells.enumerate() {
        if i % 1024 == 0 && cancel() {
            return VegetationMesh::default();
        }
        let cell = [
            c.x.div_euclid(step),
            c.y.div_euclid(steps[1]),
            c.z.div_euclid(step),
        ];
        let index = *lookup.entry(cell).or_insert_with(|| {
            groups.push(Group {
                cell,
                first: Palette::new(material),
                others: Vec::new(),
            });
            groups.len() - 1
        });
        let g = &mut groups[index];
        let p = if g.first.material == material {
            &mut g.first
        } else {
            let index = g
                .others
                .iter()
                .position(|p| p.material == material)
                .unwrap_or_else(|| {
                    g.others.push(Palette::new(material));
                    g.others.len() - 1
                });
            &mut g.others[index]
        };
        p.add(color);
    }
    drop(lookup);
    if groups.is_empty() || cancel() {
        return VegetationMesh::default();
    }
    let Some(blocks) = Occupancy::new(&groups, &cancel) else {
        return VegetationMesh::default();
    };
    let mut faces = Vec::new();
    for (i, group) in groups.into_iter().enumerate() {
        if i % 1024 == 0 && cancel() {
            return VegetationMesh::default();
        }
        let c = group.cell;
        let paint = group.paint();
        for d in 0..3 {
            for sign in [-1, 1] {
                let mut neighbor = c;
                neighbor[d] += sign;
                if !blocks.contains(neighbor) {
                    let mut paint = paint;
                    paint.ao = visibility(&blocks, c, d, sign, step);
                    faces.push(Face {
                        plane: (d, sign, c[d] + i64::from(sign > 0)),
                        start: [c[(d + 1) % 3], c[(d + 2) % 3]],
                        paint,
                    });
                }
            }
        }
    }
    if cancel() {
        return VegetationMesh::default();
    }
    faces.sort_unstable_by_key(|f| (f.plane, f.start));
    let mut mesh = VegetationMesh::default();
    let mut first = 0;
    let mut plane_lookup = Map::default();
    let mut consumed = Vec::new();
    while first < faces.len() {
        if cancel() {
            return VegetationMesh::default();
        }
        let plane_key = faces[first].plane;
        let end = first + faces[first..].partition_point(|f| f.plane == plane_key);
        let plane_faces = &faces[first..end];
        plane_lookup.clear();
        consumed.clear();
        consumed.resize(plane_faces.len(), false);
        for (i, f) in plane_faces.iter().enumerate() {
            plane_lookup.insert(f.start, i);
        }
        for (i, face) in plane_faces.iter().enumerate() {
            if i % 1024 == 0 && cancel() {
                return VegetationMesh::default();
            }
            if consumed[i] {
                continue;
            }
            let start = face.start;
            let paint = face.paint;
            let matches = |p: [i64; 2], consumed: &[bool]| {
                plane_lookup
                    .get(&p)
                    .is_some_and(|&i| !consumed[i] && plane_faces[i].paint == paint)
            };
            let mut w = 1;
            while matches([start[0] + w, start[1]], &consumed) {
                w += 1;
            }
            let mut h = 1;
            'height: loop {
                for x in 0..w {
                    if !matches([start[0] + x, start[1] + h], &consumed) {
                        break 'height;
                    }
                }
                h += 1;
            }
            for y in 0..h {
                for x in 0..w {
                    consumed[plane_lookup[&[start[0] + x, start[1] + y]]] = true;
                }
            }
            let (d, sign, plane) = plane_key;
            let base = mesh.vertices.len() as u32;
            let u = (d + 1) % 3;
            let v = (d + 2) % 3;
            let mut normal = [0.; 3];
            normal[d] = sign as f32;
            for [a, b] in [[0, 0], [w, 0], [w, h], [0, h]] {
                let mut p = [0.; 3];
                p[d] = (plane * steps[d]) as f32 * VOXEL_SIZE as f32;
                p[u] = ((start[0] + a) * steps[u]) as f32 * VOXEL_SIZE as f32;
                p[v] = ((start[1] + b) * steps[v]) as f32 * VOXEL_SIZE as f32;
                mesh.vertices.push(VegetationVertex {
                    position: p,
                    normal,
                    color: paint.color.map(f32::from_bits),
                    ao: paint.ao as f32 / 255.,
                    transmission: if paint.material == Material::Leaf as u8 {
                        1.
                    } else if paint.material == Material::Flower as u8 {
                        0.35
                    } else {
                        0.
                    },
                });
            }
            let indices = if sign > 0 {
                [0, 1, 2, 0, 2, 3]
            } else {
                [0, 2, 1, 0, 3, 2]
            };
            mesh.indices.extend(indices.map(|i| base + i));
        }
        first = end;
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vegetation::mesh_cells_scaled_reference;
    use frontier_world::vegetation::{generate_edelweiss, generate_grass, generate_rowan};

    // Check every floating-point bit (including NaN/signed zero), vertex order,
    // rectangle decomposition, material transmission, and triangle winding.
    fn assert_exact(cells: &[(Cell, u8, [f32; 3])], stride: u8, vertical: u8) {
        let a = mesh_cells_scaled(cells.iter().copied(), stride, vertical, || false);
        let b = mesh_cells_scaled_reference(cells.iter().copied(), stride, vertical, || false);
        assert_eq!(a.indices, b.indices, "indices: stride={stride}/{vertical}");
        assert_eq!(a.vertices.len(), b.vertices.len());
        for (a, b) in a.vertices.iter().zip(&b.vertices) {
            assert_eq!(a.position.map(f32::to_bits), b.position.map(f32::to_bits));
            assert_eq!(a.normal.map(f32::to_bits), b.normal.map(f32::to_bits));
            assert_eq!(a.color.map(f32::to_bits), b.color.map(f32::to_bits));
            assert_eq!(a.ao.to_bits(), b.ao.to_bits());
            assert_eq!(a.transmission.to_bits(), b.transmission.to_bits());
        }
    }
    #[test]
    fn generated_species_match_reference_exactly() {
        for model in [
            generate_grass(17, 2),
            generate_edelweiss(97, 1),
            generate_rowan(123, 0),
        ] {
            let cells: Vec<_> = model
                .voxels
                .iter()
                .map(|v| (v.cell, v.material as u8, v.color))
                .collect();
            for stride in [1, 2, 4, 8] {
                assert_exact(&cells, stride, stride);
                if stride >= 4 {
                    assert_exact(&cells, stride, 2);
                }
            }
        }
    }
    #[test]
    fn sparse_negative_coordinates_palettes_duplicates_and_holes_match() {
        let mut cells = Vec::new();
        for x in -7i64..8 {
            for y in -4i64..5 {
                for z in -6i64..7 {
                    let h = (x.wrapping_mul(73856093)
                        ^ y.wrapping_mul(19349663)
                        ^ z.wrapping_mul(83492791)) as u64;
                    if h % 5 < 2 {
                        continue;
                    }
                    let color = [(h % 17) as f32 / 17., (h % 29) as f32 / 29., -0.];
                    cells.push((Cell { x, y, z }, (h % 5) as u8, color));
                    if h % 7 == 0 {
                        cells.push((Cell { x, y, z }, ((h + 1) % 5) as u8, [0.7, 0.2, 0.3]));
                    }
                }
            }
        }
        cells.reverse();
        // Force sparse-brick fallback with no allocation proportional to gap.
        cells.push((
            Cell {
                x: 1_000_000_000_000,
                y: -1_000_000_000_000,
                z: 1_000_000_000_000,
            },
            2,
            [0.2; 3],
        ));
        for stride in [0, 1, 2, 4, 8] {
            assert_exact(&cells, stride, stride.min(2));
        }
        assert_exact(&[], 1, 1);
    }
    #[test]
    fn cancellation_discards_partial_work_at_each_phase() {
        use std::cell::Cell as Counter;
        let cells: Vec<_> = (0..4096)
            .map(|i| {
                (
                    Cell {
                        x: i % 16,
                        y: (i / 16) % 16,
                        z: i / 256,
                    },
                    2,
                    [0.2; 3],
                )
            })
            .collect();
        let polls = Counter::new(0);
        mesh_cells_scaled(cells.iter().copied(), 1, 1, || {
            polls.set(polls.get() + 1);
            false
        });
        let total = polls.get();
        assert!(total > 10);
        for at in 1..=total {
            polls.set(0);
            let mesh = mesh_cells_scaled(cells.iter().copied(), 1, 1, || {
                polls.set(polls.get() + 1);
                polls.get() >= at
            });
            assert!(
                mesh.vertices.is_empty() && mesh.indices.is_empty(),
                "cancel poll {at}/{total}"
            );
        }
    }
    #[test]
    #[ignore = "CPU-only release microbenchmark; run with --release --ignored --nocapture"]
    fn vegetation_meshing_microbenchmark() {
        use std::{hint::black_box, time::Instant};
        let grass = generate_grass(17, 2);
        let tile: Vec<_> = (0..256)
            .flat_map(|i| {
                grass.voxels.iter().map(move |v| {
                    (
                        Cell {
                            x: v.cell.x + (i % 16) * 8,
                            y: v.cell.y + (i % 3),
                            z: v.cell.z + (i / 16) * 8,
                        },
                        v.material as u8,
                        v.color,
                    )
                })
            })
            .collect();
        let mut cases = Vec::new();
        for (name, model) in [
            ("grass", grass),
            ("edelweiss", generate_edelweiss(97, 1)),
            ("rowan", generate_rowan(123, 0)),
        ] {
            cases.push((
                name,
                model
                    .voxels
                    .iter()
                    .map(|v| (v.cell, v.material as u8, v.color))
                    .collect::<Vec<_>>(),
            ));
        }
        cases.push(("grass_tile", tile));
        for (name, cells) in cases {
            for stride in [1, 2, 4, 8] {
                assert_exact(&cells, stride, stride);
                let mesh = mesh_cells_scaled(cells.iter().copied(), stride, stride, || false);
                let quads = mesh.indices.len() / 6;
                // Alternating order and median samples reduce drift; generation
                // and oracle verification are excluded from timed sections.
                let mut old = Vec::new();
                let mut new = Vec::new();
                for i in 0..9 {
                    let mut run_old = || {
                        let t = Instant::now();
                        black_box(mesh_cells_scaled_reference(
                            cells.iter().copied(),
                            stride,
                            stride,
                            || false,
                        ));
                        old.push(t.elapsed());
                    };
                    let mut run_new = || {
                        let t = Instant::now();
                        black_box(mesh_cells_scaled(
                            cells.iter().copied(),
                            stride,
                            stride,
                            || false,
                        ));
                        new.push(t.elapsed());
                    };
                    if i % 2 == 0 {
                        run_old();
                        run_new();
                    } else {
                        run_new();
                        run_old();
                    }
                }
                old.sort();
                new.sort();
                eprintln!(
                    "{name} cells={} quads={quads} stride={stride} reference_us={} optimized_us={} speedup={:.2}",
                    cells.len(),
                    old[4].as_micros(),
                    new[4].as_micros(),
                    old[4].as_secs_f64() / new[4].as_secs_f64()
                );
            }
        }
    }
}
