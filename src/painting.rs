//! Canonical voxel appearance. This code knows nothing about faces or cameras.
use crate::{
    grid::CellPos,
    materials::{Appearance, MaterialId, Voxel},
};

pub fn hash(mut n: u64) -> u64 {
    n = n.wrapping_add(0x9e3779b97f4a7c15);
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
    n ^ (n >> 31)
}
fn random(x: i64, z: i64, seed: u64) -> f64 {
    (hash(seed ^ hash(x as u64) ^ hash(z as u64).rotate_left(27)) >> 11) as f64
        / (1u64 << 53) as f64
}
pub fn appearance(pos: CellPos, seed: u64) -> Appearance {
    let x = pos.x.div_euclid(128);
    let z = pos.z.div_euclid(128);
    let smooth = |n: i64| {
        let t = n.rem_euclid(128) as f64 / 128.0;
        t * t * (3.0 - 2.0 * t)
    };
    let tx = smooth(pos.x);
    let tz = smooth(pos.z);
    let a = random(x, z, seed) * (1.0 - tx) + random(x + 1, z, seed) * tx;
    let b = random(x, z + 1, seed) * (1.0 - tx) + random(x + 1, z + 1, seed) * tx;
    let tone = ((a * (1.0 - tz) + b * tz) * 255.0).round() as u16;
    let grain = (hash(
        seed ^ hash(pos.x as u64)
            ^ hash(pos.y as u64).rotate_left(21)
            ^ hash(pos.z as u64).rotate_left(42),
    ) >> 56) as u16;
    Appearance((tone << 8) | grain)
}
pub fn default_palette(material: MaterialId) -> [[u8; 3]; 2] {
    match material {
        MaterialId::ROCK => [[138, 143, 138], [161, 153, 138]],
        MaterialId::SOIL => [[120, 92, 61], [143, 112, 74]],
        MaterialId::SAND => [[184, 166, 120], [204, 186, 140]],
        MaterialId::GRAVEL => [[125, 133, 128], [150, 150, 138]],
        MaterialId::WATER => [[41, 102, 128], [43, 105, 130]],
        MaterialId::SNOW => [[227, 232, 227], [240, 242, 237]],
        MaterialId::GRASS => [[82, 120, 66], [112, 135, 77]],
        _ => [[204, 26, 179], [204, 26, 179]],
    }
}
pub fn base_color(voxel: Voxel, registry: &crate::materials::MaterialRegistry) -> [u8; 3] {
    let [low, high] = registry
        .get(voxel.material)
        .map_or_else(|| default_palette(voxel.material), |d| d.palette);
    let tone = (voxel.appearance.0 >> 8) as f64 / 255.0;
    let grain = if voxel.material == MaterialId::WATER {
        1.0
    } else {
        1.0 + ((voxel.appearance.0 & 255) as f64 / 255.0 - 0.5) * 0.06
    };
    std::array::from_fn(|i| {
        ((low[i] as f64 + (high[i] as f64 - low[i] as f64) * tone) * grain)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}
