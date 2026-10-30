//! A bounded local voxel volume. Cells are ordinary four-byte world values.
use crate::{
    materials::{MaterialRegistry, Voxel},
    painting,
    render::surface::{FaceCollector, Surface, NORMALS},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoxelFrame {
    pub cells_per_meter: u16,
    pub origin: [i32; 3],
    pub size: [usize; 3],
    pub(super) voxels: Vec<Voxel>,
    pub(super) right_arm_cells: Vec<usize>,
}
impl VoxelFrame {
    pub fn voxel(&self, p: [i32; 3]) -> Voxel {
        self.index(p).map_or(Voxel::AIR, |i| self.voxels[i])
    }
    pub(super) fn index(&self, p: [i32; 3]) -> Option<usize> {
        let q: [i64; 3] = std::array::from_fn(|a| i64::from(p[a]) - i64::from(self.origin[a]));
        if (0..3).any(|a| q[a] < 0 || q[a] >= self.size[a] as i64) {
            return None;
        }
        Some((q[2] as usize * self.size[1] + q[1] as usize) * self.size[0] + q[0] as usize)
    }
    pub fn occupied(&self) -> impl Iterator<Item = ([i32; 3], Voxel)> + '_ {
        self.voxels
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_air())
            .map(|(i, &v)| {
                let p = [
                    i % self.size[0],
                    (i / self.size[0]) % self.size[1],
                    i / (self.size[0] * self.size[1]),
                ];
                (std::array::from_fn(|a| self.origin[a] + p[a] as i32), v)
            })
    }
    pub fn memory_bytes(&self) -> usize {
        self.voxels.capacity() * std::mem::size_of::<Voxel>()
            + self.right_arm_cells.capacity() * std::mem::size_of::<usize>()
    }
    /// Actual right-arm cells, for first-person visibility; no separate fake arm.
    pub fn right_arm(&self, _heading: u8) -> Self {
        let mut result = self.clone();
        result.voxels.fill(Voxel::AIR);
        for &i in &self.right_arm_cells {
            result.voxels[i] = self.voxels[i];
        }
        result
    }
    pub fn solid_count(&self) -> usize {
        self.occupied().count()
    }
    /// Exposed faces only, with the existing color-preserving greedy merger.
    /// Translation is integer cells; no rotating cubes or interpolated skinning.
    pub fn surface(
        &self,
        materials: &MaterialRegistry,
        offset: [i32; 3],
    ) -> Result<Surface, String> {
        let mut collector = FaceCollector::default();
        for (p, voxel) in self.occupied() {
            let color = painting::base_color(voxel, materials);
            for (face, n) in NORMALS.iter().enumerate() {
                let neighbor = std::array::from_fn(|a| p[a] + n[a]);
                if self.voxel(neighbor).is_air() {
                    let position =
                        std::array::from_fn(|a| i64::from(p[a] + offset[a] + n[a].max(0)));
                    collector.add(face as u32, position, color)?;
                }
            }
        }
        Ok(collector.finish([0; 3], 1))
    }
}
