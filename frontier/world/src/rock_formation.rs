//! Seeded rock masses with genuinely three-dimensional air and solid cells.
use crate::{voxel_size, Cell, FbmTerrain};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormationKind {
    Overhang,
    Grotto,
    Arch,
}

#[derive(Clone, Copy, Debug)]
pub struct RockFormation {
    kind: FormationKind,
    center: [f64; 3],
    phase: f64,
    min: Cell,
    max: Cell,
}

impl RockFormation {
    pub(crate) fn site(terrain: &FbmTerrain, site: [f64; 2]) -> [Self; 3] {
        [
            FormationKind::Overhang,
            FormationKind::Grotto,
            FormationKind::Arch,
        ]
        .map(|kind| {
            let index = kind as u32;
            let x = site[0] + (f64::from(index) - 1.) * 14.;
            let z = site[1] + [1., 3., 0.][index as usize];
            let y = [-4., 0., 4.]
                .into_iter()
                .flat_map(|dx| [-4., 0., 4.].map(|dz| terrain.ground_height_m(x + dx, z + dz)))
                .fold(f64::NEG_INFINITY, f64::max);
            let phase = f64::from(terrain.seed().rotate_left(index * 11) as u16) / 65536.;
            let min = Cell::from_meters([x - 6., y - 4., z - 6.]);
            let max = Cell::from_meters([
                x + 6. + voxel_size(),
                y + 7. + voxel_size(),
                z + 6. + voxel_size(),
            ]);
            Self {
                kind,
                center: [x, y, z],
                phase,
                min,
                max,
            }
        })
    }

    pub fn kind(self) -> FormationKind {
        self.kind
    }
    pub fn center_m(self) -> [f64; 3] {
        self.center
    }
    pub fn bounds(self) -> (Cell, Cell) {
        (self.min, self.max)
    }

    pub(crate) fn intersects_columns(self, min: [i64; 2], max: [i64; 2]) -> bool {
        min[0] < self.max.x && max[0] > self.min.x && min[1] < self.max.z && max[1] > self.min.z
    }

    pub fn contains(self, c: Cell) -> bool {
        if c.x < self.min.x
            || c.y < self.min.y
            || c.z < self.min.z
            || c.x >= self.max.x
            || c.y >= self.max.y
            || c.z >= self.max.z
        {
            return false;
        }
        let [x, y, z] = c.center_m();
        let x = x - self.center[0];
        let y = y - self.center[1];
        let z = z - self.center[2];
        // Shift adjacent bedding layers along the strike and vary the shape
        // along all three axes. These are physical breaks in the rock.
        let layer = (y + 0.16 * x - 0.10 * z + self.phase).div_euclid(0.58);
        let joint = ((layer * 0.618_034 + self.phase).rem_euclid(1.) - 0.5) * 0.44;
        let u = x + joint - 0.12 * z;
        let v = y;
        let w = z + 0.20 * x;
        let sq = |a: f64| a * a;
        match self.kind {
            FormationKind::Arch => {
                let shell = sq(u / 4.6) + sq((v - 0.65).max(0.) / 5.2) + sq(w / (2.15 + 0.18 * u));
                let gap = sq((u + 0.16 * w - 0.24) / 2.65) + sq((v - 0.18).max(0.) / 3.54);
                v >= -3.4 && shell < 1. && gap >= 1.
            }
            FormationKind::Grotto => {
                let shell = sq(u / 5.1) + sq((v - 0.12).max(0.) / 5.3) + sq((w - 0.35) / 5.2);
                let center = 0.32 * (w + 0.6) + 0.17 * w.abs();
                let hollow = sq((u - center) / 2.28) + sq((v - 1.35) / 1.68);
                let back = w < 2.9 - 0.20 * u + 0.28 * v;
                v >= -3.4 && shell < 1. && !(back && hollow < 1.)
            }
            FormationKind::Overhang => {
                let base = sq((u + 0.6) / 3.45) + sq((w - 1.82) / 2.30);
                let lip = (2.52 + 0.14 * u - 0.19 * w + joint).max(1.78);
                let slab = sq(u / 5.0) + sq((w + 0.42 + 0.18 * u) / 4.6);
                let crest = 4.88 - 1.17 * slab + joint;
                (v >= -3.4 && v <= lip + 0.5 && base < 1.) || (slab < 1. && v > lip && v < crest)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FbmParams, FbmRecipe, Material, World};

    #[test]
    fn seeded_arch_keeps_native_air_below_its_roof() {
        let world = World::fbm(
            83,
            FbmParams {
                recipe: FbmRecipe::Landforms,
                ..Default::default()
            },
        )
        .unwrap()
        .with_formation_site([-4200., 1800.]);
        let arch = world.rock_formations()[2];
        let [x, y, z] = arch.center_m();
        assert_eq!(
            world.material(Cell::from_meters([x, y + 1.7, z])),
            Material::Air
        );
        assert_eq!(
            world.material(Cell::from_meters([x, y + 4.4, z])),
            Material::Stone
        );
        assert!(crate::raycast_solid(&world, [x, y + 1.7, z], [0., 1., 0.], 6.).is_some());
    }
}
