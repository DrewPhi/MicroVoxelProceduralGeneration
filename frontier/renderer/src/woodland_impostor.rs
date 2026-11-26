//! Orthographic depth and pigment views generated from sparse crown voxels.
//! Each texel averages four silhouette samples. Color, normal and depth are
//! premultiplied by coverage to keep filtered outlines from turning black.
use crate::woodland_volume::{CrownAtlasOptions, WoodlandVolume};
use glam::{Vec2, Vec3};

pub(super) const WIDTH: u32 = 32;
pub(super) const HEIGHT: u32 = 56;
pub(super) const AZIMUTHS: u32 = 8;
const ELEVATIONS: [f32; 4] = [-0.38, 0.02, 0.53, 1.11];
pub(super) const VIEWS: u32 = AZIMUTHS * ELEVATIONS.len() as u32;

#[derive(Clone, Debug)]
pub(super) struct CrownAtlas {
    pub color: Vec<u8>,
    pub surface: Vec<u8>,
    pub mip_levels: u32,
    pub layers: u32,
}
impl CrownAtlas {
    pub fn bytes(&self) -> usize {
        self.color.len() + self.surface.len()
    }
}

fn append_mip_chain(data: &mut Vec<u8>, begin: usize, levels: u32) {
    let (mut width, mut height) = (WIDTH as usize, HEIGHT as usize);
    let mut source = begin;
    for _ in 1..levels {
        let next = data.len();
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(2) {
                let first = source + (x + y * width) * 4;
                let second = first + width * 4;
                let pixel: [u8; 4] = std::array::from_fn(|c| {
                    let sum = u16::from(data[first + c])
                        + u16::from(data[first + 4 + c])
                        + u16::from(data[second + c])
                        + u16::from(data[second + 4 + c]);
                    ((sum + 2) / 4) as u8
                });
                data.extend(pixel);
            }
        }
        source = next;
        width /= 2;
        height /= 2;
    }
}

#[derive(Clone, Copy)]
struct Voxel {
    p: [u32; 3],
    exposed: u8,
    paint: u32,
    illumination: f32,
}

#[derive(Clone, Copy)]
struct Fragment {
    depth: f32,
    face: u8,
    paint: u32,
    illumination: f32,
}
impl Default for Fragment {
    fn default() -> Self {
        Self {
            depth: f32::NEG_INFINITY,
            face: 0,
            paint: 0,
            illumination: 1.,
        }
    }
}

struct Projector {
    view: Vec3,
    right: Vec3,
    up: Vec3,
    half: Vec3,
    scale: Vec2,
    scale_z: f32,
}
impl Projector {
    fn new(yaw: f32, pitch: f32, dims: [u32; 3], width: usize, height: usize) -> Self {
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let view = Vec3::new(cy * cp, sp, sy * cp);
        let right = Vec3::new(sy, 0., -cy);
        let up = view.cross(right);
        let half = Vec3::from_array(dims.map(|n| n as f32)) * 0.5;
        Self {
            view,
            right,
            up,
            half,
            // A one-texel transparent border ensures bilinear filtering
            // cannot replicate an opaque silhouette across an atlas edge.
            scale: Vec2::new(
                (width - 4) as f32 / (2. * right.abs().dot(half)),
                (height - 4) as f32 / (2. * up.abs().dot(half)),
            ),
            scale_z: view.abs().dot(half),
        }
    }
    fn plane(&self, p: Vec3) -> Vec2 {
        Vec2::new(p.dot(self.right), -p.dot(self.up)) * self.scale
    }
}

impl CrownAtlas {
    pub fn build(
        volume: &WoodlandVolume,
        options: CrownAtlasOptions,
        cancel: &impl Fn() -> bool,
    ) -> Option<Self> {
        let dims: [u32; 3] = volume.words[4..7].try_into().unwrap();
        let tile = dims.map(|v| v.div_ceil(4));
        let paint_at = |[x, y, z]: [u32; 3]| {
            let i = 8 + (x / 4 + tile[0] * (y / 4 + tile[1] * (z / 4))) as usize;
            let offset = volume.words[i] as usize;
            if offset == 0 {
                0
            } else {
                volume.words[offset + ((x % 4) + 4 * ((y % 4) + 4 * (z % 4))) as usize]
            }
        };
        let crown_occlusion = options.occlusion;
        let mut voxels = Vec::new();
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                if cancel() {
                    return None;
                }
                for x in 0..dims[0] {
                    let p = [x, y, z];
                    let paint = paint_at(p);
                    if paint == 0 {
                        continue;
                    }
                    let mut exposed = 0u8;
                    for axis in 0..3 {
                        for side in 0..2 {
                            let mut q = p;
                            q[axis] = if side == 0 {
                                p[axis].wrapping_sub(1)
                            } else {
                                p[axis] + 1
                            };
                            if q[axis] >= dims[axis] || paint_at(q) == 0 {
                                exposed |= 1 << (2 * axis + side);
                            }
                        }
                    }
                    if exposed != 0 {
                        let illumination = if crown_occlusion {
                            // Compact hemispheric exposure estimate from the same
                            // occupied crown cells. Leave open needle sprays lit
                            // and preserve darkness beneath successive tiers.
                            let mut blocked = 0.;
                            for (dx, dy, dz) in [
                                (-1, 1, 0),
                                (1, 1, 0),
                                (0, 1, -1),
                                (0, 1, 1),
                                (-2, 3, 0),
                                (2, 3, 0),
                                (0, 3, -2),
                                (0, 3, 2),
                                (0, 2, 0),
                                (0, 5, 0),
                            ] {
                                let q = [
                                    x.wrapping_add_signed(dx),
                                    y.wrapping_add_signed(dy),
                                    z.wrapping_add_signed(dz),
                                ];
                                if (0..3).all(|axis| q[axis] < dims[axis]) && paint_at(q) != 0 {
                                    blocked += 1.;
                                }
                            }
                            1. - 0.032 * blocked
                        } else {
                            1.
                        };
                        voxels.push(Voxel {
                            p,
                            exposed,
                            paint,
                            illumination,
                        });
                    }
                }
            }
        }
        let layers = VIEWS + u32::from(options.zenith);
        let n = (1..=options.mip_levels)
            .map(|level| (WIDTH >> (level - 1)) * (HEIGHT >> (level - 1)) * layers * 4)
            .sum::<u32>() as usize;
        let mut atlas = Self {
            color: Vec::with_capacity(n),
            surface: Vec::with_capacity(n),
            mip_levels: options.mip_levels,
            layers,
        };
        let width = WIDTH as usize * 2;
        let height = HEIGHT as usize * 2;
        let offset = Vec2::new(width as f32 * 0.5, height as f32 * 0.5);
        let mut samples = vec![Fragment::default(); width * height];
        let ordinary_views = ELEVATIONS.into_iter().flat_map(|pitch| {
            (0..AZIMUTHS).map(move |j| {
                let yaw = j as f32 * std::f32::consts::TAU / AZIMUTHS as f32;
                (yaw, pitch)
            })
        });
        let top_view = options.zenith.then_some((0., std::f32::consts::FRAC_PI_2));
        for (yaw, pitch) in ordinary_views.chain(top_view) {
            if cancel() {
                return None;
            }
            let p = Projector::new(yaw, pitch, dims, width, height);
            samples.fill(Fragment::default());
            for axis in 0..3 {
                if p.view[axis].abs() < 0.00001 {
                    continue;
                }
                let side = u8::from(p.view[axis] > 0.);
                let face = 2 * axis as u8 + side;
                let u = (axis + 1) % 3;
                let v = (axis + 2) % 3;
                let su = Vec2::new(p.right[u], -p.up[u]) * p.scale;
                let sv = Vec2::new(p.right[v], -p.up[v]) * p.scale;
                let det = 1. / su.perp_dot(sv);
                for voxel in voxels.iter().filter(|w| w.exposed & (1 << face) != 0) {
                    let mut local = Vec3::from_array(voxel.p.map(|c| c as f32)) - p.half;
                    local[axis] += f32::from(side);
                    let base = p.plane(local) + offset;
                    let range_lo = base + Vec2::ZERO.min(su) + Vec2::ZERO.min(sv);
                    let range_hi = base + Vec2::ZERO.max(su) + Vec2::ZERO.max(sv);
                    let lo = range_lo.floor().max(Vec2::ZERO);
                    let hi = range_hi.ceil().min(Vec2::new(width as f32, height as f32));
                    let base_depth = p.view.dot(local);
                    for y in lo.y as usize..hi.y as usize {
                        for x in lo.x as usize..hi.x as usize {
                            let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - base;
                            let a = q.perp_dot(sv) * det;
                            let b = su.perp_dot(q) * det;
                            if (0. ..=1.).contains(&a) && (0. ..=1.).contains(&b) {
                                let d = (base_depth + a * p.view[u] + b * p.view[v]) / p.scale_z;
                                let dst = &mut samples[x + y * width];
                                if d > dst.depth {
                                    *dst = Fragment {
                                        depth: d,
                                        face,
                                        paint: voxel.paint,
                                        illumination: voxel.illumination,
                                    };
                                }
                            }
                        }
                    }
                }
            }
            let start = atlas.color.len();
            for y in (0..height).step_by(2) {
                for x in (0..width).step_by(2) {
                    let i = x + y * width;
                    let mut rgb = Vec3::ZERO;
                    let mut normal = Vec3::ZERO;
                    let mut depth = 0.;
                    let mut hits = 0.;
                    for f in [
                        samples[i],
                        samples[i + 1],
                        samples[i + width],
                        samples[i + width + 1],
                    ] {
                        if f.paint != 0 {
                            rgb += Vec3::from_array(
                                [f.paint, f.paint >> 10, f.paint >> 20]
                                    .map(|c| f.illumination * (c & 1023) as f32 / 1023.),
                            );
                            let mut n = Vec3::ZERO;
                            n[usize::from(f.face / 2)] = if f.face & 1 == 0 { -1. } else { 1. };
                            normal += n * 0.5 + Vec3::splat(0.5);
                            depth += 0.5 * f.depth + 0.5;
                            hits += 1.;
                        }
                    }
                    let quantize = |v: f32| (v * (255. / 4.)).round().clamp(0., 255.) as u8;
                    atlas
                        .color
                        .extend(rgb.extend(hits).to_array().map(quantize));
                    atlas
                        .surface
                        .extend(normal.extend(depth).to_array().map(quantize));
                }
            }
            append_mip_chain(&mut atlas.color, start, options.mip_levels);
            append_mip_chain(&mut atlas.surface, start, options.mip_levels);
        }
        Some(atlas)
    }
}
