//! Bounded canonical occupancy evidence for arbitrary 3D content. Mixed nodes
//! never become solid/empty through majority voting, and uncertain topology is
//! never promoted to a heightfield by merging. This is a summary, not a proxy.
use crate::{Cell, Material, World};

const MAX_INTERIOR_CELLS: usize = 262_144;
const MAX_HALO_CELLS: usize = 400_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellBounds {
    pub min: Cell,
    pub max: Cell,
}
impl CellBounds {
    pub fn contains(self, c: Cell) -> bool {
        c.x >= self.min.x
            && c.y >= self.min.y
            && c.z >= self.min.z
            && c.x < self.max.x
            && c.y < self.max.y
            && c.z < self.max.z
    }
    fn union(self, other: Self) -> Self {
        Self {
            min: Cell {
                x: self.min.x.min(other.min.x),
                y: self.min.y.min(other.min.y),
                z: self.min.z.min(other.min.z),
            },
            max: Cell {
                x: self.max.x.max(other.max.x),
                y: self.max.y.max(other.max.y),
                z: self.max.z.max(other.max.z),
            },
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Occupancy {
    Empty,
    Full,
    Mixed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopologyEvidence {
    /// Proven from a canonical source contract, never inferred from a few samples.
    HeightfieldSafe,
    /// An air-to-solid vertical transition was found, or a child has such evidence.
    Volume,
    /// The bounded region alone cannot establish global heightfield ownership.
    Unknown,
}
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeSummary {
    /// Caller-assigned identity for an independently constructed world lineage.
    pub source_id: u64,
    pub revision: u64,
    pub region: CellBounds,
    pub occupancy: Occupancy,
    pub occupied_bounds: Option<CellBounds>,
    /// Air/Grass/Soil/Stone/Wood native cell counts. No majority interpretation.
    pub material_cells: [u64; 5],
    /// Canonical exposed native unit-face counts, +X/-X/+Y/-Y/+Z/-Z,
    /// then Air/Grass/Soil/Stone/Wood. Halo culls internal child borders.
    pub exposed_faces: [[u64; 5]; 6],
    pub topology: TopologyEvidence,
}
impl VolumeSummary {
    pub fn total_cells(&self) -> u64 {
        self.material_cells.iter().sum()
    }
    pub fn occupied_cells(&self) -> u64 {
        self.total_cells() - self.material_cells[0]
    }
    pub fn uniform_material(&self) -> Option<Material> {
        let n = self.total_cells();
        self.material_cells
            .iter()
            .position(|&count| count == n)
            .map(material)
    }
    /// Merge exactly eight equal, nonoverlapping octants of one source revision.
    /// Face counts stay canonical because extraction sampled the world halo.
    pub fn merge(children: &[Self; 8]) -> Result<Self, String> {
        let first = &children[0];
        if children
            .iter()
            .any(|c| c.source_id != first.source_id || c.revision != first.revision)
        {
            return Err("volume summary source/revision mismatch".into());
        }
        let region = children.iter().fold(first.region, |a, c| a.union(c.region));
        let dims = dimensions(first.region)?;
        let parent = dimensions(region)?;
        if (0..3).any(|a| parent[a] != dims[a] * 2) {
            return Err("volume summary children do not cover an octant parent".into());
        }
        let base = [region.min.x, region.min.y, region.min.z];
        let mut mask = 0u8;
        for child in children {
            if dimensions(child.region)? != dims {
                return Err("volume summary child sizes differ".into());
            }
            let p = [child.region.min.x, child.region.min.y, child.region.min.z];
            let mut index = 0;
            for a in 0..3 {
                let delta = p[a] - base[a];
                if delta == dims[a] as i64 {
                    index |= 1 << a
                } else if delta != 0 {
                    return Err("volume summary child is not an aligned octant".into());
                }
            }
            if mask & (1 << index) != 0 {
                return Err("duplicate volume summary child".into());
            }
            mask |= 1 << index;
        }
        let mut out = Self {
            source_id: first.source_id,
            revision: first.revision,
            region,
            occupancy: Occupancy::Mixed,
            occupied_bounds: None,
            material_cells: [0; 5],
            exposed_faces: [[0; 5]; 6],
            topology: TopologyEvidence::HeightfieldSafe,
        };
        for c in children {
            if let Some(bounds) = c.occupied_bounds {
                out.occupied_bounds = Some(out.occupied_bounds.map_or(bounds, |b| b.union(bounds)))
            }
            for m in 0..5 {
                out.material_cells[m] = out.material_cells[m]
                    .checked_add(c.material_cells[m])
                    .ok_or("volume material count overflow")?;
                for a in 0..6 {
                    out.exposed_faces[a][m] = out.exposed_faces[a][m]
                        .checked_add(c.exposed_faces[a][m])
                        .ok_or("volume face count overflow")?
                }
            }
            out.topology = match (out.topology, c.topology) {
                (TopologyEvidence::Volume, _) | (_, TopologyEvidence::Volume) => {
                    TopologyEvidence::Volume
                }
                (TopologyEvidence::Unknown, _) | (_, TopologyEvidence::Unknown) => {
                    TopologyEvidence::Unknown
                }
                _ => TopologyEvidence::HeightfieldSafe,
            };
        }
        out.occupancy = occupancy(out.material_cells);
        Ok(out)
    }
}
fn dimensions(b: CellBounds) -> Result<[usize; 3], String> {
    let lo = [b.min.x, b.min.y, b.min.z];
    let hi = [b.max.x, b.max.y, b.max.z];
    let mut dims = [0; 3];
    for a in 0..3 {
        dims[a] = usize::try_from(hi[a] as i128 - lo[a] as i128)
            .ok()
            .filter(|&n| n > 0)
            .ok_or("invalid volume summary region")?
    }
    Ok(dims)
}
fn material(index: usize) -> Material {
    [
        Material::Air,
        Material::Grass,
        Material::Soil,
        Material::Stone,
        Material::Wood,
    ][index]
}
fn material_index(m: Material) -> usize {
    match m {
        Material::Air => 0,
        Material::Grass => 1,
        Material::Soil => 2,
        Material::Stone => 3,
        Material::Wood => 4,
    }
}
fn occupancy(counts: [u64; 5]) -> Occupancy {
    if counts[1..].iter().all(|&n| n == 0) {
        Occupancy::Empty
    } else if counts[0] == 0 {
        Occupancy::Full
    } else {
        Occupancy::Mixed
    }
}

pub fn build(
    world: &World,
    source_id: u64,
    region: CellBounds,
    cancelled: impl FnMut() -> bool,
) -> Result<VolumeSummary, String> {
    let safe = world.fbm_terrain().is_some()
        && world.topology_base_cells().is_none()
        && world.revision() == 0;
    sample(
        source_id,
        world.revision(),
        region,
        safe,
        |c| world.material(c),
        cancelled,
    )
}
/// The caller may certify `heightfield_safe` only from the canonical source
/// contract. False is safe for every source and keeps uncertainty explicit.
pub fn sample(
    source_id: u64,
    revision: u64,
    region: CellBounds,
    heightfield_safe: bool,
    mut query: impl FnMut(Cell) -> Material,
    mut cancelled: impl FnMut() -> bool,
) -> Result<VolumeSummary, String> {
    let dims = dimensions(region)?;
    let count = dims
        .iter()
        .try_fold(1usize, |a, &b| a.checked_mul(b))
        .filter(|&n| n <= MAX_INTERIOR_CELLS)
        .ok_or("volume summary exceeds bounded interior work")?;
    let _ = count;
    let halo = dims
        .map(|n| n.checked_add(2).ok_or("volume halo overflow"))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let halo_count = halo
        .iter()
        .try_fold(1usize, |a, &b| a.checked_mul(b))
        .filter(|&n| n <= MAX_HALO_CELLS)
        .ok_or("volume summary exceeds bounded halo work")?;
    for v in [region.min.x, region.min.y, region.min.z] {
        v.checked_sub(1).ok_or("volume halo coordinate underflow")?;
    }
    for v in [region.max.x, region.max.y, region.max.z] {
        v.checked_add(1).ok_or("volume halo coordinate overflow")?;
    }
    let mut values = Vec::with_capacity(halo_count);
    for z in 0..halo[2] {
        for y in 0..halo[1] {
            if cancelled() {
                return Err("volume summary cancelled".into());
            }
            for x in 0..halo[0] {
                values.push(query(Cell {
                    x: region.min.x + x as i64 - 1,
                    y: region.min.y + y as i64 - 1,
                    z: region.min.z + z as i64 - 1,
                }));
            }
        }
    }
    let at = |x: usize, y: usize, z: usize| values[(z * halo[1] + y) * halo[0] + x];
    let mut out = VolumeSummary {
        source_id,
        revision,
        region,
        occupancy: Occupancy::Empty,
        occupied_bounds: None,
        material_cells: [0; 5],
        exposed_faces: [[0; 5]; 6],
        topology: if heightfield_safe {
            TopologyEvidence::HeightfieldSafe
        } else {
            TopologyEvidence::Unknown
        },
    };
    for z in 1..=dims[2] {
        if cancelled() {
            return Err("volume summary cancelled".into());
        }
        for x in 1..=dims[0] {
            let mut air_below = false;
            for y in 1..=dims[1] {
                let m = at(x, y, z);
                let mi = material_index(m);
                out.material_cells[mi] += 1;
                if m == Material::Air {
                    air_below = true;
                    continue;
                }
                if air_below {
                    out.topology = TopologyEvidence::Volume
                }
                let c = Cell {
                    x: region.min.x + x as i64 - 1,
                    y: region.min.y + y as i64 - 1,
                    z: region.min.z + z as i64 - 1,
                };
                let b = CellBounds {
                    min: c,
                    max: Cell {
                        x: c.x + 1,
                        y: c.y + 1,
                        z: c.z + 1,
                    },
                };
                out.occupied_bounds = Some(out.occupied_bounds.map_or(b, |old| old.union(b)));
                for (axis, n) in [
                    at(x + 1, y, z),
                    at(x - 1, y, z),
                    at(x, y + 1, z),
                    at(x, y - 1, z),
                    at(x, y, z + 1),
                    at(x, y, z - 1),
                ]
                .into_iter()
                .enumerate()
                {
                    if n == Material::Air {
                        out.exposed_faces[axis][mi] += 1
                    }
                }
            }
        }
    }
    out.occupancy = occupancy(out.material_cells);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bounds(min: [i64; 3], size: i64) -> CellBounds {
        CellBounds {
            min: Cell {
                x: min[0],
                y: min[1],
                z: min[2],
            },
            max: Cell {
                x: min[0] + size,
                y: min[1] + size,
                z: min[2] + size,
            },
        }
    }
    fn fixture(c: Cell) -> Material {
        // Thin bridge, separated pillars, and an open hollow volume.
        if c.y == 3 && (0..8).contains(&c.x) && c.z == 3
            || [0, 7].contains(&c.x) && c.y < 3 && c.z == 3
        {
            Material::Wood
        } else if (0..8).contains(&c.x)
            && (0..8).contains(&c.y)
            && (0..8).contains(&c.z)
            && (c.x == 0 || c.x == 7 || c.y == 0 || c.y == 7 || c.z == 7)
        {
            Material::Stone
        } else {
            Material::Air
        }
    }
    #[test]
    fn merge_preserves_thin_bridge_cave_faces_and_bounds() {
        let parent = sample(1, 8, bounds([0, 0, 0], 8), false, fixture, || false).unwrap();
        let children = std::array::from_fn(|i| {
            sample(
                1,
                8,
                bounds(
                    [
                        if i & 1 != 0 { 4 } else { 0 },
                        if i & 2 != 0 { 4 } else { 0 },
                        if i & 4 != 0 { 4 } else { 0 },
                    ],
                    4,
                ),
                false,
                fixture,
                || false,
            )
            .unwrap()
        });
        let merged = VolumeSummary::merge(&children).unwrap();
        assert_eq!(merged.occupancy, Occupancy::Mixed);
        assert_eq!(merged.material_cells, parent.material_cells);
        assert!(merged.material_cells[4] > 0);
        assert_eq!(merged.exposed_faces, parent.exposed_faces);
        assert_eq!(merged.occupied_bounds, parent.occupied_bounds);
        assert_eq!(merged.topology, TopologyEvidence::Volume);
    }
    #[test]
    fn no_homogenization_or_false_crop_faces() {
        let empty = sample(
            1,
            0,
            bounds([0, 0, 0], 4),
            false,
            |_| Material::Air,
            || false,
        )
        .unwrap();
        assert_eq!(empty.occupancy, Occupancy::Empty);
        assert_eq!(empty.occupied_bounds, None);
        let solid = sample(
            1,
            0,
            bounds([0, 0, 0], 4),
            false,
            |_| Material::Stone,
            || false,
        )
        .unwrap();
        assert_eq!(solid.occupancy, Occupancy::Full);
        assert_eq!(solid.uniform_material(), Some(Material::Stone));
        assert_eq!(solid.exposed_faces, [[0; 5]; 6]);
        assert_eq!(solid.topology, TopologyEvidence::Unknown);
        let single = sample(
            1,
            0,
            bounds([0, 0, 0], 4),
            false,
            |c| {
                if c == (Cell { x: 1, y: 1, z: 1 }) {
                    Material::Wood
                } else {
                    Material::Air
                }
            },
            || false,
        )
        .unwrap();
        assert_eq!(single.occupancy, Occupancy::Mixed);
        assert_eq!(single.occupied_cells(), 1);
        assert_eq!(single.exposed_faces.iter().map(|a| a[4]).sum::<u64>(), 6);
        assert_eq!(single.topology, TopologyEvidence::Volume);
    }
    #[test]
    fn boundary_corner_edits_and_thin_tower_survive_octant_merge() {
        // Every combination touches an octant face, edge, or corner. A thin
        // tower crosses the Y split; one-cell gaps must stay air at all levels.
        use std::collections::HashSet;
        let mut cells = HashSet::new();
        for x in [-4, -1, 0, 3] {
            for y in [-4, -1, 0, 3] {
                for z in [-4, -1, 0, 3] {
                    cells.insert(Cell { x, y, z });
                }
            }
        }
        for y in -4..4 {
            cells.insert(Cell { x: 1, y, z: 1 });
        }
        let query = |c: Cell| {
            if cells.contains(&c) {
                Material::Wood
            } else {
                Material::Air
            }
        };
        let children = std::array::from_fn(|i| {
            sample(
                31,
                7,
                bounds(
                    [
                        if i & 1 == 0 { -4 } else { 0 },
                        if i & 2 == 0 { -4 } else { 0 },
                        if i & 4 == 0 { -4 } else { 0 },
                    ],
                    4,
                ),
                false,
                query,
                || false,
            )
            .unwrap()
        });
        let merged = VolumeSummary::merge(&children).unwrap();
        assert_eq!(merged.occupancy, Occupancy::Mixed);
        assert_eq!(merged.occupied_cells(), cells.len() as u64);
        // Independent combinatorial oracle: each shared face removes two
        // outward faces from the six contributed by every occupied voxel.
        let shared_pairs: usize = cells
            .iter()
            .map(|c| {
                [
                    Cell { x: c.x + 1, ..*c },
                    Cell { y: c.y + 1, ..*c },
                    Cell { z: c.z + 1, ..*c },
                ]
                .iter()
                .filter(|neighbor| cells.contains(neighbor))
                .count()
            })
            .sum();
        let exposed = 6 * cells.len() - 2 * shared_pairs;
        assert_eq!(
            merged.exposed_faces.iter().map(|face| face[4]).sum::<u64>(),
            exposed as u64
        );
        assert_eq!(merged.occupied_bounds, Some(bounds([-4; 3], 8)));
        assert_eq!(merged.topology, TopologyEvidence::Volume);
        let mut edited = children;
        edited[0].revision += 1;
        assert!(
            VolumeSummary::merge(&edited).is_err(),
            "an edited child invalidates its parent"
        );
    }
    #[test]
    fn only_source_contract_certifies_heightfield_safety() {
        let world = World::fbm(7, crate::FbmParams::default()).unwrap();
        let summary = build(&world, 4, bounds([0, 0, 0], 4), || false).unwrap();
        assert_eq!(summary.topology, TopologyEvidence::HeightfieldSafe);
        let children = std::array::from_fn(|i| {
            sample(
                2,
                9,
                bounds(
                    [
                        if i & 1 != 0 { 2 } else { 0 },
                        if i & 2 != 0 { 2 } else { 0 },
                        if i & 4 != 0 { 2 } else { 0 },
                    ],
                    2,
                ),
                false,
                |_| Material::Stone,
                || false,
            )
            .unwrap()
        });
        assert_eq!(
            VolumeSummary::merge(&children).unwrap().topology,
            TopologyEvidence::Unknown
        );
    }
    #[test]
    fn rejects_stale_incomplete_duplicate_and_unbounded_work() {
        let children = std::array::from_fn(|i| {
            sample(
                2,
                9,
                bounds(
                    [
                        if i & 1 != 0 { 2 } else { 0 },
                        if i & 2 != 0 { 2 } else { 0 },
                        if i & 4 != 0 { 2 } else { 0 },
                    ],
                    2,
                ),
                false,
                |_| Material::Air,
                || false,
            )
            .unwrap()
        });
        let mut stale = children.clone();
        stale[0].revision = 10;
        assert!(VolumeSummary::merge(&stale).is_err());
        let mut duplicate = children.clone();
        duplicate[1] = duplicate[0].clone();
        assert!(VolumeSummary::merge(&duplicate).is_err());
        assert!(sample(
            1,
            0,
            bounds([0, 0, 0], 1024),
            false,
            |_| Material::Air,
            || false
        )
        .is_err());
        assert!(sample(
            1,
            0,
            bounds([0, 0, 0], 4),
            false,
            |_| Material::Air,
            || true
        )
        .is_err());
    }
}
