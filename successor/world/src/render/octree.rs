//! Experimental binary-occupancy octrees and immutable DAG snapshots.
//! Geometry is independent of color/run attributes and of the editable world.
use super::ray::{Brick, LEAF};
use std::{collections::HashMap, sync::Arc};
pub const EMPTY: u32 = 0;
pub const SOLID: u32 = 1;
#[derive(Clone, Debug)]
pub struct Occupancy {
    pub columns: Vec<Vec<[u32; 2]>>,
    pub max_y: u32,
}
impl Occupancy {
    pub fn from_brick(brick: &Brick) -> Result<Self, String> {
        let mut columns = vec![Vec::<[u32; 2]>::new(); 1024];
        let mut max_y = 0;
        for node in brick.words[..brick.nodes * 8].chunks_exact(8) {
            if node[3] & LEAF == 0 {
                continue;
            }
            let x = node[0] as i32 - brick.bounds[0][0];
            let z = node[2] as i32 - brick.bounds[0][2];
            if !(0..32).contains(&x)
                || !(0..32).contains(&z)
                || node[4] - node[0] != 1
                || node[6] - node[2] != 1
            {
                return Err("octree experiment requires native 32x32 columns".into());
            }
            let column = &mut columns[(z * 32 + x) as usize];
            for i in 0..node[7] {
                let p = (node[3] & !LEAF) as usize + i as usize * 8;
                let low = brick.words[p];
                let high = brick.words[p + 1];
                max_y = max_y.max(high);
                if let Some(last) = column.last_mut() {
                    if last[1] == low {
                        last[1] = high;
                        continue;
                    }
                }
                column.push([low, high]);
            }
        }
        Ok(Self { columns, max_y })
    }
    fn classify(&self, p: [u32; 3], size: u32) -> Option<u32> {
        if p[0] >= 32 || p[2] >= 32 || p[1] >= self.max_y {
            return Some(EMPTY);
        }
        let mut full = p[0] + size <= 32 && p[2] + size <= 32;
        let mut empty = true;
        for z in p[2]..(p[2] + size).min(32) {
            for x in p[0]..(p[0] + size).min(32) {
                let column = &self.columns[(z * 32 + x) as usize];
                let first = column.partition_point(|r| r[1] <= p[1]);
                let r = column.get(first);
                let overlaps = r.is_some_and(|r| r[0] < p[1] + size);
                empty &= !overlaps;
                full &= r.is_some_and(|r| r[0] <= p[1] && r[1] >= p[1] + size);
                if !full && !empty {
                    return None;
                }
            }
        }
        if full {
            Some(SOLID)
        } else if empty {
            Some(EMPTY)
        } else {
            None
        }
    }
    pub fn occupied(&self, p: [u32; 3]) -> bool {
        if p[0] >= 32 || p[2] >= 32 {
            return false;
        }
        self.columns[(p[2] * 32 + p[0]) as usize]
            .iter()
            .any(|r| r[0] <= p[1] && p[1] < r[1])
    }
}
#[derive(Clone, Debug)]
pub struct Tree {
    pub nodes: Vec<[u32; 8]>,
    pub root: u32,
    pub height: u32,
}
impl Tree {
    pub fn build(occupancy: &Occupancy) -> Self {
        fn visit(p: [u32; 3], size: u32, occ: &Occupancy, nodes: &mut Vec<[u32; 8]>) -> u32 {
            if let Some(value) = occ.classify(p, size) {
                return value;
            }
            assert!(size > 1);
            let half = size / 2;
            let children = std::array::from_fn(|i| {
                visit(
                    [
                        p[0] + (i as u32 & 1) * half,
                        p[1] + ((i as u32 >> 1) & 1) * half,
                        p[2] + ((i as u32 >> 2) & 1) * half,
                    ],
                    half,
                    occ,
                    nodes,
                )
            });
            let index = nodes.len() as u32 + 2;
            nodes.push(children);
            index
        }
        let size = occupancy.max_y.max(32).next_power_of_two();
        let height = size.trailing_zeros();
        let mut nodes = Vec::new();
        let root = visit([0; 3], size, occupancy, &mut nodes);
        Self {
            nodes,
            root,
            height,
        }
    }
    pub fn compact(&self) -> Compact {
        Compact::new(&self.nodes, self.root, self.height)
    }
}
#[derive(Default)]
pub struct DagBuilder {
    pub nodes: Vec<[u32; 8]>,
    intern: HashMap<[u32; 8], u32>,
}
impl DagBuilder {
    /// Exact equality resolves hash collisions. Node IDs are canonical already
    /// because the source tree is postordered. Shared geometry is never edited.
    pub fn insert(&mut self, tree: &Tree) -> u32 {
        let mut mapping = vec![0, 1];
        for children in &tree.nodes {
            let key = children.map(|id| mapping[id as usize]);
            let id = if let Some(id) = self.intern.get(&key) {
                *id
            } else {
                let id = self.nodes.len() as u32 + 2;
                self.nodes.push(key);
                self.intern.insert(key, id);
                id
            };
            mapping.push(id);
        }
        mapping[tree.root as usize]
    }
    pub fn snapshot(&self, root: u32, height: u32) -> Arc<Compact> {
        Arc::new(Compact::new(&self.nodes, root, height))
    }
}
#[derive(Clone, Debug)]
pub struct Compact {
    pub words: Vec<u32>,
    pub root: u32,
    pub height: u32,
    pub nodes: usize,
    pub edges: usize,
}
impl Compact {
    fn new(nodes: &[[u32; 8]], root: u32, height: u32) -> Self {
        let mut words = vec![0; nodes.len() * 2];
        let mut edges = 0;
        for (i, children) in nodes.iter().enumerate() {
            let mask = children
                .iter()
                .enumerate()
                .fold(0u32, |m, (i, v)| m | ((u32::from(*v != EMPTY)) << i));
            words[i * 2] = mask;
            words[i * 2 + 1] = words.len() as u32;
            for &child in children {
                if child != EMPTY {
                    words.push(child);
                    edges += 1;
                }
            }
        }
        Self {
            words,
            root,
            height,
            nodes: nodes.len(),
            edges,
        }
    }
    pub fn bytes(&self) -> usize {
        self.words.len() * 4
    }
    pub fn occupied(&self, p: [u32; 3]) -> bool {
        if p.iter().any(|v| *v >= 1 << self.height) {
            return false;
        }
        let mut id = self.root;
        for bit in (0..self.height).rev() {
            if id < 2 {
                break;
            }
            let child =
                ((p[0] >> bit) & 1) | (((p[1] >> bit) & 1) << 1) | (((p[2] >> bit) & 1) << 2);
            let n = (id - 2) as usize * 2;
            let mask = self.words[n];
            if mask & (1 << child) == 0 {
                return false;
            }
            let index = (mask & ((1 << child) - 1)).count_ones();
            id = self.words[self.words[n + 1] as usize + index as usize];
        }
        id == SOLID
    }
}
/// Experimental hybrid: an edited region immediately uses ordinary column data.
/// Existing immutable snapshots remain usable by other region instances.
pub struct HybridBrick {
    pub columns: Arc<Brick>,
    pub frozen: Option<Arc<Compact>>,
}
impl HybridBrick {
    pub fn editable(columns: Arc<Brick>) -> Self {
        Self {
            columns,
            frozen: None,
        }
    }
    pub fn freeze(&mut self) -> Result<(), String> {
        let tree = Tree::build(&Occupancy::from_brick(&self.columns)?);
        let mut dag = DagBuilder::default();
        let root = dag.insert(&tree);
        self.frozen = Some(dag.snapshot(root, tree.height));
        Ok(())
    }
    pub fn replace_edited(&mut self, columns: Arc<Brick>) {
        self.columns = columns;
        self.frozen = None;
    }
}
/// The alternate geometry keeps all existing color/run/light attributes. A
/// small direct column table replaces the BVH leaf lookup for shading.
pub fn attributes(brick: &Brick) -> Vec<u32> {
    let base = brick.nodes * 8;
    let mut words = vec![0u32; 2048];
    words.extend_from_slice(&brick.words[base..]);
    for i in 0..brick.runs {
        let p = 2048 + i * 8;
        words[p + 3] = brick.words[base + i * 8 + 3] - base as u32 + 2048;
    }
    for n in brick.words[..base].chunks_exact(8) {
        if n[3] & LEAF != 0 {
            let x = n[0] as i32 - brick.bounds[0][0];
            let z = n[2] as i32 - brick.bounds[0][2];
            let i = (z * 32 + x) as usize * 2;
            words[i] = (n[3] & !LEAF) - base as u32 + 2048;
            words[i + 1] = n[7];
        }
    }
    words
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        generation::FlatTerrain,
        materials::Voxel,
        render::{
            lod::{MeshKey, TileKey},
            terrain::Sampler,
        },
        CellPos, Edit, World,
    };
    #[test]
    fn tree_and_shared_dag_preserve_occupancy_and_cavities() {
        let mut world = World::new(FlatTerrain {
            height: 73,
            water: 73,
            seed: 42,
        });
        world
            .apply(&[Edit::set(CellPos::new(40, 32, 40), Voxel::AIR)])
            .unwrap();
        let mut s = Sampler::new(Arc::new(world));
        let key = MeshKey {
            tile: TileKey {
                level: 13,
                x: 1,
                z: 1,
            },
            neighbors: [13; 4],
            revision: 1,
        };
        let brick = s.ray_brick(key, || false).unwrap();
        let occ = Occupancy::from_brick(&brick).unwrap();
        let tree = Tree::build(&occ);
        let svo = tree.compact();
        let mut pool = DagBuilder::default();
        let root = pool.insert(&tree);
        let count = pool.nodes.len();
        assert_eq!(pool.insert(&tree), root);
        assert_eq!(pool.nodes.len(), count);
        let dag = pool.snapshot(root, tree.height);
        for z in 0..32 {
            for x in 0..32 {
                for y in 0..128 {
                    let p = [x, y, z];
                    assert_eq!(occ.occupied(p), svo.occupied(p));
                    assert_eq!(svo.occupied(p), dag.occupied(p));
                }
            }
        }
        assert!(dag.bytes() < svo.bytes());
    }
    #[test]
    fn edited_region_detaches_without_mutating_shared_geometry() {
        let mut world = World::new(FlatTerrain {
            height: 64,
            water: 64,
            seed: 42,
        });
        let key = MeshKey {
            tile: TileKey {
                level: 13,
                x: 1,
                z: 1,
            },
            neighbors: [13; 4],
            revision: 0,
        };
        let brick = Sampler::new(Arc::new(World::new(FlatTerrain {
            height: 64,
            water: 64,
            seed: 42,
        })))
        .ray_brick(key, || false)
        .unwrap();
        let mut region = HybridBrick::editable(Arc::new(brick));
        region.freeze().unwrap();
        let old = region.frozen.clone().unwrap();
        world
            .apply(&[Edit::set(CellPos::new(40, 63, 40), Voxel::AIR)])
            .unwrap();
        let changed = Sampler::new(Arc::new(world))
            .ray_brick(MeshKey { revision: 1, ..key }, || false)
            .unwrap();
        region.replace_edited(Arc::new(changed));
        assert!(region.frozen.is_none());
        assert!(old.occupied([8, 63, 8]));
        region.freeze().unwrap();
        assert!(!region.frozen.as_ref().unwrap().occupied([8, 63, 8]));
        assert!(old.occupied([8, 63, 8]));
    }
}
