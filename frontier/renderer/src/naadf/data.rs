//! Exact three-layer voxel snapshot; greenfield NAADF experiment.
//! 4³ voxels/block, 4³ blocks/chunk. Uniform nodes remain exact. Empty nodes
//! store a proven-empty cuboid, never six independently unsafe axis distances.
use bytemuck::{Pod, Zeroable};
use frontier_world::{
    vegetation::{self, Species},
    Cell, Material, World, VOXEL_SIZE,
};
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU32, Ordering},
    time::Instant,
};
const FULL: u32 = 1 << 30;
const CHILD: u32 = 1 << 31;
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Pigment {
    pub rgb: [f32; 3],
    pub material: u32,
}
#[derive(Debug)]
pub struct Snapshot {
    pub origin: Cell,
    pub chunks_per_edge: u32,
    /// Y-fast layout: (z*n+x)*n+y. Texture coordinates are (y,x,z).
    pub chunks: Vec<u32>,
    pub blocks: Vec<u32>,
    pub voxels: Vec<u32>,
    pub palette: Vec<Pigment>,
    pub build_ms: f64,
    pub source_revision: u64,
    pub mixed_chunks: usize,
    pub unique_blocks: usize,
    pub sampled_columns: usize,
    pub plant_cells: usize,
}
impl Snapshot {
    pub fn bytes(&self) -> usize {
        (self.chunks.len() + self.blocks.len().max(1) + self.voxels.len().max(1)) * 4
            + self.palette.len() * 16
    }
    pub fn bounds(&self) -> [[f64; 3]; 2] {
        let lo = [self.origin.x, self.origin.y, self.origin.z].map(|v| v as f64 * VOXEL_SIZE);
        [lo, lo.map(|v| v + self.chunks_per_edge as f64)]
    }
    #[cfg(test)]
    pub fn material(&self, p: [usize; 3]) -> u32 {
        let n = self.chunks_per_edge as usize;
        let c = p.map(|v| v / 16);
        let mut node = self.chunks[(c[2] * n + c[0]) * n + c[1]];
        if node & CHILD != 0 {
            let b = p.map(|v| (v % 16) / 4);
            node = self.blocks[(node & 0x3fff_ffff) as usize + b[0] + 4 * (b[1] + 4 * b[2])];
            if node & CHILD != 0 {
                let v = p.map(|v| v % 4);
                let i = v[0] + 4 * (v[1] + 4 * v[2]);
                let value = self.voxels[(node & 0x3fff_ffff) as usize + i];
                return if value & FULL != 0 { value } else { 0 };
            }
        }
        if node & FULL != 0 {
            FULL | (node as u32 & 0x3fff_ffff)
        } else {
            0
        }
    }
}
pub fn id(m: Material) -> u32 {
    match m {
        Material::Air => 0,
        Material::Grass => 1,
        Material::Soil => 2,
        Material::Stone => 3,
        Material::Wood => 4,
        Material::Leaf => 5,
        Material::Flower => 6,
    }
}
fn palette() -> Vec<Pigment> {
    [
        [0., 0., 0.],
        [0.18, 0.34, 0.10],
        [0.32, 0.22, 0.13],
        [0.35, 0.37, 0.39],
        [0.41, 0.25, 0.13],
        [0.40, 0.53, 0.25],
        [0.93, 0.92, 0.84],
    ]
    .into_iter()
    .enumerate()
    .map(|(i, rgb)| Pigment {
        rgb,
        material: i as u32,
    })
    .collect()
}
fn pigment(
    p: &mut Vec<Pigment>,
    map: &mut HashMap<[u32; 4], u32>,
    rgb: [f32; 3],
    m: Material,
) -> Result<u32, String> {
    // Visibility stays exact; RGB10 precision avoids treating insignificant
    // float variations as millions of distinct palette entries.
    let rgb = rgb.map(|v| (v.clamp(0., 1.) * 1023.).round() / 1023.);
    let key = [
        rgb[0].to_bits(),
        rgb[1].to_bits(),
        rgb[2].to_bits(),
        id(m) as u32,
    ];
    if let Some(&v) = map.get(&key) {
        return Ok(v);
    }
    if p.len() >= FULL as usize {
        return Err("NAADF palette address exhausted; snapshot not published".into());
    }
    let result = p.len() as u32;
    p.push(Pigment {
        rgb,
        material: id(m) as u32,
    });
    map.insert(key, result);
    Ok(result)
}
/// Expand one pair of faces using a neighbor's already proven-empty box.
/// Looking up the cell just beyond the old face and checking both transverse
/// extents proves the entire added slab is empty, including its corners.
fn expand(
    nodes: &[u32],
    dims: [usize; 3],
    strides: [usize; 3],
    i: usize,
    axis: usize,
    bits: u32,
) -> u32 {
    let old = nodes[i];
    if old & 0xc000_0000 != 0 {
        return old;
    }
    let mask = (1u32 << bits) - 1;
    let mut out = old;
    for positive in [false, true] {
        let side = axis * 2 + usize::from(positive);
        let shift = side as u32 * bits;
        let extent = (old >> shift) & mask;
        if extent == mask {
            continue;
        }
        let coordinate = (i / strides[axis]) % dims[axis];
        let gap = extent as usize + 1;
        let adjacent = if positive {
            coordinate.checked_add(gap).filter(|v| *v < dims[axis])
        } else {
            coordinate.checked_sub(gap)
        };
        let Some(adjacent) = adjacent else {
            continue;
        };
        let j = if positive {
            i + gap * strides[axis]
        } else {
            i - gap * strides[axis]
        };
        let neighbor = nodes[j];
        if neighbor & 0xc000_0000 != 0 {
            continue;
        }
        if (0..6)
            .filter(|s| s / 2 != axis)
            .any(|s| ((neighbor >> (s as u32 * bits)) & mask) < ((old >> (s as u32 * bits)) & mask))
        {
            continue;
        }
        let neighbor_extent = (neighbor >> shift) & mask;
        let available = if positive {
            dims[axis] - 1 - coordinate
        } else {
            coordinate
        };
        let value = (extent + 1 + neighbor_extent)
            .min(mask)
            .min(available as u32);
        let _ = adjacent;
        out = (out & !(mask << shift)) | (value << shift);
    }
    out
}
pub fn bounds(
    nodes: &mut Vec<u32>,
    dims: [usize; 3],
    strides: [usize; 3],
    bits: u32,
    cancel: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut scratch = vec![0; nodes.len()];
    // Deterministic Jacobi updates; every intermediate cache is safe to use.
    for _ in 0..((1 << bits) - 1) {
        let mut changed = false;
        for axis in 0..3 {
            for i in 0..nodes.len() {
                if i % 32768 == 0 && cancel() {
                    return Err("NAADF build superseded".into());
                }
                scratch[i] = expand(nodes, dims, strides, i, axis, bits);
                changed |= scratch[i] != nodes[i];
            }
            std::mem::swap(nodes, &mut scratch);
        }
        if !changed {
            break;
        }
    }
    Ok(())
}
fn pack_block(input: [u32; 64]) -> [u32; 64] {
    let mut nodes = input.to_vec();
    bounds(&mut nodes, [4; 3], [1, 4, 16], 2, &|| false).unwrap();
    nodes.try_into().unwrap()
}
fn budget(s: &Snapshot, overlay: usize, hash_capacity: usize, limit: usize) -> Result<(), String> {
    // New CPU + GPU payload, top-level Jacobi scratch and hash table workspace.
    // Includes capacity slack; resident old snapshot is reported separately.
    let estimate = s
        .chunks
        .capacity()
        .saturating_mul(12)
        .saturating_add((s.blocks.capacity() + s.voxels.capacity()).saturating_mul(8))
        .saturating_add(s.palette.capacity().saturating_mul(32))
        .saturating_add(overlay.saturating_mul(16))
        .saturating_add(hash_capacity.saturating_mul(304));
    if estimate > limit {
        return Err(format!(
            "NAADF build estimate {} MiB exceeds {} MiB working budget",
            estimate.div_ceil(1048576),
            limit / 1048576
        ));
    }
    Ok(())
}
fn address(c: Cell, min: Cell, n: usize) -> Option<u64> {
    let p = [c.x - min.x, c.y - min.y, c.z - min.z];
    let edge = n as i64 * 16;
    if p.iter().any(|v| *v < 0 || *v >= edge) {
        return None;
    }
    let q = p.map(|v| v as usize);
    let chunk = ((q[2] / 16 * n + q[0] / 16) * n + q[1] / 16) as u64;
    Some(chunk * 4096 + (q[0] % 16 + 16 * (q[1] % 16 + 16 * (q[2] % 16))) as u64)
}
fn cell_at(a: u64, min: Cell, n: usize) -> Cell {
    let c = a / 4096;
    let v = a % 4096;
    Cell {
        x: min.x + ((c as usize / n) % n * 16) as i64 + (v % 16) as i64,
        y: min.y + (c as usize % n * 16) as i64 + ((v / 16) % 16) as i64,
        z: min.z + (c as usize / (n * n) * 16) as i64 + (v / 256) as i64,
    }
}
pub fn build(
    world: &World,
    eye: [f64; 3],
    radius: u32,
    limit: usize,
    stage: &AtomicU32,
    cancel: impl Fn() -> bool,
) -> Result<Snapshot, String> {
    let start = Instant::now();
    if eye.iter().any(|v| !v.is_finite() || v.abs() > 1e12) {
        return Err("NAADF eye outside supported finite coordinate range".into());
    }
    let n = radius.checked_mul(2).ok_or("NAADF radius overflow")? as usize;
    if n == 0 || n > 512 {
        return Err("NAADF radius must be 1..256 metres".into());
    }
    let count = n.checked_pow(3).ok_or("NAADF chunk count overflow")?;
    if count.checked_mul(12).is_none_or(|v| v > limit) {
        return Err(format!(
            "NAADF top grid plus build/copy workspace alone needs {} MiB",
            count.saturating_mul(12).div_ceil(1048576)
        ));
    }
    let centre = Cell::from_meters(eye);
    let r = radius as i64 * 16;
    let min = Cell {
        x: centre.x.div_euclid(16) * 16 - r,
        y: centre.y.div_euclid(16) * 16 - r,
        z: centre.z.div_euclid(16) * 16 - r,
    };
    let mut out = Snapshot {
        origin: min,
        chunks_per_edge: n as u32,
        chunks: vec![0; count],
        blocks: vec![],
        voxels: vec![],
        palette: palette(),
        build_ms: 0.,
        source_revision: world.revision(),
        mixed_chunks: 0,
        unique_blocks: 0,
        sampled_columns: 0,
        plant_cells: 0,
    };
    let mut pigments = HashMap::new();
    let mut overlay = Vec::<u128>::new();
    stage.store(1, Ordering::Relaxed);
    let pad = vegetation::MAX_PLANT_RADIUS_CELLS;
    let mut records = world.vegetation_records(
        [min.x - pad, min.z - pad],
        [min.x + n as i64 * 16 + pad, min.z + n as i64 * 16 + pad],
    );
    // For intersections between models below, ask the canonical lookup rather
    // than infer precedence from this enumeration's ordering.
    records.sort_by_key(|p| {
        (
            match p.species {
                Species::Rowan => 0,
                Species::Grass => 1,
                Species::Edelweiss => 2,
            },
            p.root.x,
            p.root.z,
            p.seed,
        )
    });
    for record in records {
        if cancel() {
            return Err("NAADF build superseded".into());
        }
        let radius = if record.species == Species::Rowan {
            vegetation::MAX_PLANT_RADIUS_CELLS
        } else {
            8
        };
        let height = if record.species == Species::Rowan {
            vegetation::MAX_PLANT_HEIGHT_CELLS
        } else {
            12
        };
        if record.root.x + radius < min.x
            || record.root.x - radius >= min.x + n as i64 * 16
            || record.root.z + radius < min.z
            || record.root.z - radius >= min.z + n as i64 * 16
            || record.root.y + height < min.y
            || record.root.y >= min.y + n as i64 * 16
        {
            continue;
        }
        let Some(model) = vegetation::model_for_cancellable(&record, &cancel) else {
            return Err("NAADF build superseded".into());
        };
        for v in &model.voxels {
            if let Some(a) = address(record.world_cell(v.cell), min, n) {
                let ty = pigment(&mut out.palette, &mut pigments, v.color, v.material)?;
                overlay.push(((a as u128) << 32) | ty as u128);
            }
        }
        budget(&out, overlay.capacity(), pigments.capacity(), limit)?;
    }
    overlay.sort_unstable();
    let mut compact = 0;
    let mut i = 0;
    while i < overlay.len() {
        if i % 4096 == 0 && cancel() {
            return Err("NAADF build superseded".into());
        }
        let a = overlay[i] >> 32;
        let mut j = i + 1;
        while j < overlay.len() && overlay[j] >> 32 == a {
            j += 1;
        }
        let ty = if j - i == 1 {
            overlay[i] as u32
        } else {
            let v = world
                .vegetation_voxel(cell_at(a as u64, min, n))
                .ok_or("canonical plant overlap lookup failed")?;
            pigment(&mut out.palette, &mut pigments, v.color, v.material)?
        };
        overlay[compact] = (a << 32) | ty as u128;
        compact += 1;
        i = j;
    }
    overlay.truncate(compact);
    out.plant_cells = compact;
    // Explicit edits override both terrain and plants. Material-only edited
    // cells use the ordinary semantic palette, matching canonical material.
    let edits: HashMap<u64, u32> = world
        .edit_cells()
        .filter_map(|c| address(c, min, n).map(|a| (a, id(world.material(c)))))
        .collect();
    let edited_columns: HashSet<_> = world
        .edit_cells()
        .filter(|c| {
            c.x >= min.x
                && c.x < min.x + n as i64 * 16
                && c.z >= min.z
                && c.z < min.z + n as i64 * 16
        })
        .map(|c| (c.x.div_euclid(16), c.z.div_euclid(16)))
        .collect();
    let mut leaves = HashMap::<[u32; 64], u32>::new();
    let mut cursor = 0;
    stage.store(2, Ordering::Relaxed);
    for cz in 0..n {
        for cx in 0..n {
            if cancel() {
                return Err("NAADF build superseded".into());
            }
            let columns: Vec<_> = (0..16)
                .flat_map(|z| (0..16).map(move |x| (x, z)))
                .map(|(x, z)| {
                    world.column(min.x + (cx * 16 + x) as i64, min.z + (cz * 16 + z) as i64)
                })
                .collect();
            let heights: Vec<_> = columns
                .iter()
                .map(|c| (c.height_m() / VOXEL_SIZE) as i64)
                .collect();
            let bands: Vec<_> = columns
                .iter()
                .zip(&heights)
                .map(|(c, h)| {
                    [
                        id(c.material(h - 1)),
                        id(c.material(h - 2)),
                        id(c.material(h - 13)),
                    ]
                })
                .collect();
            out.sampled_columns += 256;
            let fast = world.fbm_terrain().is_some()
                && !world.volume_owned(Cell {
                    x: min.x + (cx * 16) as i64,
                    y: 0,
                    z: min.z + (cz * 16) as i64,
                })
                && !edited_columns.contains(&(
                    min.x.div_euclid(16) + cx as i64,
                    min.z.div_euclid(16) + cz as i64,
                ));
            for cy in 0..n {
                let ci = (cz * n + cx) * n + cy;
                let y0 = min.y + (cy * 16) as i64;
                let plant_start = cursor;
                while cursor < overlay.len() && (overlay[cursor] >> 44) == ci as u128 {
                    cursor += 1;
                }
                let clean = cursor == plant_start && edits.is_empty();
                if fast && clean && heights.iter().all(|h| y0 >= *h) {
                    continue;
                }
                if fast
                    && clean
                    && heights.iter().all(|h| y0 + 15 < *h - 12)
                    && bands.iter().all(|b| b[2] == bands[0][2])
                {
                    out.chunks[ci] = FULL | bands[0][2] as u32;
                    continue;
                }
                let mut cells = [0u32; 4096];
                for z in 0..16 {
                    for x in 0..16 {
                        let col = z * 16 + x;
                        let h = heights[col];
                        for y in 0..16 {
                            let py = y0 + y as i64;
                            let ty = if fast {
                                if py >= h {
                                    0
                                } else if py == h - 1 {
                                    bands[col][0]
                                } else if py >= h - 12 {
                                    bands[col][1]
                                } else {
                                    bands[col][2]
                                }
                            } else {
                                id(columns[col].material(py))
                            };
                            cells[x + 16 * (y + 16 * z)] = if ty == 0 { 0 } else { FULL | ty };
                        }
                    }
                }
                for p in &overlay[plant_start..cursor] {
                    let vi = ((p >> 32) & 4095) as usize;
                    if cells[vi] == 0 {
                        cells[vi] = FULL | (*p as u32);
                    }
                }
                if !edits.is_empty() {
                    for (vi, c) in cells.iter_mut().enumerate() {
                        if let Some(&ty) = edits.get(&(ci as u64 * 4096 + vi as u64)) {
                            *c = if ty == 0 { 0 } else { FULL | ty };
                        }
                    }
                }
                if cells.iter().all(|c| *c == cells[0]) {
                    out.chunks[ci] = if cells[0] == 0 {
                        0
                    } else {
                        FULL | (cells[0] & 0x3fff_ffff) as u32
                    };
                    continue;
                }
                let mut blocks = vec![0u32; 64];
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let values: [u32; 64] = std::array::from_fn(|i| {
                                cells[bx * 4
                                    + i % 4
                                    + 16 * (by * 4 + (i / 4) % 4 + 16 * (bz * 4 + i / 16))]
                            });
                            let bi = bx + 4 * (by + 4 * bz);
                            if values.iter().all(|v| *v == values[0]) {
                                blocks[bi] = if values[0] == 0 {
                                    0
                                } else {
                                    FULL | (values[0] & 0x3fff_ffff) as u32
                                };
                                continue;
                            }
                            let values = pack_block(values);
                            let pointer = if let Some(&p) = leaves.get(&values) {
                                p
                            } else {
                                let p = u32::try_from(out.voxels.len())
                                    .map_err(|_| "NAADF voxel buffer address overflow")?;
                                if p >= FULL {
                                    return Err("NAADF voxel address exhausted".into());
                                }
                                out.voxels.extend(values);
                                leaves.insert(values, p);
                                p
                            };
                            blocks[bi] = CHILD | pointer;
                        }
                    }
                }
                bounds(&mut blocks, [4; 3], [1, 4, 16], 2, &cancel)?;
                let p =
                    u32::try_from(out.blocks.len()).map_err(|_| "NAADF block address overflow")?;
                if p >= FULL {
                    return Err("NAADF block address exhausted".into());
                }
                out.blocks.extend(blocks);
                out.chunks[ci] = CHILD | p;
                out.mixed_chunks += 1;
            }
            budget(
                &out,
                overlay.capacity(),
                leaves.capacity() + pigments.capacity(),
                limit,
            )?;
            stage.store(
                10 + ((cz * n + cx) * 70 / (n * n)) as u32,
                Ordering::Relaxed,
            );
        }
    }
    out.unique_blocks = leaves.len();
    drop(leaves);
    drop(overlay);
    drop(pigments);
    stage.store(85, Ordering::Relaxed);
    bounds(&mut out.chunks, [n; 3], [n, 1, n * n], 5, &cancel)?;
    out.build_ms = start.elapsed().as_secs_f64() * 1000.;
    stage.store(100, Ordering::Relaxed);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rgb10_pigments_and_leaf_words_survive_more_than_32767_colors() {
        let mut palette = palette();
        let mut map = HashMap::new();
        let mut last = 0;
        for i in 0..40000u32 {
            last = pigment(
                &mut palette,
                &mut map,
                [(i & 255) as f32 / 255., (i >> 8) as f32 / 255., 0.7],
                Material::Leaf,
            )
            .unwrap();
        }
        assert!(last > 32767);
        let mut cells = [0u32; 64];
        cells[23] = FULL | last;
        let block = pack_block(cells);
        assert_eq!(block[23], FULL | last);
        let source = [(39999 & 255) as f32 / 255., (39999 >> 8) as f32 / 255., 0.7];
        for (actual, original) in palette[last as usize].rgb.into_iter().zip(source) {
            assert!((actual - original).abs() <= 0.5 / 1023. + 1e-7);
        }
    }
    #[test]
    fn empty_cuboids_never_cross_occupied_cells() {
        for seed in 1..33u64 {
            let mut rng = seed;
            let mut a: Vec<u32> = (0..64)
                .map(|_| {
                    rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                    if rng >> 61 == 0 {
                        FULL | 1
                    } else {
                        0
                    }
                })
                .collect();
            let occupied = a.clone();
            bounds(&mut a, [4; 3], [1, 4, 16], 2, &|| false).unwrap();
            for i in 0..64 {
                if occupied[i] != 0 {
                    continue;
                }
                let c = [i % 4, (i / 4) % 4, i / 16];
                for z in c[2] - ((a[i] >> 8) & 3) as usize..=c[2] + ((a[i] >> 10) & 3) as usize {
                    for y in c[1] - ((a[i] >> 4) & 3) as usize..=c[1] + ((a[i] >> 6) & 3) as usize {
                        for x in c[0] - (a[i] & 3) as usize..=c[0] + ((a[i] >> 2) & 3) as usize {
                            assert_eq!(occupied[x + 4 * (y + 4 * z)], 0);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn y_fast_top_bounds_are_sound() {
        let mut v = vec![0; 8 * 8 * 8];
        v[(3 * 8 + 4) * 8 + 2] = FULL | 1;
        bounds(&mut v, [8; 3], [8, 1, 64], 5, &|| false).unwrap();
        for i in 0..v.len() {
            if v[i] & FULL != 0 {
                continue;
            }
            let c = [(i / 8) % 8, i % 8, i / 64];
            let mut lo = [0; 3];
            let mut hi = [0; 3];
            for a in 0..3 {
                lo[a] = c[a] - ((v[i] >> (a * 10)) & 31) as usize;
                hi[a] = c[a] + ((v[i] >> (a * 10 + 5)) & 31) as usize;
            }
            assert!(
                !(lo[0] <= 4 && hi[0] >= 4 && lo[1] <= 2 && hi[1] >= 2 && lo[2] <= 3 && hi[2] >= 3)
            );
        }
    }
    #[test]
    fn snapshot_matches_canonical_fixture_with_edit() {
        let mut w = World::new(7);
        w.set(Cell { x: 0, y: 30, z: 0 }, Material::Wood);
        let s = build(
            &w,
            [0., 3., 0.],
            1,
            32 * 1048576,
            &AtomicU32::new(0),
            || false,
        )
        .unwrap();
        for z in 0..32 {
            for y in 0..32 {
                for x in 0..32 {
                    let c = Cell {
                        x: s.origin.x + x as i64,
                        y: s.origin.y + y as i64,
                        z: s.origin.z + z as i64,
                    };
                    assert_eq!(s.material([x, y, z]) & 0x3fff_ffff, id(w.material(c)));
                }
            }
        }
    }
    #[test]
    fn fbm_and_seeded_plants_match_canonical_materials() {
        let world = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::Alpine,
                ..Default::default()
            },
        )
        .unwrap()
        .with_vegetation();
        let eye = [-41., world.height_m(-41., -250.) + 1.7, -250.];
        let s = build(&world, eye, 2, 128 * 1048576, &AtomicU32::new(0), || false).unwrap();
        assert!(s.plant_cells > 0);
        let mut rng = 123u64;
        for _ in 0..4096 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let p = [
                (rng & 63) as usize,
                ((rng >> 9) & 63) as usize,
                ((rng >> 18) & 63) as usize,
            ];
            let c = Cell {
                x: s.origin.x + p[0] as i64,
                y: s.origin.y + p[1] as i64,
                z: s.origin.z + p[2] as i64,
            };
            let ty = s.material(p) & 0x3fff_ffff;
            assert_eq!(
                s.palette[ty as usize].material,
                id(world.material(c)) as u32,
                "{c:?}"
            );
        }
    }
    #[test]
    fn oversized_request_is_rejected_before_allocation() {
        assert!(build(
            &World::new(7),
            [0.; 3],
            256,
            16 * 1048576,
            &AtomicU32::new(0),
            || false
        )
        .unwrap_err()
        .contains("top grid"));
    }
}
