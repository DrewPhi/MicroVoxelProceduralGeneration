//! Independently implemented packed face-page mesher. Algorithm references:
//! cgerikj/binary-greedy-meshing and Inspirateur/binary-greedy-meshing.
//! Occupancy, trailing-zero scans and reusable buffers apply to already exposed
//! faces here, rather than allocating a full native volume for tall terrain.
use super::{Quad, Surface, AXES};
use std::collections::HashMap;

const SIDE: usize = 32;
const SCRATCH_SIDE: usize = 64;
const BUDGET: usize = 262_144;
type Key = (u32, i64, i64, i64); // face, plane, page V, page U

#[derive(Default)]
struct Page {
    rows: [u32; SIDE],
    // Unique writes keep the hot path contiguous. Rare repeated writes update
    // in place so duplicate submissions cannot grow memory without bound.
    colors: Vec<(u16, u32)>,
}

/// Reusable bounded worker scratch, independent of world-coordinate span.
/// Scratch uses 512 occupancy bytes and 16 KiB of appearance. Neighboring pages
/// share a sweep when the complete plane fits, avoiding native tile seams.
pub struct MesherScratch {
    rows: [u64; SCRATCH_SIDE],
    colors: [u32; SCRATCH_SIDE * SCRATCH_SIDE],
}
impl Default for MesherScratch {
    fn default() -> Self {
        Self {
            rows: [0; SCRATCH_SIDE],
            colors: [0; SCRATCH_SIDE * SCRATCH_SIDE],
        }
    }
}

/// Sparse planes of 32×32 packed face pages. Only touched pages allocate.
#[derive(Default)]
pub struct FaceCollector {
    pages: HashMap<Key, Page>,
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
        if self.count >= BUDGET {
            return Err("surface color budget exceeded (262144 exposed faces)".into());
        }
        let [axis, ua, va] = AXES[face as usize];
        let u = position[ua];
        let v = position[va];
        let key = (
            face,
            position[axis],
            v.div_euclid(SIDE as i64),
            u.div_euclid(SIDE as i64),
        );
        let u = u.rem_euclid(SIDE as i64) as usize;
        let v = v.rem_euclid(SIDE as i64) as usize;
        let page = self.pages.entry(key).or_default();
        let bit = 1u32 << u;
        let index = (v * SIDE + u) as u16;
        let packed = u32::from_le_bytes([color[0], color[1], color[2], ambient]);
        if page.rows[v] & bit != 0 {
            page.colors.iter_mut().find(|(i, _)| *i == index).unwrap().1 = packed;
        } else {
            self.count += 1;
            page.rows[v] |= bit;
            page.colors.push((index, packed));
        }
        Ok(())
    }
    pub fn finish(self, origin: [i64; 3], step: i64) -> Surface {
        self.finish_with_scratch(origin, step, &mut MesherScratch::default())
    }
    /// Greedy geometry may merge different colors: each atlas cell stays exact.
    /// Stable page ordering makes output independent of hash randomization and
    /// insertion order (apart from intentional duplicate last-color-wins writes).
    pub fn finish_with_scratch(
        self,
        origin: [i64; 3],
        step: i64,
        scratch: &mut MesherScratch,
    ) -> Surface {
        let mut surface = Surface {
            unit_faces: self.count,
            min_y: i64::MAX,
            max_y: i64::MIN,
            ..Surface::default()
        };
        surface.colors.reserve(self.count);
        let mut pages: Vec<_> = self.pages.into_iter().collect();
        pages.sort_unstable_by_key(|(key, _)| *key);
        let mut begin = 0;
        while begin < pages.len() {
            let (face, plane, _, _) = pages[begin].0;
            let mut end = begin + 1;
            while end < pages.len() && pages[end].0 .0 == face && pages[end].0 .1 == plane {
                end += 1;
            }
            let plane_pages = &pages[begin..end];
            let min_v = plane_pages.iter().map(|(k, _)| k.2).min().unwrap();
            let max_v = plane_pages.iter().map(|(k, _)| k.2).max().unwrap();
            let min_u = plane_pages.iter().map(|(k, _)| k.3).min().unwrap();
            let max_u = plane_pages.iter().map(|(k, _)| k.3).max().unwrap();
            if max_v - min_v <= 1 && max_u - min_u <= 1 {
                scratch.load(plane_pages, min_u, min_v);
                scratch.emit(&mut surface, face, plane, min_u, min_v, origin, step);
            } else {
                for page in plane_pages {
                    let (_, _, pv, pu) = page.0;
                    scratch.load(std::slice::from_ref(page), pu, pv);
                    scratch.emit(&mut surface, face, plane, pu, pv, origin, step);
                }
            }
            begin = end;
        }
        if surface.quads.is_empty() {
            surface.min_y = 0;
            surface.max_y = 0;
        }
        surface
    }
}

impl MesherScratch {
    fn load(&mut self, pages: &[(Key, Page)], pu: i64, pv: i64) {
        self.rows.fill(0);
        for (key, page) in pages {
            let du = ((key.3 - pu) * SIDE as i64) as usize;
            let dv = ((key.2 - pv) * SIDE as i64) as usize;
            for (v, bits) in page.rows.iter().enumerate() {
                self.rows[dv + v] |= (*bits as u64) << du;
            }
            for &(index, color) in &page.colors {
                let index = index as usize;
                self.colors[(dv + index / SIDE) * SCRATCH_SIDE + du + index % SIDE] = color;
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        surface: &mut Surface,
        face: u32,
        plane: i64,
        pu: i64,
        pv: i64,
        origin: [i64; 3],
        step: i64,
    ) {
        let [axis, ua, va] = AXES[face as usize];
        for v in 0..SCRATCH_SIDE {
            while self.rows[v] != 0 {
                let u = self.rows[v].trailing_zeros() as usize;
                let width = (self.rows[v] >> u).trailing_ones() as usize;
                // Shifting by 64 is invalid, so construct the full-row case explicitly.
                let mask = if width == SCRATCH_SIDE {
                    u64::MAX
                } else {
                    ((1u64 << width) - 1) << u
                };
                let mut height = 1;
                while v + height < SCRATCH_SIDE && self.rows[v + height] & mask == mask {
                    height += 1;
                }
                let offset = surface.colors.len() as u32;
                for dv in 0..height {
                    self.rows[v + dv] &= !mask;
                    let start = (v + dv) * SCRATCH_SIDE + u;
                    surface
                        .colors
                        .extend_from_slice(&self.colors[start..start + width]);
                }
                let mut p = [0; 3];
                p[axis] = plane;
                p[ua] = pu * SIDE as i64 + u as i64;
                p[va] = pv * SIDE as i64 + v as i64;
                let mut end = p;
                end[ua] += width as i64;
                end[va] += height as i64;
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
    }
}
