//! Small, periodic, world-space cloud noise shared by the sky and ground shadows.
use glam::{IVec3, Vec3};
use std::sync::OnceLock;

pub(crate) const SIDE: u32 = 48;
pub(crate) const BYTES: u64 = SIDE as u64 * SIDE as u64 * SIDE as u64 * 4;

fn random(mut value: u32) -> f32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846ca68b);
    value ^= value >> 16;
    (value >> 8) as f32 / 16_777_216.
}

pub(crate) fn phase(seed: u64) -> [f32; 4] {
    let key = seed as u32 ^ ((seed >> 32) as u32).rotate_left(13);
    [
        random(key ^ 0x5f294d87),
        random(key ^ 0x37f3911b),
        random(key ^ 0xda7f6922),
        0.,
    ]
}

struct Cellular {
    side: i32,
    points: Vec<Vec3>,
}

impl Cellular {
    fn new(side: i32, salt: u32) -> Self {
        let points = (0..side.pow(3) as u32)
            .map(|i| {
                let key = (i ^ salt).wrapping_mul(0x4c957f2d);
                Vec3::new(random(key), random(key ^ 0xd51d), random(key ^ 0x671a)) * 0.86 + 0.07
            })
            .collect();
        Self { side, points }
    }

    fn sample(&self, point: Vec3) -> (f32, f32) {
        let point = point * self.side as f32;
        let cell = point.floor().as_ivec3();
        let mut first = f32::INFINITY;
        let mut second = f32::INFINITY;
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let p = cell + IVec3::new(x, y, z);
                    let q = p.rem_euclid(IVec3::splat(self.side));
                    let id = (q.x + self.side * (q.y + self.side * q.z)) as usize;
                    let d = (p.as_vec3() + self.points[id] - point).length_squared();
                    if d < first {
                        second = first;
                        first = d;
                    } else {
                        second = second.min(d);
                    }
                }
            }
        }
        (first.sqrt(), second.sqrt())
    }
}

pub(crate) fn samples() -> &'static [u8] {
    static SAMPLES: OnceLock<Vec<u8>> = OnceLock::new();
    SAMPLES.get_or_init(|| {
        let broad = Cellular::new(5, 0x6b2bef);
        let billow = Cellular::new(11, 0x42c917);
        let grain = Cellular::new(19, 0x13594d);
        let mut output = Vec::with_capacity(BYTES as usize);
        for z in 0..SIDE {
            for y in 0..SIDE {
                for x in 0..SIDE {
                    let p = (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / SIDE as f32;
                    let (a, _) = broad.sample(p);
                    let (b, dome) = billow.sample(p);
                    let (c, _) = grain.sample(p);
                    output.extend(
                        [1. - 0.92 * a, 1. - b, 1. - c, dome - b]
                            .map(|v| (255. * v.clamp(0., 1.)).round() as u8),
                    );
                }
            }
        }
        output
    })
}
