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
// Key is (outward face, plane coordinate); each plane stores rows of unit faces.
#[derive(Default)]
pub struct FaceCollector {
    planes: BTreeMap<(u32, i64), BTreeMap<(i64, i64), u32>>,
    count: usize,
}
impl FaceCollector {
    pub fn add(&mut self, face: u32, position: [i64; 3], color: [u8; 3]) -> Result<(), String> {
        self.add_shaded(face, position, color, 255)
    }
    /// Alpha stores presentation lighting, independently of canonical RGB.
    pub fn add_shaded(
        &mut self,
        face: u32,
        position: [i64; 3],
        color: [u8; 3],
        ambient: u8,
    ) -> Result<(), String> {
        if self.count >= 262_144 {
            return Err("surface color budget exceeded (262144 exposed faces)".into());
        }
        let [axis, u, v] = AXES[face as usize];
        let packed = u32::from_le_bytes([color[0], color[1], color[2], ambient]);
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
                    constant_color: false,
                    origin: std::array::from_fn(|i| (p[i] * step - origin[i]) as i32),
                    face,
                    size: [width as u32, height as u32],
                    color_offset: offset,
                    water_level: 0,
                    slope_light: false,
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
