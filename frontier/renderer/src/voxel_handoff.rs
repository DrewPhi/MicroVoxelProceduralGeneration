//! Offline dry-heightfield representation experiment for #83. This is not a
//! live LOD path or a screen-space certificate. Integer macrocolumn tops and
//! every exposed unit side face are compared with the original native surface.
use frontier_world::{World, voxel_size};
const MAX_WIDTH: usize = 128;
const MAX_FACES: usize = 300_000;
#[derive(Clone)]
pub(crate) struct Field {
    min: [i64; 2],
    width: usize,
    // One canonical column halo, never an artificial bottom/crop cap.
    heights: Vec<i64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Face {
    cell: [i64; 3],
    axis: usize,
    positive: bool,
}
#[derive(Clone, Copy)]
struct Triangle {
    p: [[f64; 3]; 3],
    normal: [f64; 3],
}
impl Field {
    fn sample(
        min: [i64; 2],
        width: usize,
        mut height: impl FnMut(i64, i64) -> Result<i64, String>,
        cancel: &mut impl FnMut() -> bool,
    ) -> Result<Self, String> {
        if width == 0 || width > MAX_WIDTH || min.iter().any(|v| v.unsigned_abs() > 1 << 32) {
            return Err("handoff prototype exceeds bounded local coordinates/width".into());
        }
        let mut heights = Vec::with_capacity((width + 2).pow(2));
        for z in -1..=width as i64 {
            if cancel() {
                return Err("cancelled handoff experiment".into());
            }
            for x in -1..=width as i64 {
                let h = height(min[0] + x, min[1] + z)?;
                if h.unsigned_abs() > 1 << 48 {
                    return Err("handoff height exceeds integer precision limit".into());
                }
                heights.push(h);
            }
        }
        Ok(Self {
            min,
            width,
            heights,
        })
    }
    pub(crate) fn from_world(
        w: &World,
        min: [i64; 2],
        width: usize,
        cancel: &mut impl FnMut() -> bool,
    ) -> Result<Self, String> {
        if w.fbm_terrain().is_none() || w.topology_base_cells().is_some() || w.revision() != 0 {
            return Err(
                "handoff experiment requires unedited FBM heightfield; volumes/edits are excluded"
                    .into(),
            );
        }
        Self::sample(
            min,
            width,
            |x, z| {
                let column = w.column(x, z);
                if column.height_m() != column.ground_height_m() {
                    return Err("handoff experiment excludes water columns; no opaque replacement for water".into());
                }
                Ok(column.height_cells())
            },
            cancel,
        )
    }
    fn height(&self, x: i64, z: i64) -> i64 {
        self.heights[((z + 1) as usize) * (self.width + 2) + (x + 1) as usize]
    }
    fn set(&mut self, x: i64, z: i64, h: i64) {
        self.heights[((z + 1) as usize) * (self.width + 2) + (x + 1) as usize] = h;
    }
    fn stairs(&self, step: usize, cancel: &mut impl FnMut() -> bool) -> Result<Self, String> {
        if ![2, 4, 8].contains(&step)
            || self.width % step != 0
            || self.min.iter().any(|v| v.rem_euclid(step as i64) != 0)
        {
            return Err("handoff macrocolumns require globally aligned step2/4/8".into());
        }
        let mut out = self.clone();
        // A full native perimeter preserves the original top/side ownership
        // against a native or2:1 neighbor. This does not certify a live handoff.
        for z in 1..self.width as i64 - 1 {
            if cancel() {
                return Err("cancelled handoff experiment".into());
            }
            for x in 1..self.width as i64 - 1 {
                let sx = x / step as i64 * step as i64 + step as i64 / 2;
                let sz = z / step as i64 * step as i64 + step as i64 / 2;
                out.set(x, z, self.height(sx, sz));
            }
        }
        Ok(out)
    }
    /// Exhaustive bound on vertical displacement of these two subgraph tops.
    /// At a shared vertical grid edge the interval endpoints also move by <=E;
    /// this bounds their surface Hausdorff distance by E in world metres. It is
    /// not a bound on ray depth or projected silhouette error at grazing angles.
    fn vertical_bound_m(&self, other: &Self) -> f64 {
        assert_eq!((self.min, self.width), (other.min, other.width));
        self.heights
            .iter()
            .zip(&other.heights)
            .map(|(a, b)| (*a as f64 - *b as f64).abs() * voxel_size())
            .fold(0., f64::max)
    }
    fn faces(&self, cancel: &mut impl FnMut() -> bool) -> Result<Vec<Face>, String> {
        let mut faces = Vec::new();
        let mut push = |face| {
            if faces.len() == MAX_FACES {
                Err("handoff face budget exceeded")
            } else {
                faces.push(face);
                Ok(())
            }
        };
        for z in 0..self.width as i64 {
            if cancel() {
                return Err("cancelled handoff experiment".into());
            }
            for x in 0..self.width as i64 {
                let h = self.height(x, z);
                push(Face {
                    cell: [x, h - 1, z],
                    axis: 1,
                    positive: true,
                })?;
                for (axis, dx, dz, positive) in [
                    (0, 1, 0, true),
                    (0, -1, 0, false),
                    (2, 0, 1, true),
                    (2, 0, -1, false),
                ] {
                    let neighbor = self.height(x + dx, z + dz);
                    // Preflight relief before iterating; no hidden geometry
                    // clipping or synthesis of a sealed finite crop boundary.
                    if h as i128 - neighbor as i128 > MAX_FACES as i128 {
                        return Err("handoff relief exceeds face budget".into());
                    }
                    for y in neighbor..h {
                        push(Face {
                            cell: [x, y, z],
                            axis,
                            positive,
                        })?;
                    }
                }
            }
        }
        Ok(faces)
    }
    fn triangles(&self, cancel: &mut impl FnMut() -> bool) -> Result<Vec<Triangle>, String> {
        let mut out = Vec::new();
        for face in self.faces(cancel)? {
            let u = (face.axis + 1) % 3;
            let v = (face.axis + 2) % 3;
            let mut p = face.cell.map(|v| v as f64);
            p[face.axis] += f64::from(face.positive);
            let mut a = p;
            a[u] += 1.;
            let mut b = a;
            b[v] += 1.;
            let mut c = p;
            c[v] += 1.;
            let mut n = [0.; 3];
            n[face.axis] = if face.positive { 1. } else { -1. };
            let vertices = if face.positive {
                [p, a, b, c]
            } else {
                [p, c, b, a]
            };
            out.push(Triangle {
                p: [vertices[0], vertices[1], vertices[2]],
                normal: n,
            });
            out.push(Triangle {
                p: [vertices[0], vertices[2], vertices[3]],
                normal: n,
            });
        }
        Ok(out)
    }
    fn smooth_triangles(&self, step: usize) -> Vec<Triangle> {
        let mut out = Vec::new();
        for z in (0..self.width).step_by(step) {
            for x in (0..self.width).step_by(step) {
                let p =
                    [[x, z], [x + step, z], [x + step, z + step], [x, z + step]].map(|[x, z]| {
                        // Source corner height is sampled in the canonical halo.
                        [x as f64, self.height(x as i64, z as i64) as f64, z as f64]
                    });
                for indices in [[0, 2, 1], [0, 3, 2]] {
                    let points = indices.map(|i| p[i]);
                    let a = std::array::from_fn::<_, 3, _>(|i| points[1][i] - points[0][i]);
                    let b = std::array::from_fn::<_, 3, _>(|i| points[2][i] - points[0][i]);
                    let n = [
                        a[1] * b[2] - a[2] * b[1],
                        a[2] * b[0] - a[0] * b[2],
                        a[0] * b[1] - a[1] * b[0],
                    ];
                    let length = n.iter().map(|v| v * v).sum::<f64>().sqrt();
                    out.push(Triangle {
                        p: points,
                        normal: n.map(|v| v / length),
                    });
                }
            }
        }
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_edges_and_vertical_bound_survive_negative_coordinates_and_two_to_one_steps() {
        let source = |x: i64, z: i64| Ok(16 + (x.rem_euclid(13) * 3 + z.rem_euclid(7) * 2) % 21);
        let a = Field::sample([-32, -32], 32, source, &mut || false).unwrap();
        for step in [2, 4, 8] {
            let b = a.stairs(step, &mut || false).unwrap();
            let error = a.vertical_bound_m(&b);
            for z in 0..32 {
                for x in 0..32 {
                    assert!((a.height(x, z) - b.height(x, z)).abs() as f64 * voxel_size() <= error);
                    if x == 0 || z == 0 || x == 31 || z == 31 {
                        assert_eq!(a.height(x, z), b.height(x, z));
                    }
                }
            }
            let boundary = |field: &Field| {
                field
                    .faces(&mut || false)
                    .unwrap()
                    .into_iter()
                    .filter(|f| {
                        f.axis == 0
                            && (f.cell[0] == 0 && !f.positive || f.cell[0] == 31 && f.positive)
                            || f.axis == 2
                                && (f.cell[2] == 0 && !f.positive || f.cell[2] == 31 && f.positive)
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(boundary(&a), boundary(&b));
            assert!(b.faces(&mut || true).is_err());
            assert!(a.stairs(step, &mut || true).is_err());
        }
        let right = Field::sample([0, -32], 32, source, &mut || false)
            .unwrap()
            .stairs(4, &mut || false)
            .unwrap();
        let left = a.stairs(8, &mut || false).unwrap();
        for z in 0..32 {
            assert_eq!(left.height(32, z), right.height(0, z));
            assert_eq!(left.height(31, z), right.height(-1, z));
        }
        assert!(a.stairs(16, &mut || false).is_err());
    }
    #[test]
    fn prototype_rejects_topology_edits_water_and_excessive_relief() {
        let params = frontier_world::FbmParams::default();
        let mut world = World::fbm(7, params).unwrap();
        world.set(
            frontier_world::Cell { x: 0, y: 0, z: 0 },
            frontier_world::Material::Wood,
        );
        assert!(Field::from_world(&world, [0, 0], 8, &mut || false).is_err());
        assert!(Field::from_world(&World::new(7), [0, 0], 8, &mut || false).is_err());
        assert!(Field::from_world(
            &World::fbm_topology(7, params).unwrap(),
            [0, 0],
            8,
            &mut || false
        )
        .is_err());
        let ocean = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::SeededWorld,
                ..Default::default()
            },
        )
        .unwrap();
        // Find a real water column; rejection is conditional on that source,
        // rather than treating terrain below sea level as opaque dry geometry.
        let [x, z] = (-4..=4)
            .flat_map(|z| (-4..=4).map(move |x| [x * 320000, z * 320000]))
            .find(|[x, z]| {
                ocean.column(*x, *z).height_m() != ocean.column(*x, *z).ground_height_m()
            })
            .expect("seed7 must exercise an actual water column");
        assert!(Field::from_world(&ocean, [x, z], 8, &mut || false).is_err());
        let cliff = Field::sample(
            [0, 0],
            8,
            |x, _| Ok(if x == 0 { 1_000_000 } else { 0 }),
            &mut || false,
        )
        .unwrap();
        assert!(cliff.faces(&mut || false).is_err());
    }
    fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
        a.into_iter().zip(b).map(|(a, b)| a * b).sum()
    }
    fn export(
        path: &std::path::Path,
        name: &str,
        triangles: &[Triangle],
        center: [f64; 3],
        scale: f64,
    ) -> std::io::Result<()> {
        use std::io::Write;
        let mut obj =
            std::io::BufWriter::new(std::fs::File::create(path.join(format!("{name}.obj")))?);
        for (i, t) in triangles.iter().enumerate() {
            for p in t.p {
                writeln!(
                    obj,
                    "v {} {} {}",
                    p[0] * voxel_size(),
                    p[1] * voxel_size(),
                    p[2] * voxel_size()
                )?;
            }
            writeln!(obj, "f {} {} {}", i * 3 + 1, i * 3 + 2, i * 3 + 3)?;
        }
        let (width, height) = (512usize, 384usize);
        let mut pixels = vec![[27u8, 31, 38]; width * height];
        let mut depth = vec![f64::NEG_INFINITY; width * height];
        let r = [0.707106781, 0., -0.707106781];
        let u = [0.40824829, 0.816496581, 0.40824829];
        let view = [-0.577350269, 0.577350269, -0.577350269];
        for t in triangles {
            if dot(t.normal, view) <= 0. {
                continue;
            }
            let p = t.p.map(|p| {
                let v = std::array::from_fn(|i| p[i] - center[i]);
                [
                    width as f64 * 0.5 + dot(v, r) * scale,
                    height as f64 * 0.5 - dot(v, u) * scale,
                    dot(v, view),
                ]
            });
            let edge = |a: [f64; 3], b: [f64; 3], x: f64, y: f64| {
                (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0])
            };
            let area = edge(p[0], p[1], p[2][0], p[2][1]);
            if area.abs() < 1e-9 {
                continue;
            }
            let xmin = p.iter().map(|p| p[0].floor() as i64).min().unwrap().max(0) as usize;
            let xmax = p
                .iter()
                .map(|p| p[0].ceil() as i64)
                .max()
                .unwrap()
                .min(width as i64 - 1)
                .max(-1);
            let ymin = p.iter().map(|p| p[1].floor() as i64).min().unwrap().max(0) as usize;
            let ymax = p
                .iter()
                .map(|p| p[1].ceil() as i64)
                .max()
                .unwrap()
                .min(height as i64 - 1)
                .max(-1);
            if xmax < xmin as i64 || ymax < ymin as i64 {
                continue;
            }
            for y in ymin..=ymax as usize {
                if y >= height {
                    break;
                }
                for x in xmin..=xmax as usize {
                    if x >= width {
                        break;
                    }
                    let weights = [
                        edge(p[1], p[2], x as f64 + 0.5, y as f64 + 0.5) / area,
                        edge(p[2], p[0], x as f64 + 0.5, y as f64 + 0.5) / area,
                        edge(p[0], p[1], x as f64 + 0.5, y as f64 + 0.5) / area,
                    ];
                    if weights.iter().any(|v| *v < 0.) {
                        continue;
                    }
                    let d = weights
                        .into_iter()
                        .zip(p)
                        .map(|(w, p)| w * p[2])
                        .sum::<f64>();
                    let at = y * width + x;
                    if d > depth[at] {
                        depth[at] = d;
                        let light = 0.45 + 0.55 * dot(t.normal, [-0.4, 0.8, -0.447213595]).max(0.);
                        let color = if t.normal[1] > 0.9 {
                            [105., 151., 69.]
                        } else {
                            [126., 102., 74.]
                        };
                        pixels[at] = color.map(|v| (v * light) as u8);
                    }
                }
            }
        }
        let mut ppm =
            std::io::BufWriter::new(std::fs::File::create(path.join(format!("{name}.ppm")))?);
        writeln!(ppm, "P6\n{width} {height}\n255")?;
        for p in pixels {
            ppm.write_all(&p)?;
        }
        Ok(())
    }
    #[test]
    #[ignore = "offline #83 OBJ/PPM reference export; deliberately not live LOD integration"]
    fn export_voxel_handoff_reference_comparison() {
        let path = std::path::PathBuf::from(
            std::env::var("FRONTIER_HANDOFF_OUTPUT")
                .unwrap_or_else(|_| "output/nov16/voxel-handoff".into()),
        );
        std::fs::create_dir_all(&path).unwrap();
        let mut csv =
            String::from("fixture,representation,step,triangles,vertical_bound_m,build_ms\n");
        let synthetic = Field::sample(
            [-16, -16],
            32,
            |x, z| {
                Ok(24
                    + (x.rem_euclid(11) - 5).abs() * 2
                    + (z.rem_euclid(7) - 3).abs() * 2
                    + if x.rem_euclid(13) == 0 { 8 } else { 0 })
            },
            &mut || false,
        )
        .unwrap();
        let w = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::Alpine,
                ..Default::default()
            },
        )
        .unwrap();
        let canonical = Field::from_world(&w, [-58 * 16, -48 * 16], 32, &mut || false).unwrap();
        for (name, field) in [
            ("synthetic_spikes", synthetic),
            ("canonical_alpine", canonical),
        ] {
            let low = *field.heights.iter().min().unwrap();
            let high = *field.heights.iter().max().unwrap();
            let center = [16., (low + high) as f64 * 0.5, 16.];
            let scale = (420f64 / (32. * 1.414213562)).min(300. / ((high - low) as f64 + 24.));
            let start = std::time::Instant::now();
            let native = field.triangles(&mut || false).unwrap();
            csv += &format!(
                "{name},native,1,{},0,{}\n",
                native.len(),
                start.elapsed().as_secs_f64() * 1000.
            );
            export(&path, &format!("{name}_native"), &native, center, scale).unwrap();
            for step in [2, 4, 8] {
                let start = std::time::Instant::now();
                let candidate = field.stairs(step, &mut || false).unwrap();
                let triangles = candidate.triangles(&mut || false).unwrap();
                csv += &format!(
                    "{name},stairs,{step},{},{},{}\n",
                    triangles.len(),
                    field.vertical_bound_m(&candidate),
                    start.elapsed().as_secs_f64() * 1000.
                );
                export(
                    &path,
                    &format!("{name}_stairs{step}"),
                    &triangles,
                    center,
                    scale,
                )
                .unwrap();
            }
            let smooth = field.smooth_triangles(8);
            csv += &format!("{name},smooth,8,{},unmeasured,0\n", smooth.len());
            export(&path, &format!("{name}_smooth8"), &smooth, center, scale).unwrap();
        }
        std::fs::write(path.join("comparison.csv"), &csv).unwrap();
        eprintln!("{csv}exported {}", path.display());
    }
}
