//! CPU-only comparison of equal native edit pages and an incremental splat page.
//! Run with cargo test -p frontier-renderer --release local_native_edit_cost
//! -- --ignored --nocapture. GPU allocation and uploads are not timed.
use super::*;
use crate::mesh;
use frontier_world::{World, VOXEL_SIZE};
use std::{
    collections::{BTreeMap, BTreeSet},
    hint::black_box,
    time::Instant,
};

type Splat = [u32; 4];
type Face = (Cell, usize, u32);
const MISSING_SLOT: u32 = u32::MAX;
const SEED: u64 = 7;
const SAMPLES: usize = 100;
const WARMUP: usize = 4;

fn offset(c: Cell, d: [i64; 3]) -> Cell {
    Cell {
        x: c.x + d[0],
        y: c.y + d[1],
        z: c.z + d[2],
    }
}

fn neighbor(c: Cell, face: usize) -> Cell {
    let mut d = [0; 3];
    d[face / 2] = if face % 2 == 0 { -1 } else { 1 };
    offset(c, d)
}

fn page_min(c: Cell, side: i64) -> Cell {
    let align = |v: i64| v.div_euclid(side) * side;
    Cell {
        x: align(c.x),
        y: align(c.y),
        z: align(c.z),
    }
}

fn affected_cells(edit: &[(Cell, Material)]) -> BTreeSet<Cell> {
    edit.iter()
        .flat_map(|&(c, _)| {
            [
                c,
                neighbor(c, 0),
                neighbor(c, 1),
                neighbor(c, 2),
                neighbor(c, 3),
                neighbor(c, 4),
                neighbor(c, 5),
            ]
        })
        .collect()
}

fn local(c: Cell, min: Cell) -> [u32; 3] {
    [
        (c.x - min.x) as u32,
        (c.y - min.y) as u32,
        (c.z - min.z) as u32,
    ]
}

fn splat_word(material: Material, is_air: impl Fn(usize) -> bool) -> u32 {
    if material == Material::Air {
        return 0;
    }
    let mut word = u32::from(material_id(material));
    for face in 0..6 {
        if is_air(face) {
            word |= 1 << (8 + face);
        }
    }
    word
}

fn rebuild_splats(world: &World, min: Cell, side: usize) -> Vec<Splat> {
    let halo = side + 2;
    let plane = halo * halo;
    let mut occupancy = Vec::with_capacity(halo * plane);
    for z in -1..=side as i64 {
        for y in -1..=side as i64 {
            for x in -1..=side as i64 {
                occupancy.push(world.material(offset(min, [x, y, z])));
            }
        }
    }
    let strides = [1, halo, plane];
    let mut splats = Vec::new();
    for z in 0..side {
        for y in 0..side {
            for x in 0..side {
                let i = ((z + 1) * halo + y + 1) * halo + x + 1;
                let word = splat_word(occupancy[i], |face| {
                    let step = strides[face / 2];
                    let j = if face % 2 == 0 { i - step } else { i + step };
                    occupancy[j] == Material::Air
                });
                if word & EXPOSED_FACE_MASK != 0 {
                    splats.push([x as u32, y as u32, z as u32, word]);
                }
            }
        }
    }
    splats
}

/// Simulate CPU ownership of a packed splat list. Only final changed storage
/// slots and draw counts would need to be sent to an existing GPU allocation.
struct SplatPage {
    splats: Vec<Splat>,
    slot_at: Vec<u32>,
    side: usize,
}
impl SplatPage {
    fn new(splats: Vec<Splat>, side: usize) -> Self {
        let mut page = Self {
            splats,
            slot_at: vec![MISSING_SLOT; side.pow(3)],
            side,
        };
        for (slot, &[x, y, z, _]) in page.splats.iter().enumerate() {
            let i = page.index([x, y, z]);
            page.slot_at[i] = slot as u32;
        }
        page
    }

    fn index(&self, [x, y, z]: [u32; 3]) -> usize {
        (z as usize * self.side + y as usize) * self.side + x as usize
    }

    fn apply(&mut self, [x, y, z, word]: Splat) -> Option<usize> {
        let i = self.index([x, y, z]);
        let slot = self.slot_at[i];
        let has_surface = word & EXPOSED_FACE_MASK != 0;
        match (slot, has_surface) {
            (MISSING_SLOT, false) => None,
            (MISSING_SLOT, true) => {
                let new_slot = self.splats.len();
                self.splats.push([x, y, z, word]);
                self.slot_at[i] = new_slot as u32;
                Some(new_slot)
            }
            (slot, true) => {
                let slot = slot as usize;
                if self.splats[slot][3] == word {
                    return None;
                }
                self.splats[slot][3] = word;
                Some(slot)
            }
            (slot, false) => {
                self.slot_at[i] = MISSING_SLOT;
                let slot = slot as usize;
                self.splats.swap_remove(slot);
                if let Some(&[mx, my, mz, _]) = self.splats.get(slot) {
                    let moved = self.index([mx, my, mz]);
                    self.slot_at[moved] = slot as u32;
                    Some(slot)
                } else {
                    None
                }
            }
        }
    }
}

fn splat_faces(min: Cell, splats: &[Splat]) -> BTreeSet<Face> {
    let mut faces = BTreeSet::new();
    for &[x, y, z, word] in splats {
        let cell = offset(min, [x as i64, y as i64, z as i64]);
        for face in 0..6 {
            if word & (1 << (8 + face)) != 0 {
                assert!(faces.insert((cell, face, word & 255)));
            }
        }
    }
    faces
}

fn mesh_faces(world: &World, mesh: &mesh::Mesh) -> BTreeSet<Face> {
    let mut faces = BTreeSet::new();
    for vertices in mesh.vertices.chunks_exact(4) {
        let normal = vertices[0].normal;
        let axis = normal.iter().position(|&v| v != 0.).unwrap();
        let positive = normal[axis] > 0.;
        let face = axis * 2 + usize::from(positive);
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let lattice =
            |corner: usize, a: usize| (vertices[corner].position[a] as f64 / VOXEL_SIZE) as i64;
        let mut local = [0; 3];
        local[axis] = lattice(0, axis) - i64::from(positive);
        for i in lattice(0, u)..lattice(2, u) {
            for j in lattice(0, v)..lattice(2, v) {
                local[u] = i;
                local[v] = j;
                let cell = offset(mesh.origin, local);
                let material = world.material(cell);
                assert_ne!(material, Material::Air);
                assert_eq!(material.color(), vertices[0].color);
                assert_eq!(world.material(neighbor(cell, face)), Material::Air);
                assert!(faces.insert((cell, face, u32::from(material_id(material)))));
            }
        }
    }
    assert_eq!(faces.len(), mesh.exposed_native_faces);
    faces
}

#[derive(Default)]
struct Samples {
    times_us: Vec<f64>,
    payload_bytes: Vec<usize>,
}
impl Samples {
    fn record(&mut self, sample: usize, started: Instant, bytes: usize) {
        let elapsed = started.elapsed().as_secs_f64() * 1e6;
        if sample >= WARMUP {
            self.times_us.push(elapsed);
            self.payload_bytes.push(bytes);
        }
    }

    fn json(mut self) -> String {
        let len = self.times_us.len();
        self.times_us.sort_unstable_by(f64::total_cmp);
        self.payload_bytes.sort_unstable();
        let q = |v: f64| ((len as f64 * v).ceil() as usize) - 1;
        format!(
            "{{\"p50_us\":{:.4},\"p95_us\":{:.4},\"p99_us\":{:.4},\"payload_p50_bytes\":{},\"payload_max_bytes\":{}}}",
            self.times_us[q(0.5)], self.times_us[q(0.95)], self.times_us[q(0.99)],
            self.payload_bytes[q(0.5)], self.payload_bytes[len - 1],
        )
    }
}

fn benchmark(name: &str, edit: &[(Cell, Material)], side: usize) {
    let mut world = World::new(SEED);
    let original: Vec<_> = edit.iter().map(|&(c, _)| (c, world.material(c))).collect();
    let cells = affected_cells(edit);
    let mut owners: BTreeMap<Cell, Vec<Cell>> = BTreeMap::new();
    for &cell in &cells {
        owners
            .entry(page_min(cell, side as i64))
            .or_default()
            .push(cell);
    }
    let mut retained: Vec<_> = owners
        .keys()
        .map(|&min| SplatPage::new(rebuild_splats(&world, min, side), side))
        .collect();
    let size = [side; 3];
    let mut mesh_stats = Samples::default();
    let mut splat_stats = Samples::default();
    let mut delta_stats = Samples::default();
    for sample in 0..WARMUP + SAMPLES {
        let change = if sample % 2 == 0 { edit } else { &original };
        world.set_batch(change.iter().copied());

        // Rotate operation order across samples so no method always benefits
        // from the other backend having just sampled the same source cells.
        let mut meshes = Vec::new();
        let mut rebuilt = Vec::new();
        for phase in 0..3 {
            match (phase + sample) % 3 {
                0 => {
                    let started = Instant::now();
                    meshes = owners
                        .keys()
                        .map(|&min| mesh::extract(|p| world.material(p), min, size))
                        .collect();
                    let bytes = meshes
                        .iter()
                        .map(|mesh| {
                            std::mem::size_of_val(mesh.vertices.as_slice())
                                + std::mem::size_of_val(mesh.indices.as_slice())
                        })
                        .sum();
                    mesh_stats.record(sample, started, black_box(bytes));
                }
                1 => {
                    let started = Instant::now();
                    rebuilt = owners
                        .keys()
                        .map(|&min| rebuild_splats(&world, min, side))
                        .collect();
                    let bytes = rebuilt
                        .iter()
                        .map(|v| std::mem::size_of_val(v.as_slice()))
                        .sum();
                    splat_stats.record(sample, started, black_box(bytes));
                }
                _ => {
                    let started = Instant::now();
                    let mut bytes = 0;
                    for ((&min, coords), page) in owners.iter().zip(&mut retained) {
                        let old_count = page.splats.len();
                        let mut writes = Vec::new();
                        for &c in coords {
                            let word = splat_word(world.material(c), |f| {
                                world.material(neighbor(c, f)) == Material::Air
                            });
                            let [x, y, z] = local(c, min);
                            writes.extend(page.apply([x, y, z, word]));
                        }
                        writes.sort_unstable();
                        writes.dedup();
                        bytes += writes.iter().filter(|&&i| i < page.splats.len()).count()
                            * std::mem::size_of::<Splat>()
                            + usize::from(old_count != page.splats.len())
                                * std::mem::size_of::<u32>();
                    }
                    delta_stats.record(sample, started, black_box(bytes));
                }
            }
        }
        // Prove both edit directions, including a warm and final run. Greedy
        // rectangles are expanded back into exact oriented material faces.
        if sample < WARMUP || sample >= WARMUP + SAMPLES - 2 {
            for ((&min, mesh), (page, full)) in owners
                .keys()
                .zip(&meshes)
                .zip(retained.iter().zip(&rebuilt))
            {
                let faces = mesh_faces(&world, mesh);
                assert_eq!(faces, splat_faces(min, full));
                assert_eq!(faces, splat_faces(min, &page.splats));
                if sample < 2 {
                    let brick = BrickSnapshot::build(|p| world.material(p), min, size);
                    assert_eq!(faces, splat_faces(min, &brick.collect_surface_cells()));
                }
                for (slot, &[x, y, z, _]) in page.splats.iter().enumerate() {
                    assert_eq!(page.slot_at[page.index([x, y, z])], slot as u32);
                }
            }
        }
    }
    println!(
        "{{\"workload\":{name:?},\"seed\":{SEED},\"page_side_cells\":{side},\"edit_cells\":{},\"faces_may_change_on_cells\":{},\"dirty_pages\":{},\"samples\":{SAMPLES},\"warmup\":{WARMUP},\"greedy_mesh\":{},\"page_splats\":{},\"delta_splats\":{},\"delta_cpu_slot_index_bytes\":{},\"restored_splat_payload_bytes\":{}}}",
        edit.len(), cells.len(), owners.len(), mesh_stats.json(), splat_stats.json(), delta_stats.json(),
        owners.len() * side.pow(3) * std::mem::size_of::<u32>(),
        retained.iter().map(|page| std::mem::size_of_val(page.splats.as_slice())).sum::<usize>(),
    );
}

#[test]
#[ignore = "run explicitly as a release-mode CPU benchmark"]
fn local_native_edit_cost() {
    let world = World::new(SEED);
    let single = [(
        Cell {
            x: 65,
            y: 67,
            z: -24,
        },
        Material::Air,
    )];
    assert_eq!(world.material(single[0].0), Material::Wood);
    let mut brush = Vec::new();
    for x in 62..66 {
        for y in 64..68 {
            for z in -34..-30 {
                let c = Cell { x, y, z };
                if world.material(c) == Material::Wood {
                    brush.push((c, Material::Air));
                }
            }
        }
    }
    assert_eq!(brush.len(), 32);
    let mut cave = Vec::new();
    for x in -73..-70 {
        for y in 47..49 {
            for z in 31..34 {
                let c = Cell { x, y, z };
                if world.material(c) == Material::Stone {
                    cave.push((c, Material::Air));
                }
            }
        }
    }
    assert!(!cave.is_empty());
    for side in [16, 32] {
        benchmark("bridge_single_cell", &single, side);
        benchmark("bridge_edge_brush", &brush, side);
        benchmark("cave_wall_brush", &cave, side);
    }
}
