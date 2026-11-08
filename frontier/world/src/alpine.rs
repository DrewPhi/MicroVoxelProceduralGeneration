//! One algebraic composition shared by canonical values, cover derivatives and
//! outward-rounded affine bounds. No sampled slope/error claims or renderer LOD.
use crate::FbmParams;

/// (wavelength, amplitude, salt), in authored scene meters. These low relief
/// bands retain irregular native stair boundaries on otherwise quiet meadows.
/// Shared with the global source envelope/slope bounds, independent of octaves.
pub(crate) const MICRORELIEF_BANDS: [(f64, f64, u64); 2] =
    [(1.2, 0.03, 0xa130), (0.75, 0.01, 0xa131)];

pub(crate) trait Value: Copy {
    fn point(v: f64) -> Self;
    fn add(self, b: Self) -> Self;
    fn scale(self, k: f64) -> Self;
    fn mul(self, b: Self) -> Self;
    fn clamp01(self) -> Self;
}
impl Value for f64 {
    fn point(v: f64) -> Self {
        v
    }
    fn add(self, b: Self) -> Self {
        self + b
    }
    fn scale(self, k: f64) -> Self {
        self * k
    }
    fn mul(self, b: Self) -> Self {
        self * b
    }
    fn clamp01(self) -> Self {
        self.clamp(0.0, 1.0)
    }
}
fn bump<V: Value>(d: V) -> V {
    let q = d.mul(d).scale(-1.0).add(V::point(1.0)).clamp01();
    q.mul(q)
}
/// Coordinates are meters / horizontal_scale. Defaults describe a roughly 1km
/// meadow and a range centered at 3.3km. Parameters scale the complete scene.
pub(crate) fn elevation<V: Value>(
    x: V,
    z: V,
    p: FbmParams,
    detail: bool,
    mut noise: impl FnMut(V, V, u64) -> V,
) -> V {
    let x = x.scale(1800.0);
    let z = z.scale(1800.0);
    let n = |x: V, z: V, wavelength: f64, salt, noise: &mut dyn FnMut(V, V, u64) -> V| {
        noise(
            x.scale(1.0 / wavelength).add(V::point(12.7)),
            z.scale(1.0 / wavelength).add(V::point(-29.1)),
            salt,
        )
    };
    // Quiet broad hummocks throughout the foreground. No independent random
    // mountain provinces: a single continuous curved ridge owns the backdrop.
    let meadow = n(x, z, 750.0, 0xa101, &mut noise)
        .scale(42.0)
        .add(n(x, z, 240.0, 0xa102, &mut noise).scale(12.0));
    let center = n(x, V::point(0.0), 1800.0, 0xa103, &mut noise)
        .scale(340.0)
        .add(V::point(3300.0));
    let d = z.add(center.scale(-1.0)).scale(1.0 / 1900.0);
    let envelope = bump(d);
    // Lower connected shoulders, four distinct broad peaks and a wide saddle
    // between the central two peaks. These anchors shape one continuous range.
    let peaks = bump(x.add(V::point(1800.0)).scale(1.0 / 1100.0))
        .scale(650.0)
        .add(bump(x.add(V::point(650.0)).scale(1.0 / 850.0)).scale(470.0))
        .add(bump(x.add(V::point(-1100.0)).scale(1.0 / 1000.0)).scale(800.0))
        .add(bump(x.add(V::point(-2600.0)).scale(1.0 / 1000.0)).scale(600.0));
    let crest = n(x, z, 850.0, 0xa104, &mut noise)
        .scale(100.0)
        .add(V::point(500.0))
        .add(peaks);
    let ribs = n(x, z, 180.0, 0xa105, &mut noise).scale(55.0);
    // Two broad lower hills overlap the range's feet at different depths. Their
    // compact support leaves the central foreground meadow open and walkable.
    let hill = |cx: f64, cz: f64, rx: f64, rz: f64| {
        let dx = x.add(V::point(-cx)).scale(1.0 / rx);
        let dz = z.add(V::point(-cz)).scale(1.0 / rz);
        let q = dx
            .mul(dx)
            .add(dz.mul(dz))
            .scale(-1.0)
            .add(V::point(1.0))
            .clamp01();
        q.mul(q)
    };
    let foothills = hill(-900.0, 1500.0, 1500.0, 900.0)
        .scale(190.0)
        .add(hill(1550.0, 2000.0, 1400.0, 1100.0).scale(290.0));
    // This sixth broad band adds continuous shoulder variation without making
    // another layer of disconnected mountains.
    let shoulder = n(x, z, 480.0, 0xa106, &mut noise).scale(25.0);
    let mut h = meadow
        .add(envelope.mul(crest.add(ribs).add(shoulder)))
        .add(foothills);
    if detail {
        let (mut sum, mut norm, mut weight, mut wavelength) = (V::point(0.0), 0.0, 1.0, 45.0);
        for octave in 0..p.octaves {
            sum = sum.add(n(x, z, wavelength, 0xa110 + octave as u64, &mut noise).scale(weight));
            norm += weight;
            weight *= 0.65 * p.roughness;
            wavelength *= 0.5;
        }
        h = h.add(
            sum.scale(1.0 / norm)
                .mul(envelope.scale(7.5).add(V::point(0.45))),
        );
        // Fixed physical bands avoid the vanishing sub-meter amplitude of the
        // larger 45m octave stack. The same expression feeds scalar values,
        // derivatives and affine bounds; these are real heights, not paint.
        for (wavelength, amplitude, salt) in MICRORELIEF_BANDS {
            h = h.add(n(x, z, wavelength, salt, &mut noise).scale(amplitude));
        }
    }
    h.scale(p.amplitude / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, FbmRecipe, FbmTerrain, Material, World, VOXEL_SIZE};
    fn terrain(seed: u64) -> FbmTerrain {
        FbmTerrain::new(
            seed,
            FbmParams {
                recipe: FbmRecipe::Alpine,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn centimeter_relief_is_bounded_coherent_and_changes_native_stairs() {
        for seed in [0, 7, 91] {
            let mut changed = 0;
            let mut max_relief = 0.0_f64;
            let mut previous = 0.0;
            let t = terrain(seed);
            let p = t.params();
            for i in 0..1024 {
                let x = (-50.0 + (i as f64 + 0.5) * VOXEL_SIZE) / p.horizontal_scale;
                let z = -249.96875 / p.horizontal_scale;
                let full = elevation(x, z, p, true, |x, z, salt| {
                    crate::terrain::noise(x, z, seed ^ salt)
                });
                let old = elevation(x, z, p, true, |x, z, salt| {
                    if MICRORELIEF_BANDS.iter().any(|b| b.2 == salt) {
                        0.0
                    } else {
                        crate::terrain::noise(x, z, seed ^ salt)
                    }
                });
                let relief = full - old;
                assert!(relief.abs() <= 0.04 + 1e-12);
                max_relief = max_relief.max(relief.abs());
                // Neighboring native columns share coherent relief rather than
                // independent random heights at each 6.25cm address.
                if i > 0 {
                    assert!((relief - previous).abs() < 0.015);
                }
                previous = relief;
                let old_cells = (old / VOXEL_SIZE).floor() as i64;
                let full_cells = (full / VOXEL_SIZE).floor() as i64;
                assert!((old_cells - full_cells).abs() <= 1);
                changed += usize::from(old_cells != full_cells);
                assert_eq!(
                    t.height_m(x * p.horizontal_scale, z * p.horizontal_scale),
                    full_cells as f64 * VOXEL_SIZE
                );
            }
            assert!(max_relief > 0.015, "seed={seed} max={max_relief}");
            assert!(changed > 50, "seed={seed} changed={changed}");
        }
    }

    #[test]
    fn quiet_meadow_connected_distant_range_and_native_path() {
        for seed in [0, 7, 91] {
            let t = terrain(seed);
            for z in (-500..=500).step_by(50) {
                for x in (-500..=500).step_by(50) {
                    assert!(t.height_m(x as f64, z as f64).abs() < 55.0);
                }
            }
            // The entire central crest has connected elevated shoulders, with
            // neither foreground spikes nor repeated isolated mountain islands.
            for x in (-2000..=2000).step_by(50) {
                let h = t.height_m(x as f64, 3300.0);
                assert!((250.0..1600.0).contains(&h), "seed={seed} x={x} h={h}");
            }
            let w = World::fbm(seed, t.params()).unwrap();
            for z in [-400., 0., 500., 1400.] {
                let x = crate::terrain::alpine_path_center_m(z);
                let h = t.height_m(x, z);
                assert_eq!(t.surface_material(x, z, h), Material::Soil);
                if z <= 500. {
                    assert_eq!(
                        t.surface_material(x + 8., z, t.height_m(x + 8., z)),
                        Material::Grass
                    );
                }
                let mut c = Cell::from_meters([x, h, z]);
                c.y -= 1;
                assert_eq!(w.material(c), Material::Soil);
                assert_eq!(w.material(Cell { y: c.y + 1, ..c }), Material::Air);
                assert_eq!(h / VOXEL_SIZE, (h / VOXEL_SIZE).floor());
            }
            let mut rock_columns = 0;
            for x in (-1800..=1800).step_by(300) {
                let z = 2800.;
                let h = t.height_m(x as f64, z);
                if t.surface_material(x as f64, z, h) == Material::Stone {
                    rock_columns += 1;
                    let c = Cell::from_meters([x as f64, h, z]);
                    for depth in [1, 2, 6, 11, 12, 20] {
                        assert_eq!(
                            w.material(Cell {
                                y: c.y - depth,
                                ..c
                            }),
                            Material::Stone
                        );
                    }
                }
            }
            assert!(rock_columns > 0);
            assert_eq!(t.source_version(), 4);
            assert_eq!(
                t.appearance_version(),
                crate::terrain::ALPINE_APPEARANCE_VERSION
            );
        }
    }
}
