//! Greedy coplanar face merging with an independent, nearest-cell color atlas.
use std::collections::BTreeMap;

/// U × V points outward. All coordinates and extents are in integer grid units.
pub const AXES: [[usize; 3]; 6] = [
    [1, 2, 0], // +Y: plane Y, U Z, V X
    [1, 0, 2], // -Y
    [0, 2, 1], // -X
    [0, 1, 2], // +X
    [2, 1, 0], // -Z
    [2, 0, 1], // +Z
];
pub const NORMALS: [[i32; 3]; 6] = [
    [0, 1, 0],
    [0, -1, 0],
    [-1, 0, 0],
    [1, 0, 0],
    [0, 0, -1],
    [0, 0, 1],
];
/// One GPU instance, regardless of how many cell faces it covers. The atlas is
/// indexed by floor(U,V), so interpolation never blends neighboring cell colors.
#[derive(Clone, Debug)]
pub struct Quad {
    /// One atlas value covers this rectangle (used for thin proxy closure faces).
    pub constant_color: bool,
    pub origin: [i32; 3],
    pub face: u32,
    pub size: [u32; 2],
    pub color_offset: u32,
    /// Native-cell water plane, zero for ordinary solid geometry.
    pub water_level: u32,
    /// Atlas alpha packs filtered slope illumination and near AO.
    pub slope_light: bool,
}
#[derive(Clone, Debug, Default)]
pub struct Surface {
    pub quads: Vec<Quad>,
    pub colors: Vec<u32>,
    pub unit_faces: usize,
    pub min_y: i64,
    pub max_y: i64,
}
impl Surface {
    /// Coarse columns retain native Y, but flat equal-color tops still merge.
    /// No color array is expanded to the rectangle's native-cell area.
    pub fn merge_constant_tops(&mut self) {
        let old = std::mem::take(&mut self.quads);
        let colors = std::mem::take(&mut self.colors);
        let (mut tops, mut sides) = (Vec::new(), Vec::new());
        for q in old {
            let color = colors[q.color_offset as usize];
            if q.face < 2 && q.constant_color {
                tops.push((q, color));
            } else {
                sides.push((q, color));
            }
        }
        tops.sort_by_key(|(q, _)| (q.origin[1], q.face, q.origin[2], q.origin[0]));
        let mut rows: Vec<(Quad, u32)> = Vec::new();
        for (q, color) in tops {
            let ax = if q.face == 0 { 1 } else { 0 };
            let az = 1 - ax;
            if let Some((last, c)) = rows.last_mut() {
                if *c == color
                    && last.face == q.face
                    && last.water_level == q.water_level
                    && last.slope_light == q.slope_light
                    && last.origin[1] == q.origin[1]
                    && last.origin[2] == q.origin[2]
                    && last.size[az] == q.size[az]
                    && last.origin[0] as i64 + last.size[ax] as i64 == q.origin[0] as i64
                {
                    last.size[ax] += q.size[ax];
                    continue;
                }
            }
            rows.push((q, color));
        }
        let mut output: Vec<(Quad, u32)> = Vec::new();
        let mut active = BTreeMap::<_, usize>::new();
        for (q, color) in rows {
            let ax = if q.face == 0 { 1 } else { 0 };
            let az = 1 - ax;
            let key = (
                q.origin[1],
                q.face,
                q.origin[0],
                q.size[ax],
                color,
                q.water_level,
                q.slope_light,
            );
            if let Some(&index) = active.get(&key) {
                let last: &mut Quad = &mut output[index].0;
                if last.origin[2] as i64 + last.size[az] as i64 == q.origin[2] as i64 {
                    last.size[az] += q.size[az];
                    continue;
                }
            }
            active.insert(key, output.len());
            output.push((q, color));
        }
        output.extend(sides);
        for (mut q, color) in output {
            let old_offset = q.color_offset as usize;
            q.color_offset = self.colors.len() as u32;
            if q.constant_color {
                self.colors.push(color);
            } else {
                let count = q.size[0] as usize * q.size[1] as usize;
                self.colors
                    .extend_from_slice(&colors[old_offset..old_offset + count]);
            }
            self.quads.push(q);
        }
    }
    pub fn bytes(&self) -> usize {
        self.quads.len() * 32 + self.colors.len() * 4
    }
}
// The native collector uses bounded packed face pages; coarse top merging above
// remains independent and does not expand its rectangles into native cells.
#[path = "bitmask.rs"]
mod bitmask;
pub use bitmask::{FaceCollector, MesherScratch};
