use bytemuck::{Pod, Zeroable};
use frontier_world::{
    affine_bounds::{
        bound_eroded_ground_ceiling, bound_eroded_ground_drainage, bound_eroded_ground_interval,
        bound_native_height, BoundBudget, ColumnBox,
    },
    FbmTerrain, Material,
};

pub const ROOT_LOG: u32 = 18;
pub const ROOT: u32 = 1 << ROOT_LOG;
pub const NATIVE_PITCH: f64 = 0.0625;
const PAGE_FLAG: u32 = 0x8000_0000;
fn node_bits(page_side: u32) -> u32 {
    if page_side == 64 {
        22
    } else {
        23
    }
}
fn block_of(key: u32, bits: u32) -> Option<u32> {
    (key >> bits).checked_sub(1)
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct Node {
    pub ceiling: f32,
    link: u32,
    parent: u32,
}

#[derive(Clone, Copy)]
struct Tile {
    min: [i64; 2],
    width: u32,
    exact_ceiling: bool,
    refined_ceiling: bool,
}
impl Tile {
    fn child(self, quadrant: u32) -> Self {
        let width = self.width / 2;
        let x = i64::from((quadrant & 1) * width);
        let z = i64::from((quadrant >> 1) * width);
        Self {
            min: [self.min[0] + x, self.min[1] + z],
            width,
            exact_ceiling: false,
            refined_ceiling: false,
        }
    }
}
enum Build {
    Branch([f32; 4]),
    Page(Vec<i32>, f32),
    Roof(f32),
}

pub fn encode_material(material: Material) -> i32 {
    match material {
        Material::Grass => 1,
        Material::Soil => 2,
        Material::Sand => 3,
        Material::Snow => 4,
        _ => 0,
    }
}
pub fn albedo(id: u32) -> [f32; 3] {
    match id {
        1 => Material::Grass,
        2 => Material::Soil,
        3 => Material::Sand,
        4 => Material::Snow,
        _ => Material::Stone,
    }
    .color()
}

pub struct Request {
    pub key: u32,
    pub count: u32,
    pub minimum: f32,
}
type Job = (usize, Tile, f32, f32);
type Completed = (usize, Tile, f32, Build);

pub struct Workers {
    queues: Vec<std::sync::mpsc::Sender<Vec<Job>>>,
    results: std::sync::mpsc::Receiver<Vec<Completed>>,
    handles: Vec<std::thread::JoinHandle<()>>,
}
impl Workers {
    pub fn new(
        terrain: &FbmTerrain,
        count: usize,
        page_side: u32,
        land_bound: bool,
        interval_bounds: bool,
        drainage_bounds: bool,
        refine_roofs: bool,
        block_packing: bool,
    ) -> Self {
        let (done, results) = std::sync::mpsc::channel();
        let mut queues = Vec::with_capacity(count);
        let mut handles = Vec::with_capacity(count);
        for _ in 0..count {
            let (send, receive) = std::sync::mpsc::channel::<Vec<Job>>();
            queues.push(send);
            let done = done.clone();
            let terrain = terrain.clone();
            handles.push(std::thread::spawn(move || {
                while let Ok(jobs) = receive.recv() {
                    let ready = jobs
                        .into_iter()
                        .map(|(index, tile, parent, minimum)| {
                            let bound = |subtile, roof| {
                                ceiling(
                                    &terrain,
                                    subtile,
                                    roof,
                                    land_bound,
                                    interval_bounds,
                                    drainage_bounds,
                                )
                            };
                            let build = if tile.width > page_side {
                                Build::Branch(std::array::from_fn(|i| {
                                    bound(tile.child(i as u32), parent)
                                }))
                            } else if refine_roofs
                                && block_of(index as u32, node_bits(page_side)).is_some()
                            {
                                let roof = bound(tile, parent);
                                let stored = (roof * 16.).floor() * 0.0625 + 0.003;
                                if roof < minimum && stored < minimum {
                                    Build::Roof(roof)
                                } else {
                                    fill_page(&terrain, tile, minimum, block_packing)
                                }
                            } else if refine_roofs
                                && !tile.refined_ceiling
                                && minimum.is_finite()
                                && tile.width >= 8
                            {
                                refined_ceiling(tile, parent, minimum, &bound).map_or_else(
                                    || fill_page(&terrain, tile, minimum, block_packing),
                                    Build::Roof,
                                )
                            } else {
                                fill_page(&terrain, tile, minimum, block_packing)
                            };
                            (index, tile, minimum, build)
                        })
                        .collect();
                    if done.send(ready).is_err() {
                        break;
                    }
                }
            }));
        }
        Self {
            queues,
            results,
            handles,
        }
    }

    fn build(&self, jobs: &mut [Job]) -> Vec<Completed> {
        // Keep nearby generation on the same worker to retain its local
        // drainage, climate and erosion caches across visibility rounds.
        jobs.sort_unstable_by_key(|(_, tile, _, _)| {
            (tile.min[1].div_euclid(1024), tile.min[0], tile.min[1])
        });
        let block = jobs.len().div_ceil(self.queues.len());
        let blocks = jobs.len().div_ceil(block);
        for (queue, slice) in self.queues.iter().zip(jobs.chunks(block)) {
            queue.send(slice.to_vec()).expect("terrain worker stopped");
        }
        let mut completed = Vec::with_capacity(jobs.len());
        for _ in 0..blocks {
            completed.extend(self.results.recv().expect("terrain worker stopped"));
        }
        // Preserve deterministic tree IDs despite different worker completion orders.
        completed.sort_unstable_by_key(|(index, _, _, _)| *index);
        completed
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        self.queues.clear();
        for thread in self.handles.drain(..) {
            let _ = thread.join();
        }
    }
}

fn ceiling(
    terrain: &FbmTerrain,
    tile: Tile,
    parent: f32,
    land_bound: bool,
    interval_bounds: bool,
    drainage_bounds: bool,
) -> f32 {
    let region = ColumnBox {
        min: tile.min,
        size_cells: [tile.width; 2],
    };
    let budget = BoundBudget {
        max_noise_cells: 512,
        max_cells_per_noise: 4,
    };
    let bound = land_bound
        .then(|| {
            if drainage_bounds && tile.width <= 4096 {
                bound_eroded_ground_drainage(terrain, region, budget)
            } else if interval_bounds {
                bound_eroded_ground_interval(terrain, region, budget)
            } else {
                bound_eroded_ground_ceiling(terrain, region, budget)
            }
        })
        .flatten()
        .or_else(|| bound_native_height(terrain, region, budget).map(|b| b.height_range[1]));
    // Round outward again before evaluating the bound in f32.
    bound.map_or(parent, |height| {
        ((height as f32).next_up() + 0.003).min(parent)
    })
}

fn refined_ceiling(
    tile: Tile,
    parent: f32,
    minimum: f32,
    bound: &impl Fn(Tile, f32) -> f32,
) -> Option<f32> {
    let mut roof = f32::NEG_INFINITY;
    for q in 0..4 {
        let child = tile.child(q);
        let child_roof = bound(child, parent);
        if child_roof < minimum {
            roof = roof.max(child_roof);
        } else {
            for r in 0..4 {
                let leaf_roof = bound(child.child(r), child_roof);
                if leaf_roof >= minimum {
                    return None;
                }
                roof = roof.max(leaf_roof);
            }
        }
    }
    Some(roof)
}

pub struct Cache {
    pub nodes: Vec<Node>,
    tiles: Vec<Tile>,
    pub pages: u32,
    pub words: u32,
    word_limit: u32,
    node_limit: usize,
    pub page_side: u32,
    pub origin: [i64; 2],
    pub branch_jobs: usize,
    pub certificates: usize,
    pub refined_roofs: usize,
    pub resident_sparse_blocks: usize,
    sparse_certified: std::collections::HashSet<u32>,
    pub page_jobs: usize,
}
pub struct Upload {
    pub first_word: u32,
    pub columns: Vec<i32>,
    pub patches: Vec<[u32; 2]>,
    pub first_node: usize,
    pub bound_patches: Vec<[u32; 2]>,
    pub jobs: usize,
}
impl Cache {
    pub fn new(eye: [f64; 3], page_side: u32, word_limit: u32, node_limit: usize) -> Self {
        let origin = [eye[0], eye[2]].map(|m| {
            let cell = (m / NATIVE_PITCH).floor() as i64;
            cell.div_euclid(1 << 10) * (1 << 10) - i64::from(ROOT / 2)
        });
        let nodes = vec![Node {
            ceiling: f32::INFINITY,
            link: 0,
            parent: 0,
        }];
        let tiles = vec![Tile {
            min: origin,
            width: ROOT,
            exact_ceiling: false,
            refined_ceiling: false,
        }];
        Self {
            nodes,
            tiles,
            pages: 0,
            words: 0,
            word_limit,
            node_limit,
            page_side,
            origin,
            branch_jobs: 0,
            certificates: 0,
            refined_roofs: 0,
            resident_sparse_blocks: 0,
            sparse_certified: Default::default(),
            page_jobs: 0,
        }
    }

    pub fn node_of(&self, key: u32) -> usize {
        let bits = node_bits(self.page_side);
        (key & ((1 << bits) - 1)) as usize
    }

    pub fn cover(&self, eye: [f64; 3], distance: f64) -> bool {
        [eye[0], eye[2]]
            .iter()
            .zip(self.origin)
            .all(|(&position, start)| {
                let base = start as f64 * NATIVE_PITCH;
                position - distance >= base
                    && position + distance < base + f64::from(ROOT) * NATIVE_PITCH
            })
    }

    pub fn fill(
        &mut self,
        workers: &Workers,
        requests: &[Request],
        batch: usize,
        prefetch: bool,
        keep_certificates: bool,
    ) -> Upload {
        let mut jobs = Vec::with_capacity(batch.min(requests.len()));
        let mut reserved_nodes = self.nodes.len();
        let mut reserved_words = self.words;
        let page_words = 2 + self.page_side.pow(2) + (self.page_side / 4).pow(2);
        let mut predicted = Vec::new();
        if prefetch {
            for &Request { key, .. } in requests {
                if self.tiles[key as usize].width == self.page_side {
                    let first = (key - 1) / 4 * 4 + 1;
                    predicted.extend(first..first + 4);
                }
            }
            predicted.sort_unstable();
            predicted.dedup();
        }
        let mut selected = if prefetch {
            vec![false; self.nodes.len()]
        } else {
            Vec::new()
        };
        let mut active_tables = std::collections::HashSet::new();
        let mut page_work = 0;
        for (key, minimum) in requests
            .iter()
            .map(|q| (q.key, q.minimum))
            .chain(predicted.into_iter().map(|i| (i, f32::NEG_INFINITY)))
        {
            if jobs.len() == batch {
                break;
            }
            let index = self.node_of(key);
            let node = self.nodes[index];
            let block = block_of(key, node_bits(self.page_side));
            if block.is_none() && (node.link != 0 || (prefetch && selected[index])) {
                continue;
            }
            let tile = self.tiles[index];
            if let Some(block) = block {
                let blocks = self.page_side / 4;
                let new_table = node.link == 0 && !active_tables.contains(&index);
                let reserve = if new_table { blocks * blocks } else { 0 };
                let payload = 19; // signed base, encoding, maximum, up to 16 native columns
                if reserved_words + reserve + payload <= self.word_limit {
                    if new_table {
                        active_tables.insert(index);
                    }
                    reserved_words += reserve + payload;
                } else if self.sparse_certified.contains(&key) {
                    continue;
                }
                let min = [
                    tile.min[0] + i64::from(4 * (block % blocks)),
                    tile.min[1] + i64::from(4 * (block / blocks)),
                ];
                jobs.push((
                    key as usize,
                    Tile {
                        min,
                        width: 4,
                        ..tile
                    },
                    node.ceiling,
                    minimum,
                ));
                continue;
            }
            if tile.width == self.page_side {
                if page_work >= 8_388_608 / tile.width.pow(2) {
                    continue;
                }
                if reserved_words + page_words <= self.word_limit {
                    reserved_words += page_words;
                } else if !keep_certificates || tile.exact_ceiling || !minimum.is_finite() {
                    continue;
                }
                page_work += 1;
            } else {
                if reserved_nodes + 4 > self.node_limit {
                    continue;
                }
                reserved_nodes += 4;
            }
            if prefetch {
                selected[index] = true;
            }
            let minimum = if keep_certificates {
                minimum
            } else {
                f32::NEG_INFINITY
            };
            jobs.push((index, tile, node.ceiling, minimum));
        }

        let first_word = self.words;
        let first_node = self.nodes.len();
        let mut columns = Vec::with_capacity((reserved_words - first_word) as usize);
        if jobs.is_empty() {
            return Upload {
                columns,
                patches: Vec::new(),
                first_node,
                bound_patches: Vec::new(),
                first_word,
                jobs: 0,
            };
        }
        let output = workers.build(&mut jobs);
        let mut progress = 0;
        let mut patches = Vec::new();
        let mut bound_patches = Vec::new();
        let mut dirty = std::collections::BinaryHeap::<usize>::new();
        for (key, tile, minimum, result) in output {
            let index = self.node_of(key as u32);
            if let Some(block) = block_of(key as u32, node_bits(self.page_side)) {
                let (data, ceiling) = match result {
                    Build::Page(data, ceiling) => {
                        self.page_jobs += 1;
                        (data, ceiling)
                    }
                    Build::Roof(ceiling) => {
                        self.refined_roofs += 1;
                        (Vec::new(), ceiling)
                    }
                    Build::Branch(_) => unreachable!(),
                };
                assert!(ceiling <= self.nodes[index].ceiling + 0.003);
                let table_size = (self.page_side / 4).pow(2);
                let new_table = self.nodes[index].link == 0;
                let need_certificate = ceiling < minimum;
                let required = if new_table { table_size } else { 0 };
                if self.words + required + data.len() as u32 > self.word_limit {
                    continue;
                }
                if new_table {
                    self.nodes[index].link = PAGE_FLAG | self.words;
                    bound_patches.push([3 * index as u32 + 1, self.nodes[index].link]);
                    self.words += table_size;
                    columns.resize((self.words - first_word) as usize, 0);
                    self.pages += 1;
                }
                let table = self.nodes[index].link & !PAGE_FLAG;
                let code = if need_certificate {
                    // Tops are integer voxels, so flooring an outward-rounded
                    // continuous ceiling retains an upper bound on every top.
                    let height = (ceiling * 16.).floor() as i32;
                    self.sparse_certified.insert(key as u32);
                    0x80000000 | ((height as u32) & 0x00ffffff)
                } else {
                    let offset = self.words;
                    self.words += data.len() as u32;
                    columns.extend(data);
                    self.resident_sparse_blocks += 1;
                    self.sparse_certified.remove(&(key as u32));
                    offset + 1
                };
                patches.push([table + block, code]);
                progress += 1;
                continue;
            }
            match result {
                Build::Branch(bounds) => {
                    self.nodes[index].link = self.nodes.len() as u32;
                    bound_patches.push([3 * index as u32 + 1, self.nodes[index].link]);
                    dirty.push(index);
                    self.branch_jobs += 1;
                    progress += 1;
                    for (quadrant, ceiling) in bounds.into_iter().enumerate() {
                        self.nodes.push(Node {
                            ceiling,
                            link: 0,
                            parent: index as u32,
                        });
                        self.tiles.push(tile.child(quadrant as u32));
                    }
                }
                Build::Roof(ceiling) => {
                    self.nodes[index].ceiling = self.nodes[index].ceiling.min(ceiling);
                    bound_patches.push([3 * index as u32, self.nodes[index].ceiling.to_bits()]);
                    dirty.push(self.nodes[index].parent as usize);
                    self.tiles[index].refined_ceiling = true;
                    self.refined_roofs += 1;
                    progress += 1;
                }
                Build::Page(data, ceiling) => {
                    assert!(
                        ceiling <= self.nodes[index].ceiling + 0.003,
                        "opaque ground exceeded its generation bound at {:?}",
                        tile.min,
                    );
                    self.page_jobs += 1;
                    if keep_certificates
                        && (ceiling < minimum || self.words + data.len() as u32 > self.word_limit)
                    {
                        if self.nodes[index].ceiling > ceiling {
                            progress += 1;
                            bound_patches.push([3 * index as u32, ceiling.to_bits()]);
                            dirty.push(self.nodes[index].parent as usize);
                        }
                        if !self.tiles[index].exact_ceiling {
                            self.certificates += 1;
                        }
                        self.nodes[index].ceiling = self.nodes[index].ceiling.min(ceiling);
                        self.tiles[index].exact_ceiling = true;
                        continue;
                    }
                    progress += 1;
                    let ceiling = self.nodes[index].ceiling.min(ceiling);
                    self.nodes[index] = Node {
                        ceiling,
                        link: PAGE_FLAG | self.words,
                        parent: self.nodes[index].parent,
                    };
                    bound_patches.push([3 * index as u32, ceiling.to_bits()]);
                    bound_patches.push([3 * index as u32 + 1, self.nodes[index].link]);
                    dirty.push(self.nodes[index].parent as usize);
                    self.pages += 1;
                    self.words += data.len() as u32;
                    columns.extend(data);
                }
            }
        }
        // Parent IDs precede child IDs. Process affected subtrees in
        // reverse order, and compute each shared parent after its children.
        let mut previous = None;
        while let Some(index) = dirty.pop() {
            if previous == Some(index) {
                continue;
            }
            previous = Some(index);
            let node = self.nodes[index];
            let at = node.link as usize;
            let ceiling = self.nodes[at..at + 4]
                .iter()
                .fold(f32::NEG_INFINITY, |h, child| h.max(child.ceiling));
            if ceiling < node.ceiling {
                self.nodes[index].ceiling = ceiling;
                bound_patches.push([3 * index as u32, ceiling.to_bits()]);
                if index != 0 {
                    dirty.push(node.parent as usize);
                }
            }
        }
        Upload {
            columns,
            patches,
            first_node,
            bound_patches,
            first_word,
            jobs: progress,
        }
    }
}

fn fill_page(terrain: &FbmTerrain, tile: Tile, minimum: f32, block_packing: bool) -> Build {
    let mut data = Vec::with_capacity(tile.width.pow(2) as usize);
    let mut ceiling = f32::NEG_INFINITY;
    let [base_x, base_z] = tile.min;
    for z in 0..i64::from(tile.width) {
        for x in 0..i64::from(tile.width) {
            let c = terrain.column_cells(base_x + x, base_z + z);
            let solid = c.ground_top as i32;
            ceiling = ceiling.max(solid as f32 * 0.0625);
            data.push((solid << 8) | encode_material(c.ground_surface));
        }
    }
    let ceiling = ceiling.next_up() + 0.003;
    let packed = if ceiling < minimum {
        Vec::new()
    } else {
        let side = tile.width as usize;
        let regular = pack_page(&data, side);
        if block_packing && regular[1] != 8 {
            pack_blocks(&data, &regular, side).unwrap_or(regular)
        } else {
            regular
        }
    };
    Build::Page(packed, ceiling)
}

// Lossless 8-, 16- or 32-bit column words. The two packed forms keep
// a signed base height for the page and store each offset with 3 material bits.
fn pack_page(full: &[i32], side: usize) -> Vec<i32> {
    let base = full.iter().map(|&value| value >> 8).min().unwrap();
    let maximum = full.iter().map(|&value| value >> 8).max().unwrap();
    let span = maximum - base;
    let bits = if span <= 31 {
        8
    } else if span <= 8191 {
        16
    } else {
        32
    };
    let mut output = Vec::with_capacity(2 + (side / 4).pow(2) + full.len() / (32 / bits));
    output.extend([base, bits as i32]);
    for block_z in (0..side).step_by(4) {
        for block_x in (0..side).step_by(4) {
            let mut max = i32::MIN;
            for z in block_z..block_z + 4 {
                for x in block_x..block_x + 4 {
                    max = max.max(full[z * side + x] >> 8);
                }
            }
            output.push(max);
        }
    }
    if bits == 32 {
        output.extend_from_slice(full);
    } else {
        for group in full.chunks_exact(32 / bits) {
            let mut packed = 0u32;
            for (i, &column) in group.iter().enumerate() {
                let entry = (((column >> 8) - base) << 3) | (column & 7);
                packed |= (entry as u32) << (i * bits);
            }
            output.push(packed as i32);
        }
    }
    output
}

// Use the existing 4x4 ceiling as each block's signed height origin.
// Groups of up to 32 blocks store a wide-block bitmask and one payload offset.
fn pack_blocks(full: &[i32], regular: &[i32], side: usize) -> Option<Vec<i32>> {
    let blocks = (side / 4).pow(2);
    let maxima = &regular[2..2 + blocks];
    let masks = blocks.div_ceil(32) * 2;
    let mut packed = Vec::with_capacity(regular.len());
    packed.extend([0, 0]);
    packed.extend_from_slice(maxima);
    let group_start = packed.len();
    let data_start = group_start + masks;
    packed.resize(data_start, 0);
    for (block, &height) in maxima.iter().enumerate() {
        let group = group_start + 2 * (block / 32);
        if block % 32 == 0 {
            packed[group] = (packed.len() - data_start) as i32;
        }
        let x = 4 * (block % (side / 4));
        let z = 4 * (block / (side / 4));
        let values: [i32; 16] = std::array::from_fn(|i| {
            let column = full[(z + i / 4) * side + x + i % 4];
            let down = height - (column >> 8);
            (down << 3) | (column & 7)
        });
        let maximum = *values.iter().max().unwrap();
        if maximum > 65535 {
            return None;
        }
        let bits = if maximum > 255 {
            packed[group + 1] |= (1u32 << (block % 32)) as i32;
            16
        } else {
            8
        };
        for chunk in values.chunks_exact(32 / bits) {
            let word = chunk.iter().enumerate().fold(0u32, |packed, (i, &value)| {
                packed | ((value as u32) << (i * bits))
            });
            packed.push(word as i32);
        }
    }
    (packed.len() < regular.len()).then_some(packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_words_preserve_signed_heights_and_materials() {
        for (gap, bits) in [(2, 8), (20, 16), (600, 32)] {
            let original: Vec<_> = (0..16).map(|i| ((i * gap - 777) << 8) | (i % 5)).collect();
            let data = pack_page(&original, 4);
            assert_eq!(data[0], -777);
            assert_eq!(data[1], bits);
            assert_eq!(data[2], original[15] >> 8);
            for (i, &wanted) in original.iter().enumerate() {
                let n = 32 / bits as usize;
                let word = data[3 + i / n] as u32;
                let value = if bits == 32 {
                    word as i32
                } else {
                    let v = (word >> (bits as usize * (i % n))) & ((1 << bits) - 1);
                    (((v >> 3) as i32 + data[0]) << 8) | (v as i32 & 7)
                };
                assert_eq!(value, wanted);
            }
        }
    }

    #[test]
    fn block_words_preserve_columns_across_mask_groups() {
        let side = 32;
        let full: Vec<_> = (0..side * side)
            .map(|index| {
                let x = index % side;
                let z = index / side;
                let block = x / 4 + z / 4 * (side / 4);
                let grade = if block % 11 < 2 { 64 } else { 3 };
                let height = block as i32 * 50 - 2121 - (x % 4 + z % 4) as i32 * grade;
                (height << 8) | (index % 5) as i32
            })
            .collect();
        let regular = pack_page(&full, side);
        let packed = pack_blocks(&full, &regular, side).unwrap();
        let blocks = (side / 4).pow(2);
        let data = 2 + blocks + blocks.div_ceil(32) * 2;
        assert!(packed.len() < regular.len());
        for (index, &column) in full.iter().enumerate() {
            let x = index % side;
            let z = index / side;
            let block = x / 4 + z / 4 * (side / 4);
            let group = 2 + blocks + 2 * (block / 32);
            let lane = block % 32;
            let mask = packed[group + 1] as u32;
            let preceding_wide = (mask & ((1u32 << lane) - 1)).count_ones() as usize;
            let width = 8 << ((mask >> lane) & 1);
            let row = data + packed[group] as usize + (lane + preceding_wide) * 4;
            let n = 32 / width;
            let local = z % 4 * 4 + x % 4;
            let word =
                (packed[row + local / n] as u32 >> (width * (local % n))) & ((1 << width) - 1);
            let height = packed[2 + block] - (word >> 3) as i32;
            assert_eq!((height << 8) | (word & 7) as i32, column);
        }
    }
}
