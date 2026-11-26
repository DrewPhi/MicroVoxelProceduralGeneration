//! Shared, sparse branch-and-leaf volume for distant crowns.
//!
//! The first eight words hold grid origin, native cells per sample, and
//! dimensions. A 4³ cell brick table follows. Its nonzero entries point to
//! 64 packed RGB/material samples, and empty bricks need no leaf storage.
use frontier_world::{vegetation::living_flora::WoodlandRecipe, Cell, Material};

#[derive(Clone, Debug)]
pub(super) struct WoodlandVolume {
    pub words: Vec<u32>,
    pub low: [i64; 3],
    pub high: [i64; 3],
    pub atlas: Option<crate::woodland_impostor::CrownAtlas>,
}

#[derive(Clone, Copy)]
pub(crate) struct CrownAtlasOptions {
    pub(crate) mip_levels: u32,
    pub(crate) occlusion: bool,
    pub(crate) triangular: bool,
    pub(crate) projected_footprint: bool,
    pub(crate) zenith: bool,
}

#[derive(Clone, Copy, Default)]
pub(crate) enum CrownDisplay {
    #[default]
    Tiered,
    Raycast(i32),
    Atlas(CrownAtlasOptions),
}

impl CrownDisplay {
    pub(crate) fn choose(living_woods: bool) -> Self {
        let atlas = || {
            Self::Atlas(CrownAtlasOptions {
                mip_levels: if render_option("FRONTIER_WOODLAND_ATLAS_MIPS", living_woods) {
                    4
                } else {
                    1
                },
                occlusion: render_option("FRONTIER_WOODLAND_CROWN_OCCLUSION", living_woods),
                triangular: render_option("FRONTIER_WOODLAND_ATLAS_TRIANGLE", false),
                projected_footprint: render_option(
                    "FRONTIER_WOODLAND_ATLAS_PROJECTED_FOOTPRINT",
                    living_woods,
                ),
                zenith: render_option("FRONTIER_WOODLAND_ATLAS_ZENITH", false),
            })
        };
        match std::env::var("FRONTIER_WOODLAND_VOLUME").as_deref() {
            Ok("atlas") => atlas(),
            Ok("on" | "6") => Self::Raycast(6),
            Ok("4") => Self::Raycast(4),
            Ok("8") => Self::Raycast(8),
            Err(_) if living_woods => atlas(),
            _ => Self::Tiered,
        }
    }

    pub(crate) fn step(self) -> Option<i32> {
        match self {
            Self::Tiered => None,
            Self::Raycast(step) => Some(step),
            Self::Atlas(_) => Some(6),
        }
    }

    pub(crate) fn atlas(self) -> Option<CrownAtlasOptions> {
        if let Self::Atlas(options) = self {
            Some(options)
        } else {
            None
        }
    }
}

pub(crate) fn render_option(name: &str, default: bool) -> bool {
    match std::env::var(name).as_deref() {
        Ok("0" | "off") => false,
        Ok("1" | "on") => true,
        _ => default,
    }
}

fn pigment(rgb: [u8; 3], material: Material) -> u32 {
    let [r, g, b] = rgb.map(|v| {
        let c = f64::from(v) / 255.;
        let linear = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
        (linear * 1023.).round() as u32
    });
    r | (g << 10) | (b << 20) | (u32::from(material == Material::Leaf) << 30) | (1 << 31)
}

/// Dense scratch for the bounded, coarsely sampled recipe. Only populated
/// 4³ bricks survive publication.
struct CrownSamples {
    low: [i32; 3],
    size: [usize; 3],
    values: Vec<u32>,
    filled_low: [i32; 3],
    filled_high: [i32; 3],
}

impl CrownSamples {
    fn new(recipe: &WoodlandRecipe, pitch: f64) -> Option<Self> {
        let floor = |v: f64| (v / pitch).floor() as i32;
        let mut bounds = ([i32::MAX; 3], [i32::MIN; 3]);
        let mut extend = |lo: [i32; 3], hi: [i32; 3]| {
            for axis in 0..3 {
                bounds.0[axis] = bounds.0[axis].min(lo[axis]);
                bounds.1[axis] = bounds.1[axis].max(hi[axis]);
            }
        };
        for limb in &recipe.wood {
            let radius = (f64::from(limb.radius) - pitch * 0.24).max(0.);
            let r = [radius, 0., radius];
            extend(
                std::array::from_fn(|i| {
                    floor(f64::from(limb.from[i].min(limb.to[i])) + 0.5 - r[i])
                }),
                std::array::from_fn(|i| {
                    floor(f64::from(limb.from[i].max(limb.to[i])) + 0.5 + r[i])
                }),
            );
        }
        for group in &recipe.foliage {
            let center = group.center.map(|s| f64::from(s) + 0.5);
            let radius = group.radius.map(|r| r + 0.27 * pitch);
            extend(
                std::array::from_fn(|i| floor(center[i] - radius[i])),
                std::array::from_fn(|i| floor(center[i] + radius[i])),
            );
        }
        if bounds.0[0] > bounds.1[0] {
            return None;
        }
        let size = std::array::from_fn(|i| (bounds.1[i] - bounds.0[i] + 1) as usize);
        let len = size.into_iter().try_fold(1usize, usize::checked_mul)?;
        Some(Self {
            low: bounds.0,
            size,
            values: vec![0; len],
            filled_low: [i32::MAX; 3],
            filled_high: [i32::MIN; 3],
        })
    }

    fn index(&self, coord: [i32; 3]) -> usize {
        let [x, y, z] = std::array::from_fn(|i| (coord[i] - self.low[i]) as usize);
        x + self.size[0] * (y + self.size[1] * z)
    }

    fn insert(&mut self, coord: [i32; 3], paint: u32, overwrite: bool) {
        let index = self.index(coord);
        let previous = self.values[index];
        if previous == 0 {
            for (axis, value) in coord.into_iter().enumerate() {
                self.filled_low[axis] = self.filled_low[axis].min(value);
                self.filled_high[axis] = self.filled_high[axis].max(value + 1);
            }
        }
        if overwrite || previous == 0 {
            self.values[index] = paint;
        }
    }
}

impl WoodlandVolume {
    pub fn from_recipe(
        recipe: &WoodlandRecipe,
        step: i32,
        atlas: Option<CrownAtlasOptions>,
        cancel: &impl Fn() -> bool,
    ) -> Option<Self> {
        let pitch = f64::from(step);
        let mut cells = CrownSamples::new(recipe, pitch)?;
        let wood = pigment(recipe.bark, Material::Wood);
        let mut twigs = Vec::new();
        for (i, limb) in recipe.wood.iter().enumerate() {
            if i % 16 == 0 && cancel() {
                return None;
            }
            let a = limb.from.map(f64::from);
            let d: [f64; 3] = std::array::from_fn(|axis| f64::from(limb.to[axis]) - a[axis]);
            let samples = d.iter().map(|v| v.abs()).fold(1., f64::max).ceil() as i32;
            // Retain narrow branches while avoiding full-width shadow slabs.
            let radius = (f64::from(limb.radius) - pitch * 0.24).max(0.);
            let offsets = [
                [0., 0.],
                [radius, 0.],
                [-radius, 0.],
                [0., radius],
                [0., -radius],
            ];
            for j in 0..=samples {
                let t = f64::from(j) / f64::from(samples);
                let p: [f64; 3] = std::array::from_fn(|axis| a[axis] + d[axis] * t + 0.5);
                for [dx, dz] in offsets {
                    let coord = [p[0] + dx, p[1], p[2] + dz].map(|v| (v / pitch).floor() as i32);
                    if i < recipe.trunk_strokes {
                        cells.insert(coord, wood, true);
                    } else {
                        twigs.push(coord);
                    }
                }
            }
        }
        for (i, group) in recipe.foliage.iter().enumerate() {
            if i % 24 == 0 && cancel() {
                return None;
            }
            let center = group.center.map(|c| f64::from(c) + 0.5);
            // Filter small clusters before sampling the shared crown. A box
            // touching an ellipsoid's edge alone does not become solid.
            let radius = group.radius.map(|r| r + 0.27 * pitch);
            let lo: [i32; 3] =
                std::array::from_fn(|i| ((center[i] - radius[i]) / pitch).floor() as i32);
            let hi: [i32; 3] =
                std::array::from_fn(|i| ((center[i] + radius[i]) / pitch).floor() as i32);
            let main_cell = center.map(|c| (c / pitch).floor() as i32);
            let color = pigment(group.pigment, Material::Leaf);
            for x in lo[0]..=hi[0] {
                for y in lo[1].max(0)..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        let p = [x, y, z];
                        let d = (0..3)
                            .map(|a| {
                                (((f64::from(p[a]) + 0.5) * pitch - center[a]) / radius[a]).powi(2)
                            })
                            .sum::<f64>();
                        if p == main_cell || d <= 1. {
                            cells.insert(p, color, false);
                        }
                    }
                }
            }
        }
        for cell in twigs {
            cells.insert(cell, wood, false);
        }
        let (lo, hi) = (cells.filled_low, cells.filled_high);
        if lo[0] >= hi[0] || cancel() {
            return None;
        }
        let dims: [u32; 3] = std::array::from_fn(|i| (hi[i] - lo[i]) as u32);
        let tiles = dims.map(|n| n.div_ceil(4));
        let count = tiles.iter().product::<u32>() as usize;
        let mut words = Vec::with_capacity(8 + count + cells.values.len() / 4);
        words.extend(lo.map(|i| i as u32));
        words.push(step as u32);
        words.extend(dims);
        words.push(0);
        words.resize(8 + count, 0);
        // Preserve the x/y/z visitation order, including stable brick layout.
        for x in lo[0]..hi[0] {
            if cancel() {
                return None;
            }
            for y in lo[1]..hi[1] {
                for z in lo[2]..hi[2] {
                    let coord = [x, y, z];
                    let paint = cells.values[cells.index(coord)];
                    if paint == 0 {
                        continue;
                    }
                    let p: [u32; 3] = std::array::from_fn(|i| (coord[i] - lo[i]) as u32);
                    let [x, y, z] = p.map(|v| v / 4);
                    let index = 8 + (x + tiles[0] * (y + tiles[1] * z)) as usize;
                    let mut address = words[index] as usize;
                    if address == 0 {
                        address = words.len();
                        words[index] = address as u32;
                        words.resize(address + 64, 0);
                    }
                    let [x, y, z] = p.map(|v| v & 3);
                    words[address + (x + 4 * (y + 4 * z)) as usize] = paint;
                }
            }
        }
        words.shrink_to_fit();
        let mut result = Self {
            words,
            low: lo.map(|v| i64::from(v * step)),
            high: hi.map(|v| i64::from(v * step)),
            atlas: None,
        };
        if let Some(options) = atlas {
            result.atlas = Some(crate::woodland_impostor::CrownAtlas::build(
                &result, options, cancel,
            )?);
            result.words.truncate(8);
            result.words.shrink_to_fit();
        }
        Some(result)
    }

    pub fn bytes(&self) -> usize {
        self.words.len() * 4 + self.atlas.as_ref().map_or(0, |a| a.bytes())
    }

    pub fn coarse_cells(&self) -> impl Iterator<Item = (Cell, u8, [f32; 3])> + '_ {
        let dims: [u32; 3] = self.words[4..7].try_into().unwrap();
        let tiles = dims.map(|d| d.div_ceil(4));
        let pitch = i64::from(self.words[3]);
        let count = tiles.into_iter().product::<u32>() as usize;
        self.words[8..8 + count]
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, address)| *address > 0)
            .flat_map(move |(index, address)| {
                let i = index as u32;
                let origin = [
                    i % tiles[0],
                    (i / tiles[0]) % tiles[1],
                    i / (tiles[0] * tiles[1]),
                ]
                .map(|v| i64::from(v) * 4);
                self.words[address as usize..address as usize + 64]
                    .iter()
                    .copied()
                    .enumerate()
                    .filter_map(move |(index, paint)| {
                        if paint == 0 {
                            return None;
                        }
                        let local = [index % 4, (index / 4) % 4, index / 16].map(|n| n as i64);
                        let [x, y, z] =
                            std::array::from_fn(|a| self.low[a] + (origin[a] + local[a]) * pitch);
                        let mat = if paint & (1 << 30) != 0 {
                            Material::Leaf
                        } else {
                            Material::Wood
                        };
                        let rgb = [paint, paint >> 10, paint >> 20].map(|v| {
                            let linear = (v & 1023) as f32 / 1023.;
                            (linear * 128.).round() / 128.
                        });
                        Some((Cell { x, y, z }, mat as u8, rgb))
                    })
            })
    }
}
