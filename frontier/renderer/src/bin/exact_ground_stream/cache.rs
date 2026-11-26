use bytemuck::{Pod, Zeroable};
use frontier_world::{
    affine_bounds::{bound_eroded_ground_ceiling, bound_native_height, BoundBudget, ColumnBox},
    FbmTerrain, Material,
};

pub const ROOT_LOG: u32 = 18;
pub const ROOT: u32 = 1 << ROOT_LOG;
pub const NATIVE_PITCH: f64 = 0.0625;
const PAGE_FLAG: u32 = 0x8000_0000;

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
}
impl Tile {
    fn child(self, quadrant: u32) -> Self {
        let width = self.width / 2;
        let x = i64::from((quadrant & 1) * width);
        let z = i64::from((quadrant >> 1) * width);
        Self {
            min: [self.min[0] + x, self.min[1] + z],
            width,
        }
    }
}
enum Build {
    Branch([f32; 4]),
    Page(Vec<i32>, f32),
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

type Job = (usize, Tile, f32);
type Completed = (usize, Tile, Build);

pub struct Workers {
    queues: Vec<std::sync::mpsc::Sender<Vec<Job>>>,
    results: std::sync::mpsc::Receiver<Vec<Completed>>,
    handles: Vec<std::thread::JoinHandle<()>>,
}
impl Workers {
    pub fn new(terrain: &FbmTerrain, count: usize, page_side: u32, land_bound: bool) -> Self {
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
                        .map(|(index, tile, parent)| {
                            let build = if tile.width == page_side {
                                fill_page(&terrain, tile)
                            } else {
                                Build::Branch(std::array::from_fn(|i| {
                                    ceiling(&terrain, tile.child(i as u32), parent, land_bound)
                                }))
                            };
                            (index, tile, build)
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
        jobs.sort_unstable_by_key(|(_, tile, _)| {
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
        completed.sort_unstable_by_key(|(index, _, _)| *index);
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

fn ceiling(terrain: &FbmTerrain, tile: Tile, parent: f32, land_bound: bool) -> f32 {
    let region = ColumnBox {
        min: tile.min,
        size_cells: [tile.width; 2],
    };
    let budget = BoundBudget {
        max_noise_cells: 512,
        max_cells_per_noise: 4,
    };
    let bound = land_bound
        .then(|| bound_eroded_ground_ceiling(terrain, region, budget))
        .flatten()
        .or_else(|| bound_native_height(terrain, region, budget).map(|b| b.height_range[1]));
    // Round outward again before evaluating the bound in f32.
    bound.map_or(parent, |height| {
        ((height as f32).next_up() + 0.003).min(parent)
    })
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
    pub page_jobs: usize,
}
pub struct Upload {
    pub first_word: u32,
    pub columns: Vec<i32>,
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
            page_jobs: 0,
        }
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
        requests: &[(u32, u32)],
        batch: usize,
        prefetch: bool,
    ) -> Upload {
        let mut jobs = Vec::with_capacity(batch.min(requests.len()));
        let mut reserved_nodes = self.nodes.len();
        let mut reserved_words = self.words;
        let page_words = 2 + self.page_side.pow(2) + (self.page_side / 4).pow(2);
        let mut predicted = Vec::new();
        if prefetch {
            for &(key, _) in requests {
                if self.tiles[key as usize].width == self.page_side {
                    let first = (key - 1) / 4 * 4 + 1;
                    predicted.extend(first..first + 4);
                }
            }
            predicted.sort_unstable();
            predicted.dedup();
        }
        let mut selected = vec![false; self.nodes.len()];
        for key in requests.iter().map(|&(key, _)| key).chain(predicted) {
            if jobs.len() == batch {
                break;
            }
            let index = key as usize;
            let node = self.nodes[index];
            if node.link != 0 || selected[index] {
                continue;
            }
            let tile = self.tiles[index];
            if tile.width == self.page_side {
                if reserved_words + page_words > self.word_limit {
                    continue;
                }
                reserved_words += page_words;
            } else {
                if reserved_nodes + 4 > self.node_limit {
                    continue;
                }
                reserved_nodes += 4;
            }
            selected[index] = true;
            jobs.push((index, tile, node.ceiling));
        }

        let first_word = self.words;
        let mut columns = Vec::with_capacity((reserved_words - first_word) as usize);
        if jobs.is_empty() {
            return Upload {
                columns,
                first_word,
                jobs: 0,
            };
        }
        let output = workers.build(&mut jobs);
        for (index, tile, result) in output {
            match result {
                Build::Branch(bounds) => {
                    self.nodes[index].link = self.nodes.len() as u32;
                    self.branch_jobs += 1;
                    for (quadrant, ceiling) in bounds.into_iter().enumerate() {
                        self.nodes.push(Node {
                            ceiling,
                            link: 0,
                            parent: index as u32,
                        });
                        self.tiles.push(tile.child(quadrant as u32));
                    }
                }
                Build::Page(data, ceiling) => {
                    assert!(
                        ceiling <= self.nodes[index].ceiling + 0.003,
                        "opaque ground exceeded its generation bound at {:?}",
                        tile.min,
                    );
                    self.nodes[index] = Node {
                        ceiling,
                        link: PAGE_FLAG | self.words,
                        parent: self.nodes[index].parent,
                    };
                    self.pages += 1;
                    self.words += data.len() as u32;
                    self.page_jobs += 1;
                    columns.extend(data);
                }
            }
        }
        // Resident native pages yield exact ceilings. Propagate any reduction
        // up the tree while retaining conservative bounds for pending siblings.
        for index in (0..self.nodes.len()).rev() {
            let node = self.nodes[index];
            if node.link > 0 && node.link < PAGE_FLAG {
                let at = node.link as usize;
                let ceiling = self.nodes[at..at + 4]
                    .iter()
                    .fold(f32::NEG_INFINITY, |h, child| h.max(child.ceiling));
                self.nodes[index].ceiling = node.ceiling.min(ceiling);
            }
        }
        Upload {
            columns,
            first_word,
            jobs: jobs.len(),
        }
    }
}

fn fill_page(terrain: &FbmTerrain, tile: Tile) -> Build {
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
    Build::Page(
        pack_page(&data, tile.width as usize),
        ceiling.next_up() + 0.003,
    )
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
}
