//! Original meter-scale character recipe, sampled at voxel centers after posing.
//! Continuous math is authoring machinery; the output contains whole cells only.
use super::{Clip, Pose, VoxelFrame};
use crate::{
    materials::{Appearance, MaterialId, MaterialKind, MaterialRegistry, Voxel},
    painting::hash,
};
#[derive(Clone, Debug)]
pub struct CharacterRecipe {
    pub seed: u64,
}
impl Default for CharacterRecipe {
    fn default() -> Self {
        Self { seed: 42 }
    }
}
#[derive(Clone, Copy)]
#[repr(usize)]
enum Paint {
    Skin,
    Hair,
    Shirt,
    Trousers,
    Boots,
    White,
    Dark,
    Iris,
    Scarf,
    Trim,
    Cheek,
}
const PAINTS: usize = 11;
/// Palette IDs are allocated into a registry, so later world integration can
/// share ordinary voxel/material values without hard-coded new global IDs.
pub struct CharacterPalette {
    ids: [MaterialId; PAINTS],
}
impl CharacterRecipe {
    pub fn register_palette(
        &self,
        registry: &mut MaterialRegistry,
    ) -> Result<CharacterPalette, String> {
        let shirts = [
            [49, 142, 146],
            [62, 130, 177],
            [103, 145, 96],
            [163, 105, 129],
        ];
        let shirt = shirts[(hash(self.seed) & 3) as usize];
        let entries = [
            ("skin", [226, 173, 126]),
            ("hair", [77, 49, 37]),
            ("shirt", shirt),
            ("trousers", [47, 64, 86]),
            ("boots", [77, 61, 48]),
            ("eye_white", [244, 239, 218]),
            ("dark", [37, 35, 43]),
            ("iris", [51, 100, 110]),
            ("scarf", [232, 155, 67]),
            ("trim", [163, 202, 191]),
            ("cheek", [211, 133, 112]),
        ];
        let mut ids = [MaterialId::AIR; PAINTS];
        for (i, (name, rgb)) in entries.into_iter().enumerate() {
            let id = registry
                .register(
                    format!("character/{}/{name}", self.seed),
                    MaterialKind::Solid,
                )
                .ok_or("material registry full")?;
            registry.get_mut(id).unwrap().palette = [rgb, rgb];
            ids[i] = id;
        }
        Ok(CharacterPalette { ids })
    }
    pub fn generate(
        &self,
        cells_per_meter: u16,
        pose: Pose,
        palette: &CharacterPalette,
    ) -> Result<VoxelFrame, String> {
        if !(4..=128).contains(&cells_per_meter) {
            return Err("character resolution must be 4..128 cells/m".into());
        }
        let r = f64::from(cells_per_meter);
        let half = if pose.arm_forward {
            i32::from(cells_per_meter) * 7 / 8
        } else {
            i32::from(cells_per_meter) * 11 / 16
        };
        let origin = [-half, 0, -half];
        let size = [
            (half * 2) as usize,
            usize::from(cells_per_meter) * if pose.arm_forward { 18 } else { 15 } / 8,
            (half * 2) as usize,
        ];
        let mut frame = VoxelFrame {
            cells_per_meter,
            origin,
            size,
            voxels: vec![Voxel::AIR; size.iter().product()],
            right_arm_cells: Vec::new(),
        };
        let mut owners = vec![false; frame.voxels.len()];
        let parts = body(pose);
        // Rasterize each small part's transformed AABB instead of evaluating
        // every body primitive at every point in the enclosing volume.
        let heading = f64::from(pose.heading % 16) * std::f64::consts::TAU / 16.;
        let (sin, cos) = heading.sin_cos();
        for part in parts {
            let (low, high) = part.bounds(sin, cos);
            let min: [i32; 3] =
                std::array::from_fn(|a| ((low[a] * r).floor() as i32).max(origin[a]));
            let max: [i32; 3] = std::array::from_fn(|a| {
                ((high[a] * r).ceil() as i32).min(origin[a] + size[a] as i32)
            });
            for z in min[2]..max[2] {
                for y in min[1]..max[1] {
                    for x in min[0]..max[0] {
                        let world = [
                            (f64::from(x) + 0.5) / r,
                            (f64::from(y) + 0.5) / r,
                            (f64::from(z) + 0.5) / r,
                        ];
                        let facing = [
                            cos * world[0] - sin * world[2],
                            world[1],
                            sin * world[0] + cos * world[2],
                        ];
                        let local = part.local(facing);
                        if part.contains(local) {
                            let paint = part.paint(local, cells_per_meter, pose, self.seed);
                            let index = frame.index([x, y, z]).unwrap();
                            owners[index] = part.right_arm;
                            frame.voxels[index] =
                                Voxel::new(palette.ids[paint as usize], Appearance(0x8080));
                        }
                    }
                }
            }
        }
        frame.right_arm_cells = owners
            .into_iter()
            .enumerate()
            .filter_map(|(i, arm)| arm.then_some(i))
            .collect();
        Ok(frame)
    }
}
#[derive(Clone, Copy)]
enum Kind {
    Head,
    Torso,
    Limb(Paint),
    Shoe,
}
struct Part {
    center: [f64; 3],
    half: [f64; 3],
    angle: f64,
    yaw: f64,
    right_arm: bool,
    kind: Kind,
}
impl Part {
    fn local(&self, p: [f64; 3]) -> [f64; 3] {
        let p = std::array::from_fn::<_, 3, _>(|a| p[a] - self.center[a]);
        let (sy, cy) = self.yaw.sin_cos();
        let p = [cy * p[0] - sy * p[2], p[1], sy * p[0] + cy * p[2]];
        let (s, c) = self.angle.sin_cos();
        [p[0], c * p[1] + s * p[2], -s * p[1] + c * p[2]]
    }
    fn contains(&self, p: [f64; 3]) -> bool {
        // Beveled boxes keep a chunky silhouette; there is no smooth surface mesh.
        let bevel = 0.025f64.min(self.half.iter().copied().fold(f64::INFINITY, f64::min) * 0.35);
        let q = std::array::from_fn::<_, 3, _>(|a| p[a].abs() - (self.half[a] - bevel));
        q.iter().map(|v| v.max(0.).powi(2)).sum::<f64>().sqrt()
            + q.iter().copied().fold(f64::NEG_INFINITY, f64::max).min(0.)
            <= bevel
    }
    fn bounds(&self, sin: f64, cos: f64) -> ([f64; 3], [f64; 3]) {
        let mut low = [f64::INFINITY; 3];
        let mut high = [f64::NEG_INFINITY; 3];
        let (s, c) = self.angle.sin_cos();
        for i in 0..8 {
            let p: [f64; 3] =
                std::array::from_fn(|a| self.half[a] * if i & (1 << a) == 0 { -1. } else { 1. });
            let rotated = [p[0], c * p[1] - s * p[2], s * p[1] + c * p[2]];
            let (sy, cy) = self.yaw.sin_cos();
            let posed = [
                cy * rotated[0] + sy * rotated[2] + self.center[0],
                rotated[1] + self.center[1],
                -sy * rotated[0] + cy * rotated[2] + self.center[2],
            ];
            let world = [
                cos * posed[0] + sin * posed[2],
                posed[1],
                -sin * posed[0] + cos * posed[2],
            ];
            for a in 0..3 {
                low[a] = low[a].min(world[a]);
                high[a] = high[a].max(world[a]);
            }
        }
        (low, high)
    }
    fn paint(&self, p: [f64; 3], resolution: u16, pose: Pose, seed: u64) -> Paint {
        match self.kind {
            Kind::Head => head_paint(p, resolution, pose, seed),
            Kind::Shoe => {
                if p[1] < -0.035 {
                    Paint::Dark
                } else if resolution >= 32 && p[2] < -0.06 && p[1] > 0.025 && (p[0].abs() < 0.02) {
                    Paint::Trim
                } else {
                    Paint::Boots
                }
            }
            Kind::Torso => {
                if p[1] < -0.22 {
                    return Paint::Trousers;
                }
                if p[1] > 0.14 {
                    return Paint::Scarf;
                }
                if p[2] < -0.09 && p[0].abs() < 0.035 && p[1] > -0.10 {
                    return Paint::Trim;
                }
                if resolution >= 32
                    && p[2] < -0.09
                    && (p[0] - 0.13).abs() < 0.055
                    && (p[1] - 0.01).abs() < 0.05
                {
                    return Paint::Trim;
                }
                Paint::Shirt
            }
            Kind::Limb(paint) => paint,
        }
    }
}
fn head_paint(p: [f64; 3], resolution: u16, pose: Pose, seed: u64) -> Paint {
    let [x, y, z] = p;
    let fringe = 0.12 + 0.025 * (x * 18. + (hash(seed) & 7) as f64).sin();
    if y > fringe || z > 0.12 || (x.abs() > 0.24 && y > 0.02) {
        return Paint::Hair;
    }
    if z < -0.25 + 1. / f64::from(resolution) {
        let eye_center = if resolution == 8 { 0.1875 } else { 0.125 };
        let eye = (x.abs() - eye_center).abs();
        if eye < 0.05 && (y - 0.01).abs() < 0.042 {
            if pose.clip == Clip::Idle && pose.frame % 8 == 6 {
                return Paint::Skin;
            }
            if resolution == 8 {
                return Paint::Dark;
            }
            if resolution == 16 {
                return if x.abs() < 0.125 {
                    Paint::Dark
                } else {
                    Paint::White
                };
            }
            if eye < 0.023 {
                return if resolution >= 64 && y > 0.025 {
                    Paint::White
                } else {
                    Paint::Iris
                };
            }
            return Paint::White;
        }
        if resolution >= 32 && (x.abs() - 0.125).abs() < 0.055 && (y - 0.085).abs() < 0.015 {
            return Paint::Hair;
        }
        if resolution >= 32 && (x.abs() - 0.20).abs() < 0.03 && (y + 0.065).abs() < 0.025 {
            return Paint::Cheek;
        }
        if x.abs() < 0.07 && (y + 0.13).abs() < 0.022 {
            return Paint::Dark;
        }
    }
    Paint::Skin
}
fn segment(a: [f64; 3], b: [f64; 3], width: f64, depth: f64, paint: Paint) -> Part {
    let delta = std::array::from_fn::<_, 3, _>(|i| b[i] - a[i]);
    let length = delta.iter().map(|v| v * v).sum::<f64>().sqrt().max(0.0001);
    Part {
        center: std::array::from_fn(|i| (a[i] + b[i]) * 0.5),
        half: [width * 0.5, length * 0.5 + 0.035, depth * 0.5],
        angle: (-delta[1] / length).clamp(-1., 1.).acos(),
        yaw: (-delta[0]).atan2(-delta[2]),
        right_arm: false,
        kind: Kind::Limb(paint),
    }
}
fn body(pose: Pose) -> Vec<Part> {
    let mut parts = vec![Part {
        center: [0., 0.96, 0.],
        half: [0.23, 0.29, 0.125],
        angle: 0.,
        yaw: 0.,
        right_arm: false,
        kind: Kind::Torso,
    }];
    for side in [-1., 1.] {
        let phase = pose.phase() + if side < 0. { 0. } else { std::f64::consts::PI };
        let walking = pose.clip == Clip::Walk;
        let lift = if walking {
            phase.sin().max(0.) * 0.12
        } else {
            0.
        };
        let swing = if walking { phase.cos() * 0.19 } else { 0. };
        let hip = [side * 0.125, 0.70, 0.];
        let ankle = [side * 0.125, 0.13 + lift, swing];
        let knee = [
            side * 0.125,
            (hip[1] + ankle[1]) * 0.5,
            swing * 0.5 - if walking { 0.055 } else { 0. },
        ];
        parts.push(segment(hip, knee, 0.19, 0.20, Paint::Trousers));
        parts.push(segment(knee, ankle, 0.175, 0.18, Paint::Trousers));
        parts.push(Part {
            center: [side * 0.125, 0.075 + lift, swing - 0.04],
            half: [0.105, 0.075, 0.16],
            angle: 0.,
            yaw: 0.,
            right_arm: false,
            kind: Kind::Shoe,
        });
        let arm_swing = if walking { -swing } else { 0. };
        let shoulder = [side * 0.28, 1.18, 0.];
        let mut elbow = [side * 0.335, 0.99, arm_swing * 0.45];
        let mut hand = [side * 0.35, 0.78, arm_swing];
        if pose.clip == Clip::Wave && side > 0. {
            elbow = [0.43, 1.36, 0.];
            hand = [0.50 + 0.045 * pose.phase().sin(), 1.67, -0.02];
        } else if pose.clip == Clip::Jump {
            elbow = [side * 0.40, 1.26, 0.];
            hand = [side * 0.49, 1.48, 0.];
        }
        if pose.arm_forward && side > 0. {
            let yaw = f64::from(pose.head_yaw) * std::f64::consts::TAU / 32.;
            let pitch = f64::from(pose.head_pitch) * 7.5f64.to_radians();
            let (sy, cy) = yaw.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            // Aim in the eye's right/forward/up basis, then voxelize. This keeps
            // the actual hand in the lower-right view when looking up/down.
            hand = [
                0.30 * cy - 0.60 * cp * sy - 0.28 * sp * sy,
                1.50 + 0.60 * sp - 0.28 * cp,
                -0.30 * sy - 0.60 * cp * cy - 0.28 * sp * cy,
            ];
            elbow = std::array::from_fn(|i| shoulder[i] * 0.5 + hand[i] * 0.5);
            elbow[1] -= 0.06;
        }
        // Centers bridge sideward displacement with overlapping chunky joints.
        let mut upper = segment(shoulder, elbow, 0.19, 0.19, Paint::Shirt);
        // Hide the shoulder/sleeve in first person, like a forearm viewmodel.
        // The full arm remains part of the world model and collision shape.
        upper.right_arm = false;
        let mut lower = segment(elbow, hand, 0.17, 0.17, Paint::Skin);
        lower.right_arm = side > 0.;
        parts.push(upper);
        parts.push(lower);
    }
    // Neck/head are added last, giving deterministic overlap ownership.
    parts.push(Part {
        center: [0., 1.24, 0.],
        half: [0.105, 0.10, 0.095],
        angle: 0.,
        yaw: 0.,
        right_arm: false,
        kind: Kind::Limb(Paint::Skin),
    });
    let yaw = f64::from(pose.head_yaw) * std::f64::consts::TAU / 32.;
    let pitch = f64::from(pose.head_pitch) * 7.5f64.to_radians();
    parts.push(Part {
        center: [0., 1.46, 0.],
        half: [0.29, 0.29, 0.25],
        angle: pitch,
        yaw,
        right_arm: false,
        kind: Kind::Head,
    });
    parts
}
