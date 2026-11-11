//! Fixed-window two-level sparse root table. Uniform pages need one entry;
//! mixed pages retain their original root words and distance proofs.
#[cfg(test)]
use super::data::Snapshot;

const CHILD: u32 = 1 << 31;
const FULL: u32 = 1 << 30;
const PAYLOAD: u32 = FULL - 1;
const PAGE_SIDE: usize = 4;

struct Sparse {
    #[cfg(test)]
    side: usize,
    #[cfg(test)]
    shape: [usize; 3],
    table: Vec<u32>,
    pages: Vec<u32>,
}

fn index([x, y, z]: [usize; 3], [nx, ny, _]: [usize; 3]) -> usize {
    (z * nx + x) * ny + y
}

fn classification(word: u32) -> u32 {
    if word & (CHILD | FULL) == 0 {
        0
    } else {
        word
    }
}

impl Sparse {
    #[cfg(test)]
    fn new(dense: &[u32], dims: [usize; 3], side: usize) -> Self {
        Self::build(dense, dims, side, usize::MAX, || false).unwrap()
    }

    fn build(
        dense: &[u32],
        dims: [usize; 3],
        side: usize,
        max_words: usize,
        cancel: impl Fn() -> bool,
    ) -> Option<Self> {
        let shape = dims.map(|d| d.div_ceil(side));
        let table_len: usize = shape.iter().product();
        if table_len >= max_words {
            return None;
        }
        let mut table = Vec::with_capacity(table_len);
        let mut pages = Vec::new();
        let mut page = Vec::with_capacity(side.pow(3));
        for z in 0..shape[2] {
            if cancel() {
                return None;
            }
            for x in 0..shape[0] {
                for y in 0..shape[1] {
                    page.clear();
                    for dz in 0..side {
                        for dx in 0..side {
                            for dy in 0..side {
                                let p = [x * side + dx, y * side + dy, z * side + dz];
                                page.push(if (0..3).all(|a| p[a] < dims[a]) {
                                    dense[index(p, dims)]
                                } else {
                                    0
                                });
                            }
                        }
                    }
                    let first = classification(page[0]);
                    if first & CHILD == 0 && page.iter().all(|&word| classification(word) == first)
                    {
                        table.push(first);
                    } else {
                        if table_len + pages.len() + page.len() >= max_words {
                            return None;
                        }
                        let offset = u32::try_from(pages.len()).ok()?;
                        if offset > PAYLOAD {
                            return None;
                        }
                        table.push(CHILD | offset);
                        pages.extend_from_slice(&page);
                    }
                }
            }
        }
        Some(Self {
            #[cfg(test)]
            side,
            #[cfg(test)]
            shape,
            table,
            pages,
        })
    }

    #[cfg(test)]
    fn get(&self, cell: [usize; 3]) -> u32 {
        let page = self.table[index(cell.map(|p| p / self.side), self.shape)];
        if page & CHILD == 0 {
            page
        } else {
            let local = index(cell.map(|p| p % self.side), [self.side; 3]);
            self.pages[(page & PAYLOAD) as usize + local]
        }
    }

    #[cfg(test)]
    fn verify(&self, dense: &[u32], dims: [usize; 3]) {
        for z in 0..dims[2] {
            for x in 0..dims[0] {
                for y in 0..dims[1] {
                    let p = [x, y, z];
                    let word = dense[index(p, dims)];
                    let actual = self.get(p);
                    assert!(actual == word || actual == 0 && classification(word) == 0);
                }
            }
        }
    }
}

/// Pack the table and mixed pages into a texture with no dimension larger
/// than the original. Preserve the dense texture if padding erases savings.
pub(super) fn atlas(
    dense: &[u32],
    n: u32,
    cancel: impl Fn() -> bool,
) -> Option<(Vec<u32>, [u32; 3])> {
    let n = n as usize;
    let Sparse {
        mut table, pages, ..
    } = Sparse::build(dense, [n; 3], PAGE_SIDE, dense.len(), cancel)?;
    let len = table.len() + pages.len();
    let width = n.min(len);
    let height = n.min(len.div_ceil(width));
    let depth = len.div_ceil(width * height);
    let texels = width * height * depth;
    if texels >= dense.len() {
        return None;
    }
    table.reserve_exact(texels - table.len());
    table.extend(pages);
    table.resize(texels, 0);
    Some((table, [width as u32, height as u32, depth as u32]))
}

#[test]
fn atlas_offsets_decode_original_chunks_and_fall_back_if_not_smaller() {
    let n = 8;
    let mut dense = vec![0; n * n * n];
    dense[index([3, 6, 4], [n; 3])] = CHILD | 128;
    let (words, dims) = atlas(&dense, n as u32, || false).unwrap();
    assert_eq!(
        words.len(),
        dims.map(|d| d as usize).iter().product::<usize>()
    );
    let table_side = n.div_ceil(PAGE_SIDE);
    let table_len = table_side.pow(3);
    for z in 0..n {
        for x in 0..n {
            for y in 0..n {
                let cp = [x, y, z];
                let ptr = words[index(cp.map(|v| v / PAGE_SIDE), [table_side; 3])];
                let word = if ptr & CHILD == 0 {
                    ptr
                } else {
                    words[table_len
                        + (ptr & PAYLOAD) as usize
                        + index(cp.map(|v| v % PAGE_SIDE), [PAGE_SIDE; 3])]
                };
                assert_eq!(word, dense[index(cp, [n; 3])]);
            }
        }
    }
    dense.fill(CHILD);
    assert!(atlas(&dense, n as u32, || false).is_none());
}

#[test]
fn sparse_root_pages_preserve_occupancy_pigment_child_and_partial_edges() {
    let dims = [7, 8, 9];
    let mut dense = vec![0xff; dims.iter().product()];
    for x in 0..4 {
        for y in 0..4 {
            for z in 0..4 {
                dense[index([x, y, z], dims)] = FULL | 3;
            }
        }
    }
    dense[index([6, 7, 8], dims)] = CHILD | 128;
    let sparse = Sparse::new(&dense, dims, 4);
    assert_eq!(sparse.pages.len(), 64);
    assert_eq!(sparse.get([2, 2, 2]), FULL | 3);
    sparse.verify(&dense, dims);
}

#[test]
#[ignore = "explicit CPU corpus: measures two-level sparse root storage for matched Alpine data"]
fn alpine_sparse_root_corpus() {
    use frontier_world::{FbmParams, FbmRecipe, World};
    use std::sync::atomic::AtomicU32;
    let world = World::fbm(
        7,
        FbmParams {
            recipe: FbmRecipe::Alpine,
            ..Default::default()
        },
    )
    .unwrap()
    .with_vegetation();
    let eye = [-41., world.height_m(-41., -250.) + 1.7, -250.];
    let snapshot: Snapshot =
        super::data::build(&world, eye, 16, 512 * 1048576, &AtomicU32::new(0), || false).unwrap();
    let dims = [snapshot.chunks_per_edge as usize; 3];
    for side in [2, 4, 8, 16] {
        let sparse = Sparse::new(&snapshot.chunks, dims, side);
        sparse.verify(&snapshot.chunks, dims);
        eprintln!("NAADF seed-7 Alpine radius 16 m, root side {side}: {} dense bytes, {} table bytes, {} mixed page bytes, {} empty/{} full/{} allocated pages",
            snapshot.chunks.len() * 4, sparse.table.len() * 4, sparse.pages.len() * 4,
            sparse.table.iter().filter(|&&w| w == 0).count(),
            sparse.table.iter().filter(|&&w| w & FULL != 0).count(),
            sparse.pages.len() / side.pow(3));
    }
}
