//! Seeded desert composition. Shared scalar/affine algebra keeps display bounds
//! tied to the same ground and level-water source as native voxel columns.
use crate::terrain::{noise, TerrainColumn};
use crate::{alpine::Value, FbmParams, Material, VOXEL_SIZE};

pub const VERSION: u32 = 2;
const SPACING: f64 = 1400.;
#[derive(Clone, Copy, Debug)]
pub struct Oasis {
    pub x: f64,
    pub z: f64,
    pub radius: f64,
    pub stage: f64,
}
fn hash(mut v: u64) -> u64 {
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
    v ^ (v >> 31)
}
fn unit(v: u64) -> f64 {
    (hash(v) >> 11) as f64 / (1u64 << 53) as f64
}
pub fn oasis(seed: u64, i: i64, j: i64) -> Oasis {
    let key = seed
        ^ (i as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (j as u64).wrapping_mul(0xd1b54a32d192ed03);
    let x = (i as f64 + 0.32 + 0.36 * unit(key)) * SPACING;
    let z = (j as f64 + 0.32 + 0.36 * unit(key ^ 19)) * SPACING;
    Oasis {
        x,
        z,
        radius: 22. + 14. * unit(key ^ 31),
        stage: base(x, z, |x, z, s| noise(x, z, seed ^ s)) - 4.,
    }
}
fn smooth<V: Value>(x: V) -> V {
    let x = x.clamp01();
    x.mul(x).mul(V::point(3.).add(x.scale(-2.)))
}
fn base<V: Value>(x: V, z: V, mut n: impl FnMut(V, V, u64) -> V) -> V {
    let mut broad = V::point(0.);
    let (mut wavelength, mut amplitude) = (2200., 26.);
    for octave in 0..4 {
        broad = broad.add(
            n(
                x.scale(1. / wavelength),
                z.scale(1. / wavelength),
                0xde10 + octave,
            )
            .scale(amplitude),
        );
        wavelength *= 0.5;
        amplitude *= 0.35;
    }
    let bend = n(x.scale(1. / 600.), z.scale(1. / 600.), 0xde02).scale(35.);
    let wind = x.add(z.scale(0.38)).add(bend);
    // Anisotropic, smoothly sharpened noise produces elongated dune crests.
    let wave = n(wind.scale(1. / 95.), z.scale(1. / 480.), 0xde03)
        .scale(0.5)
        .add(V::point(0.5));
    let dunes = wave.mul(wave).mul(wave).scale(27.);
    let province = smooth(
        n(x.scale(1. / 1700.), z.scale(1. / 1700.), 0xde04)
            .scale(2.5)
            .add(V::point(0.1)),
    );
    let rocky = n(x.scale(1. / 460.), z.scale(1. / 460.), 0xde05)
        .scale(0.5)
        .add(V::point(0.5));
    let rock = province.mul(rocky.mul(rocky).scale(150.).add(V::point(18.)));
    // Narrow connected dry washes follow a continuous warped field.
    let wash = n(x.scale(1. / 380.), z.scale(1. / 650.), 0xde06);
    let channel = V::point(1.).add(wash.mul(wash).scale(-110.)).clamp01();
    V::point(45.)
        .add(broad)
        .add(dunes)
        .add(rock)
        .add(channel.mul(channel).scale(-2.))
        .add(n(x.scale(1. / 18.), z.scale(1. / 18.), 0xde07).scale(0.18))
        .add(n(x.scale(1. / 1.8), z.scale(1. / 1.8), 0xde08).scale(0.025))
}
/// Authored meters, before physical parameter scaling. Bounds specify the same
/// domain as x/z and restrict descriptor enumeration, never an evaluation grid.
pub(crate) fn elevation<V: Value>(
    seed: u64,
    x: V,
    z: V,
    min: [f64; 2],
    max: [f64; 2],
    visible: bool,
    mut n: impl FnMut(V, V, u64) -> V,
) -> V {
    let mut h = base(x, z, &mut n);
    for j in ((min[1] - 160.) / SPACING).floor() as i64..=((max[1] + 160.) / SPACING).floor() as i64
    {
        for i in
            ((min[0] - 160.) / SPACING).floor() as i64..=((max[0] + 160.) / SPACING).floor() as i64
        {
            let o = oasis(seed, i, j);
            if o.x + 160. < min[0]
                || o.x - 160. > max[0]
                || o.z + 160. < min[1]
                || o.z - 160. > max[1]
            {
                continue;
            }
            let dx = x.add(V::point(-o.x)).scale(1. / o.radius);
            let dz = z.add(V::point(-o.z)).scale(1. / (o.radius * 0.72));
            let shoreline = n(x.scale(1. / 28.), z.scale(1. / 28.), 0xde09)
                .scale(0.28)
                .add(V::point(1.));
            let q = dx.mul(dx).add(dz.mul(dz)).mul(shoreline);
            let mask = smooth(V::point(1.).add(q.add(V::point(-1.)).scale(-1. / 8.)));
            let bed = V::point(o.stage - 2.).add(q.scale(3.));
            h = h.mul(V::point(1.).add(mask.scale(-1.))).add(bed.mul(mask));
            if visible {
                // Positive part with a known > maximum source relief bound.
                let fill = V::point(o.stage)
                    .add(h.scale(-1.))
                    .scale(1. / 1000.)
                    .clamp01()
                    .scale(1000.);
                h = h.add(fill.mul(V::point(4.).add(q.scale(-1.)).clamp01()));
            }
        }
    }
    h
}
pub fn heights(seed: u64, p: FbmParams, x: f64, z: f64) -> (f64, f64) {
    let scale = p.horizontal_scale / 1800.;
    let x = x / scale;
    let z = z / scale;
    let mut n = |x, z, s| noise(x, z, seed ^ s);
    let g = elevation(seed, x, z, [x, z], [x, z], false, &mut n) * p.amplitude / 1000.;
    let v = elevation(seed, x, z, [x, z], [x, z], true, &mut n) * p.amplitude / 1000.;
    (g, v)
}
pub fn column(seed: u64, p: FbmParams, x: f64, z: f64) -> TerrainColumn {
    let (g, v) = heights(seed, p, x, z);
    let ground_top = (g / VOXEL_SIZE).floor() as i64;
    let visible_top = (v / VOXEL_SIZE).floor() as i64;
    let scale = p.horizontal_scale / 1800.;
    let exposure = noise(x / scale / 1700., z / scale / 1700., seed ^ 0xde04);
    let ground_surface = if exposure > 0.28 && g > 70. * p.amplitude / 1000. {
        Material::Stone
    } else {
        Material::Sand
    };
    TerrainColumn {
        ground_top,
        visible_top,
        ground_surface,
        ground_subsurface: ground_surface,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, FbmRecipe, FbmTerrain, World};
    #[test]
    fn affine_bounds_cover_pond_shores_dunes_and_negative_coordinates() {
        use crate::affine_bounds::{bound_native_height, BoundBudget, ColumnBox};
        for seed in [7, 19] {
            let t = FbmTerrain::new(
                seed,
                FbmParams {
                    recipe: FbmRecipe::Desert,
                    ..Default::default()
                },
            )
            .unwrap();
            let o = oasis(seed, 0, 0);
            for (x, z) in [
                (o.x, o.z),
                (o.x + o.radius, o.z),
                (-1400., -35.),
                (800., 1200.),
            ] {
                let c = Cell::from_meters([x, 0., z]);
                let r = ColumnBox {
                    min: [c.x - 128, c.z - 128],
                    size_cells: [256, 256],
                };
                let b = bound_native_height(&t, r, BoundBudget::default()).unwrap();
                for j in 0..9 {
                    for i in 0..9 {
                        let h = t.height_cells(r.min[0] + i * 31, r.min[1] + j * 31) as f64
                            * VOXEL_SIZE;
                        assert!(
                            h >= b.height_range[0] && h <= b.height_range[1],
                            "{h} {:?}",
                            b.height_range
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn ponds_are_level_bounded_and_share_native_water_and_bed() {
        for seed in [7, 19, 93] {
            let p = FbmParams {
                recipe: FbmRecipe::Desert,
                ..Default::default()
            };
            let t = FbmTerrain::new(seed, p).unwrap();
            let w = World::fbm(seed, p).unwrap();
            for (i, j) in [(0, 0), (-1, -1), (2, -3)] {
                let o = oasis(seed, i, j);
                let c = Cell::from_meters([o.x, 0., o.z]);
                let col = t.column_cells(c.x, c.z);
                assert!(col.visible_top - col.ground_top >= 30);
                assert_eq!(
                    w.material(Cell {
                        y: col.visible_top - 1,
                        ..c
                    }),
                    Material::Water
                );
                assert_ne!(
                    w.material(Cell {
                        y: col.ground_top - 1,
                        ..c
                    }),
                    Material::Water
                );
                assert_eq!(col.visible_top, (o.stage / VOXEL_SIZE).floor() as i64);
                assert_eq!(
                    column(seed, p, o.x + 160., o.z).ground_top,
                    column(seed, p, o.x + 160., o.z).visible_top
                );
            }
        }
    }
}
