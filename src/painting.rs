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
/// Store the selected tone in the cell value, so moving a voxel preserves color.
pub fn environment_appearance(
    pos: CellPos,
    seed: u64,
    material: MaterialId,
    environment: crate::generation::EnvironmentSample,
) -> Appearance {
    environment_appearance_m(
        crate::GridScale::default().point(pos),
        seed,
        material,
        environment,
    )
}
pub fn environment_appearance_m(
    point: [f64; 3],
    seed: u64,
    material: MaterialId,
    environment: crate::generation::EnvironmentSample,
) -> Appearance {
    // Fixed physical anchors: changing the grid does not reshuffle rock strata
    // or the small grain field. Every sampled cell still has one solid RGB.
    let anchor = CellPos::new(
        (point[0] * 16.).floor() as i64,
        (point[1] * 16.).floor() as i64,
        (point[2] * 16.).floor() as i64,
    );
    let mut original = appearance(anchor, seed);
    let grain = hash(
        seed ^ hash((point[0] * 100.).floor() as i64 as u64)
            ^ hash((point[1] * 100.).floor() as i64 as u64).rotate_left(21)
            ^ hash((point[2] * 100.).floor() as i64 as u64).rotate_left(42),
    ) >> 56;
    original.0 = (original.0 & 0xff00) | grain as u16;
    let patch = (original.0 >> 8) as f32 / 255.;
    let tone = if material == MaterialId::GRASS {
        (0.72 * environment.moisture + 0.28 * environment.ground_wetness + (patch - 0.5) * 0.12)
            .clamp(0., 1.)
    } else if material == MaterialId::SANDSTONE || material == MaterialId::LIMESTONE {
        // Broad, gently warped layers; interpolate between strata instead of
        // independently repainting a sharp horizontal band every two meters.
        let layer = point[1] / 12. + (patch as f64 - 0.5) * 0.65;
        let index = layer.floor() as i64;
        let t = layer - layer.floor();
        let t = t * t * (3. - 2. * t);
        let tone = |y: i64| (hash(seed ^ hash(y as u64)) >> 40) as f64 / 16777215.;
        let stratum = tone(index) * (1. - t) + tone(index + 1) * t;
        0.88 * patch + 0.12 * stratum as f32
    } else {
        patch
    };
    Appearance((((tone * 255.).round() as u16) << 8) | (original.0 & 255))
}
pub fn default_palette(material: MaterialId) -> [[u8; 3]; 2] {
    match material {
        MaterialId::ROCK => [[138, 143, 138], [161, 153, 138]],
        MaterialId::SOIL => [[120, 92, 61], [143, 112, 74]],
        MaterialId::SAND => [[184, 166, 120], [204, 186, 140]],
        MaterialId::GRAVEL => [[125, 133, 128], [150, 150, 138]],
        MaterialId::WATER => [[41, 102, 128], [43, 105, 130]],
        MaterialId::SNOW => [[227, 232, 227], [240, 242, 237]],
        MaterialId::GRASS => [[155, 143, 77], [62, 119, 65]],
        MaterialId::SANDSTONE => [[143, 100, 66], [198, 154, 106]],
        MaterialId::LIMESTONE => [[155, 154, 137], [190, 189, 168]],
        MaterialId::BASALT => [[68, 75, 75], [101, 108, 103]],
        MaterialId::MUD => [[79, 75, 52], [106, 99, 67]],
        MaterialId::SILT => [[141, 124, 84], [166, 151, 110]],
        _ => [[204, 26, 179], [204, 26, 179]],
    }
}
pub fn base_color(voxel: Voxel, registry: &crate::materials::MaterialRegistry) -> [u8; 3] {
    if voxel.material == MaterialId::SURVEY {
        return decode_rgb565(voxel.appearance);
    }
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

/// Exact compact color travels with edited/moved survey voxels. No per-face paint.
pub fn encode_rgb565(rgb: [u8; 3]) -> Appearance {
    Appearance(((rgb[0] as u16 >> 3) << 11) | ((rgb[1] as u16 >> 2) << 5) | (rgb[2] as u16 >> 3))
}
pub fn decode_rgb565(a: Appearance) -> [u8; 3] {
    let r = ((a.0 >> 11) & 31) as u8;
    let g = ((a.0 >> 5) & 63) as u8;
    let b = (a.0 & 31) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}
