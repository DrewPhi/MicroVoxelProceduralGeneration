//! Exact occupied-cell collision against canonical world queries. Unknown and
//! water are blocking. No renderer geometry, material properties or fluid solver.
use crate::{character::VoxelFrame, generation::Generator, CellPos, World};
use std::collections::BTreeSet;
#[derive(Clone, Debug)]
struct ColumnMask {
    x: i32,
    z: i32,
    ranges: Vec<[i32; 2]>,
}
fn columns(cells: &[[i32; 3]]) -> Vec<ColumnMask> {
    let mut map = std::collections::BTreeMap::<(i32, i32), Vec<i32>>::new();
    for &[x, y, z] in cells {
        map.entry((x, z)).or_default().push(y);
    }
    map.into_iter()
        .map(|((x, z), mut ys)| {
            ys.sort_unstable();
            ys.dedup();
            let mut ranges = Vec::<[i32; 2]>::new();
            for y in ys {
                if let Some(r) = ranges.last_mut() {
                    if r[1] == y {
                        r[1] = y + 1;
                        continue;
                    }
                }
                ranges.push([y, y + 1]);
            }
            ColumnMask { x, z, ranges }
        })
        .collect()
}
fn columns_clear<G: Generator>(mask: &[ColumnMask], world: &World<G>, root: CellPos) -> bool {
    mask.iter().all(|c| {
        c.ranges.iter().all(|r| {
            world
                .column_runs(
                    root.x + i64::from(c.x),
                    root.z + i64::from(c.z),
                    (root.y + i64::from(r[0]))..(root.y + i64::from(r[1])),
                )
                .is_some_and(|runs| runs.iter().all(|r| r.material == crate::MaterialId::AIR))
        })
    })
}
#[derive(Clone, Debug)]
pub struct VoxelShape {
    cells: Vec<[i32; 3]>,
    columns: Vec<ColumnMask>,
    leading_columns: [Vec<ColumnMask>; 6],
    pub min: [i32; 3],
    pub max: [i32; 3],
}
impl VoxelShape {
    pub fn memory_bytes(&self) -> usize {
        let bytes = |mask: &Vec<ColumnMask>| {
            mask.capacity() * std::mem::size_of::<ColumnMask>()
                + mask.iter().map(|c| c.ranges.capacity() * 8).sum::<usize>()
        };
        self.cells.capacity() * 12
            + bytes(&self.columns)
            + self.leading_columns.iter().map(bytes).sum::<usize>()
    }
    pub fn cells(&self) -> &[[i32; 3]] {
        &self.cells
    }
    pub fn from_frame(frame: &VoxelFrame) -> Self {
        Self::new(frame.occupied().map(|(p, _)| p).collect())
    }
    pub fn new(cells: Vec<[i32; 3]>) -> Self {
        let set: BTreeSet<_> = cells.into_iter().collect();
        assert!(!set.is_empty(), "collision shape must contain cells");
        let cells: Vec<_> = set.iter().copied().collect();
        let min = std::array::from_fn(|a| cells.iter().map(|p| p[a]).min().unwrap());
        let max = std::array::from_fn(|a| cells.iter().map(|p| p[a] + 1).max().unwrap());
        let leading: [Vec<[i32; 3]>; 6] = std::array::from_fn(|face| {
            let axis = face / 2;
            let sign = if face % 2 == 0 { -1 } else { 1 };
            cells
                .iter()
                .copied()
                .filter(|p| {
                    let mut n = *p;
                    n[axis] += sign;
                    !set.contains(&n)
                })
                .collect()
        });
        Self {
            columns: columns(&cells),
            leading_columns: std::array::from_fn(|i| columns(&leading[i])),
            cells,
            min,
            max,
        }
    }
    pub fn clear<G: Generator>(&self, world: &World<G>, root: CellPos) -> bool {
        columns_clear(&self.columns, world, root)
    }
    /// One crossed boundary at a time. Since the current pose is clear, only
    /// exposed cells facing this translation can encounter a new obstruction.
    pub fn move_one<G: Generator>(
        &self,
        world: &World<G>,
        root: CellPos,
        axis: usize,
        sign: i64,
    ) -> Option<CellPos> {
        let mut step = [0; 3];
        step[axis] = sign;
        let next = root.offset(step)?;
        let face = axis * 2 + usize::from(sign > 0);
        columns_clear(&self.leading_columns[face], world, next).then_some(next)
    }
    pub fn sweep_axis<G: Generator>(
        &self,
        world: &World<G>,
        root: CellPos,
        axis: usize,
        delta: i64,
    ) -> CellPos {
        if axis > 2 || delta.unsigned_abs() > 2048 {
            return root;
        }
        let mut position = root;
        for _ in 0..delta.unsigned_abs() {
            match self.move_one(world, position, axis, delta.signum()) {
                Some(p) => position = p,
                None => break,
            }
        }
        position
    }
    /// Conservative swept clearance for a discrete pose/facing change. The
    /// union's row bounds include the space between the two silhouettes. This
    /// may hold a pose in tight spaces; it never changes the exact tick hitbox.
    pub fn transition_clear<G: Generator>(
        &self,
        next: &Self,
        world: &World<G>,
        root: CellPos,
    ) -> bool {
        if !next.clear(world, root) {
            return false;
        }
        // Only changed occupancy needs a swept envelope. Unchanged legs/head
        // must not prevent an arm-only change because of a rock between feet.
        if self.cells == next.cells {
            return true;
        }
        let old_set: BTreeSet<_> = self.cells.iter().copied().collect();
        let new_set: BTreeSet<_> = next.cells.iter().copied().collect();
        let changed: Vec<_> = old_set.symmetric_difference(&new_set).copied().collect();
        let low = self.min[1].min(next.min[1]);
        let high = self.max[1].max(next.max[1]);
        let mut rows = vec![[i32::MAX, i32::MIN, i32::MAX, i32::MIN]; (high - low) as usize];
        for p in &changed {
            let row = &mut rows[(p[1] - low) as usize];
            row[0] = row[0].min(p[0]);
            row[1] = row[1].max(p[0]);
            row[2] = row[2].min(p[2]);
            row[3] = row[3].max(p[2]);
        }
        for (y, [x0, x1, z0, z1]) in rows.into_iter().enumerate() {
            if x0 == i32::MAX {
                continue;
            }
            for z in z0..=z1 {
                for x in x0..=x1 {
                    if !air(
                        world,
                        root,
                        [i64::from(x), i64::from(low) + y as i64, i64::from(z)],
                    ) {
                        return false;
                    }
                }
            }
        }
        true
    }
}
fn air<G: Generator>(world: &World<G>, root: CellPos, offset: [i64; 3]) -> bool {
    root.offset(offset)
        .and_then(|p| world.voxel_at(p).known())
        .is_some_and(|v| v.is_air())
}
/// Sweep a small camera volume along a grid ray, checking diagonal corner
/// neighbors too. It retracts along the boom rather than sliding around walls.
pub fn camera_boom<G: Generator>(
    world: &World<G>,
    start: CellPos,
    delta: [i64; 3],
    radius: i32,
) -> CellPos {
    if delta.iter().any(|v| v.unsigned_abs() > 256) || !(1..=32).contains(&radius) {
        return start;
    }
    let mut cells = Vec::new();
    for z in -radius..radius {
        for y in -radius..radius {
            for x in -radius..radius {
                cells.push([x, y, z]);
            }
        }
    }
    let shape = VoxelShape::new(cells);
    if !shape.clear(world, start) {
        return start;
    }
    let count = delta.iter().map(|v| v.abs()).max().unwrap();
    let mut result = start;
    for i in 1..=count {
        let offset = delta.map(|v| (v as f64 * i as f64 / count as f64).round() as i64);
        let Some(next) = start.offset(offset) else {
            break;
        };
        let change = [next.x - result.x, next.y - result.y, next.z - result.z];
        for mask in 1..8 {
            let step = std::array::from_fn(|a| if mask & (1 << a) != 0 { change[a] } else { 0 });
            if let Some(candidate) = result.offset(step) {
                if !shape.clear(world, candidate) {
                    return result;
                }
            } else {
                return result;
            }
        }
        result = next;
    }
    result
}
