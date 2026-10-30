use microvoxel::render::surface::{FaceCollector, MesherScratch, Surface, AXES};
use std::collections::BTreeMap;

// Frozen pre-rewrite oracle: keep its map-based algorithm independent.
mod reference {
    use super::*;
    use microvoxel::render::surface::Quad;
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
}

type Input = (u32, [i64; 3], [u8; 3], u8);
fn fixture(size: i64, holes: bool) -> Vec<Input> {
    let mut out = Vec::new();
    for face in 0..6 {
        let [axis, ua, va] = AXES[face as usize];
        for v in -size..size {
            for u in -size..size {
                if holes && ((u * 17 + v * 23).rem_euclid(11) == 0 || (u.abs() < 3 && v.abs() < 5))
                {
                    continue;
                }
                let mut p = [0; 3];
                p[axis] = face as i64 - 7;
                p[ua] = u;
                p[va] = v;
                out.push((
                    face,
                    p,
                    [u as u8, v as u8, (u ^ v) as u8],
                    (u + 3 * v) as u8,
                ));
            }
        }
    }
    out
}
fn unpack(surface: &Surface) -> BTreeMap<(u32, [i32; 3]), u32> {
    let mut out = BTreeMap::new();
    for q in &surface.quads {
        let [_, ua, va] = AXES[q.face as usize];
        for v in 0..q.size[1] {
            for u in 0..q.size[0] {
                let mut p = q.origin;
                p[ua] += u as i32;
                p[va] += v as i32;
                let index = q.color_offset as usize
                    + if q.constant_color {
                        0
                    } else {
                        (v * q.size[0] + u) as usize
                    };
                assert!(out.insert((q.face, p), surface.colors[index]).is_none());
            }
        }
    }
    out
}
fn compare(input: &[Input], scratch: &mut MesherScratch) {
    let mut old = reference::FaceCollector::default();
    let mut new = FaceCollector::default();
    for &(f, p, c, a) in input {
        old.add_shaded(f, p, c, a).unwrap();
        new.add_shaded(f, p, c, a).unwrap();
    }
    let old = old.finish([5, -8, 19], 1);
    let new = new.finish_with_scratch([5, -8, 19], 1, scratch);
    assert_eq!(new.unit_faces, old.unit_faces);
    assert_eq!((new.min_y, new.max_y), (old.min_y, old.max_y));
    assert_eq!(unpack(&new), unpack(&old));
}
#[test]
fn exact_faces_colors_holes_signed_pages_and_scratch_reuse() {
    let mut scratch = MesherScratch::default();
    compare(&fixture(40, false), &mut scratch);
    compare(&fixture(32, false), &mut scratch);
    compare(&fixture(40, true), &mut scratch);
    compare(&[], &mut scratch);
    let mut input = fixture(3, true);
    input.push(input[0]);
    input.last_mut().unwrap().2 = [9, 8, 7];
    compare(&input, &mut scratch);
}
#[test]
fn sparse_large_signed_positions_and_nonunit_step() {
    let mut old = reference::FaceCollector::default();
    let mut new = FaceCollector::default();
    for p in [[-1_000_000, -39, 999_999], [1_000_000, 41, -999_999]] {
        for f in 0..6 {
            old.add(f, p, [1, 2, 3]).unwrap();
            new.add(f, p, [1, 2, 3]).unwrap();
        }
    }
    let old = old.finish([200, -400, 600], 2);
    let new = new.finish([200, -400, 600], 2);
    assert_eq!(unpack(&old), unpack(&new));
    assert_eq!((new.min_y, new.max_y), (old.min_y, old.max_y));
}
#[test]
fn deterministic_output_and_budget() {
    let input = fixture(40, true);
    let build = |reverse: bool| {
        let mut c = FaceCollector::default();
        let ordered: Vec<_> = if reverse {
            input.iter().rev().collect()
        } else {
            input.iter().collect()
        };
        for &(f, p, rgb, a) in ordered {
            c.add_shaded(f, p, rgb, a).unwrap();
        }
        format!("{:?}", c.finish([0; 3], 1))
    };
    assert_eq!(build(false), build(true));
    let mut c = FaceCollector::default();
    for u in 0..262_144 {
        c.add(0, [u, 0, 0], [1, 2, 3]).unwrap();
    }
    assert!(c.add(0, [262_144, 0, 0], [1, 2, 3]).is_err());
}
#[test]
#[ignore = "manual matched release mesher benchmark"]
fn matched_collector_timings() {
    use std::time::Instant;
    for (size, holes) in [(16, false), (16, true), (64, false), (64, true)] {
        let input = fixture(size, holes);
        let start = Instant::now();
        let mut old_quads = 0;
        for _ in 0..10 {
            let mut c = reference::FaceCollector::default();
            for &(f, p, rgb, a) in &input {
                c.add_shaded(f, p, rgb, a).unwrap();
            }
            old_quads += std::hint::black_box(c.finish([0; 3], 1)).quads.len();
        }
        let old_time = start.elapsed();
        let mut scratch = MesherScratch::default();
        let start = Instant::now();
        let mut new_quads = 0;
        for _ in 0..10 {
            let mut c = FaceCollector::default();
            for &(f, p, rgb, a) in &input {
                c.add_shaded(f, p, rgb, a).unwrap();
            }
            new_quads += std::hint::black_box(c.finish_with_scratch([0; 3], 1, &mut scratch))
                .quads
                .len();
        }
        let new_time = start.elapsed();
        eprintln!("size={size} holes={holes} faces={} iterations=10 old={old_time:?} packed={new_time:?} speedup={:.2} quads(old/new)={old_quads}/{new_quads}", input.len(), old_time.as_secs_f64() / new_time.as_secs_f64());
    }
}
