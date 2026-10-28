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
    pub origin: [i32; 3],
    pub face: u32,
    pub size: [u32; 2],
    pub color_offset: u32,
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
    pub fn bytes(&self) -> usize {
        self.quads.len() * 32 + self.colors.len() * 4
    }
}
// Key is (outward face, plane coordinate); each plane stores rows of unit faces.
#[derive(Default)]
pub struct FaceCollector {
    planes: BTreeMap<(u32, i64), BTreeMap<(i64, i64), u32>>,
    count: usize,
}
impl FaceCollector {
    pub fn add(&mut self, face: u32, position: [i64; 3], color: [u8; 3]) -> Result<(), String> {
        if self.count >= 262_144 {
            return Err("surface color budget exceeded (262144 exposed faces)".into());
        }
        let [axis, u, v] = AXES[face as usize];
        let packed = u32::from_le_bytes([color[0], color[1], color[2], 255]);
        let old = self
            .planes
            .entry((face, position[axis]))
            .or_default()
            .insert((position[v], position[u]), packed);
        self.count += usize::from(old.is_none());
        Ok(())
    }
    /// Greedy geometry can merge different colors: the atlas retains every one.
    pub fn finish(self, origin: [i64; 3], step: i64) -> Surface {
        let mut surface = Surface {
            unit_faces: self.count,
            min_y: i64::MAX,
            max_y: i64::MIN,
            ..Surface::default()
        };
        for ((face, plane), mut cells) in self.planes {
            let [axis, ua, va] = AXES[face as usize];
            while let Some((&(v, u), _)) = cells.first_key_value() {
                let mut width = 1;
                while cells.contains_key(&(v, u + width)) {
                    width += 1;
                }
                let mut height = 1;
                while (0..width).all(|du| cells.contains_key(&(v + height, u + du))) {
                    height += 1;
                }
                let offset = surface.colors.len() as u32;
                for dv in 0..height {
                    for du in 0..width {
                        surface
                            .colors
                            .push(cells.remove(&(v + dv, u + du)).unwrap());
                    }
                }
                let mut p = [0; 3];
                p[axis] = plane;
                p[ua] = u;
                p[va] = v;
                let mut end = p;
                end[ua] += width;
                end[va] += height;
                surface.min_y = surface.min_y.min(p[1] * step);
                surface.max_y = surface.max_y.max(end[1] * step);
                surface.quads.push(Quad {
                    origin: std::array::from_fn(|i| (p[i] * step - origin[i]) as i32),
                    face,
                    size: [width as u32, height as u32],
                    color_offset: offset,
                });
            }
        }
        if surface.quads.is_empty() {
            surface.min_y = 0;
            surface.max_y = 0;
        }
        surface
    }
}
