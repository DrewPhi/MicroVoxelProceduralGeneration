//! Fitted model interchange and double-precision reference intersections.
use serde::Deserialize;
mod tetrahedra;
#[derive(Deserialize)]
pub struct Scene {
    pub version: u32,
    pub representation: String,
    pub solid: String,
    pub feature_order: Vec<String>,
    pub patch_order: String,
    pub patches_per_axis: [usize; 3],
    pub quadratics_per_patch: usize,
    pub patch_edge_m: f64,
    pub origin_cells: [i64; 3],
    pub voxel_m: f64,
    pub dimensions: [usize; 3],
    pub normalization: String,
    #[serde(default)]
    pub coefficients: Vec<Vec<Vec<Vec<[f64; 10]>>>>,
    #[serde(default)]
    pub coefficients_file: Option<String>,
    #[serde(default)]
    pub coefficients_dtype: Option<String>,
    #[serde(skip)]
    pub binary: Vec<[f32; 12]>,
}
impl Scene {
    pub fn load(path: &str) -> Result<Self, String> {
        let mut s: Self = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let g = s.patches_per_axis;
        if s.version != 1
            || !((s.representation == "min-of-implicit-quadrics" && s.solid == "min(Q)>0")
                || (s.is_spline() && s.solid == "Q>0" && s.quadratics_per_patch == 24))
            || s.patch_order != "y,z,x"
            || s.feature_order != ["1", "x", "y", "z", "xx", "yy", "zz", "xy", "xz", "yz"]
            || s.normalization != "local = 2*(world_m-patch_min_m)/patch_edge_m - 1"
            || !s.patch_edge_m.is_finite()
            || s.patch_edge_m <= 0.
            || !s.voxel_m.is_finite()
            || s.voxel_m <= 0.
            || g.iter().any(|&n| n == 0 || n > 256)
            || s.quadratics_per_patch == 0
            || s.quadratics_per_patch > 128
            || g.iter().product::<usize>() * s.quadratics_per_patch > 8_000_000
        {
            return Err("Unsupported or invalid fitted model".into());
        }
        if let Some(file) = &s.coefficients_file {
            use std::io::Read;
            if s.coefficients_dtype.as_deref() != Some("float32-padded12-le")
                || !s.coefficients.is_empty()
            {
                return Err("Unsupported binary coefficient encoding".into());
            }
            let count = g.iter().product::<usize>() * s.quadratics_per_patch;
            let path = std::path::Path::new(path)
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(file);
            let mut input = std::fs::File::open(path).map_err(|e| e.to_string())?;
            if input.metadata().map_err(|e| e.to_string())?.len() != (count * 48) as u64 {
                return Err("Invalid binary coefficient length".into());
            }
            s.binary = vec![[0.; 12]; count];
            input
                .read_exact(bytemuck::cast_slice_mut(&mut s.binary))
                .map_err(|e| e.to_string())?;
            for q in &mut s.binary {
                for v in q.iter_mut() {
                    *v = f32::from_le_bytes(v.to_ne_bytes());
                }
                if q.iter().any(|v| !v.is_finite() || v.abs() > 1e12) || q[10] < 0. {
                    return Err("Invalid binary coefficients".into());
                }
                let c = std::array::from_fn(|i| q[i] as f64);
                if (q[10] as f64) < voxel_center_margin(&c, s.voxel_m / s.patch_edge_m) {
                    return Err("Unsafe binary voxel envelope".into());
                }
            }
        } else {
            if s.coefficients.len() != g[1] {
                return Err("Invalid coefficient Y shape".into());
            }
            for layer in &s.coefficients {
                if layer.len() != g[2] {
                    return Err("Invalid coefficient Z shape".into());
                }
                for row in layer {
                    if row.len() != g[0] {
                        return Err("Invalid coefficient X shape".into());
                    }
                    for patch in row {
                        if patch.len() != s.quadratics_per_patch
                            || patch
                                .iter()
                                .flatten()
                                .any(|x| !x.is_finite() || x.abs() > 1e12)
                        {
                            return Err("Invalid coefficients".into());
                        }
                    }
                }
            }
        }
        for (a, &count) in g.iter().enumerate() {
            if (s.dimensions[a] as f64 * s.voxel_m - count as f64 * s.patch_edge_m).abs() > 1e-5 {
                return Err("Inconsistent scene bounds".into());
            }
        }
        Ok(s)
    }
    /// Highest solid surface in one vertical patch column. Closed-form roots only;
    /// work scales with vertical patches, never the world's horizontal area.
    pub fn ground_height(&self, x: f64, z: f64, voxels: bool) -> Option<f64> {
        let e = self.patch_edge_m;
        let [ex, _, ez] = self.extent();
        if x < 0. || z < 0. || x >= ex || z >= ez {
            return None;
        }
        let (x, z) = if voxels {
            (
                (x / self.voxel_m).floor() * self.voxel_m + self.voxel_m * 0.5,
                (z / self.voxel_m).floor() * self.voxel_m + self.voxel_m * 0.5,
            )
        } else {
            (x, z)
        };
        let px = (x / e).floor() as usize;
        let pz = (z / e).floor() as usize;
        let lx = 2. * (x / e - px as f64) - 1.;
        let lz = 2. * (z / e - pz as f64) - 1.;
        for py in (0..self.patches_per_axis[1]).rev() {
            let quads: Vec<_> = (0..self.quadratics_per_patch)
                .map(|i| self.quad(px, py, pz, i))
                .collect();
            let mut highest: Option<f64> = None;
            for (piece, q) in quads.iter().enumerate() {
                let b = q[2] + q[7] * lx + q[9] * lz;
                let c = value(q, [lx, 0., lz]);
                for y in roots(q[5], b, c) {
                    if !(-1.0000001..=1.0000001).contains(&y) {
                        continue;
                    }
                    let p = [lx, y, lz];
                    let valid = if self.is_spline() {
                        tetrahedra::BARY[piece]
                            .iter()
                            .all(|r| r[0] + r[1] * lx + r[2] * y + r[3] * lz >= -1e-7)
                    } else {
                        quads.iter().all(|q| value(q, p) >= -1e-6)
                    };
                    if valid && gradient(q, p)[1] < -1e-10 {
                        let h = (py as f64 + (y + 1.) * 0.5) * e;
                        highest = Some(highest.map_or(h, |old| old.max(h)));
                    }
                }
            }
            if let Some(h) = highest {
                return Some(if voxels {
                    ((h / self.voxel_m - 0.5).ceil() * self.voxel_m).max(0.)
                } else {
                    h
                });
            }
        }
        None
    }
    pub fn is_spline(&self) -> bool {
        self.representation == "quadratic-super-spline-24"
    }
    pub fn extent(&self) -> [f64; 3] {
        self.patches_per_axis.map(|n| n as f64 * self.patch_edge_m)
    }
    pub fn origin(&self) -> [f64; 3] {
        self.origin_cells.map(|n| n as f64 * self.voxel_m)
    }
    /// Double precision discrete occupancy reference. Recomputes boundary times
    /// from integer cell coordinates rather than accumulating GPU DDA deltas.
    pub fn reference_voxels(&self, o: [f64; 3], d: [f64; 3]) -> Option<f64> {
        let extent = self.extent();
        let size = self.voxel_m;
        let mut enter = 0f64;
        let mut exit = f64::INFINITY;
        for a in 0..3 {
            if d[a].abs() < 1e-12 {
                if o[a] < 0. || o[a] >= extent[a] {
                    return None;
                }
            } else {
                let a0 = -o[a] / d[a];
                let a1 = (extent[a] - o[a]) / d[a];
                enter = enter.max(a0.min(a1));
                exit = exit.min(a0.max(a1));
            }
        }
        if enter >= exit {
            return None;
        }
        let mut cell: [i64; 3] = std::array::from_fn(|a| {
            let p = (o[a] + enter * d[a]) / size;
            let mut c = p.floor() as i64;
            if d[a] < 0. && (p - p.round()).abs() < 1e-9 {
                c = p.round() as i64 - 1;
            }
            c.clamp(0, self.dimensions[a] as i64 - 1)
        });
        for _ in 0..self.dimensions.iter().sum::<usize>() + 3 {
            if enter >= exit || (0..3).any(|a| cell[a] < 0 || cell[a] >= self.dimensions[a] as i64)
            {
                return None;
            }
            let center = cell.map(|v| (v as f64 + 0.5) * size);
            let pc = center.map(|v| (v / self.patch_edge_m).floor() as usize);
            let local = std::array::from_fn(|a| {
                2. * (center[a] - pc[a] as f64 * self.patch_edge_m) / self.patch_edge_m - 1.
            });

            let solid = if self.is_spline() {
                value(&self.quad(pc[0], pc[1], pc[2], spline_piece(local)), local) > 0.
            } else {
                (0..self.quadratics_per_patch)
                    .all(|i| value(&self.quad(pc[0], pc[1], pc[2], i), local) > 0.)
            };
            if solid {
                return Some(enter);
            }
            let next: [f64; 3] = std::array::from_fn(|a| {
                if d[a].abs() < 1e-12 {
                    f64::INFINITY
                } else {
                    ((cell[a] as f64 + if d[a] > 0. { 1. } else { 0. }) * size - o[a]) / d[a]
                }
            });
            enter = next.into_iter().fold(f64::INFINITY, f64::min);
            for a in 0..3 {
                if next[a] <= enter + 1e-10 {
                    cell[a] += if d[a] > 0. { 1 } else { -1 };
                }
            }
        }
        None
    }
    pub fn quad(&self, x: usize, y: usize, z: usize, i: usize) -> [f64; 10] {
        if self.binary.is_empty() {
            self.coefficients[y][z][x][i]
        } else {
            let q = &self.binary[((y * self.patches_per_axis[2] + z) * self.patches_per_axis[0]
                + x)
                * self.quadratics_per_patch
                + i];
            std::array::from_fn(|a| q[a] as f64)
        }
    }
    pub fn packed(&self) -> std::borrow::Cow<'_, [[f32; 12]]> {
        if !self.binary.is_empty() {
            return std::borrow::Cow::Borrowed(&self.binary);
        }
        std::borrow::Cow::Owned(self.pack_json())
    }
    fn pack_json(&self) -> Vec<[f32; 12]> {
        let margins: Vec<f64> = self
            .coefficients
            .iter()
            .flatten()
            .flatten()
            .map(|p| {
                p.iter()
                    .map(|c| {
                        let rounded = c.map(|v| v as f32 as f64);
                        voxel_center_margin(&rounded, self.voxel_m / self.patch_edge_m)
                            + 0.000012 * (1. + rounded.iter().map(|v| v.abs()).sum::<f64>())
                    })
                    .fold(0., f64::max)
            })
            .collect();
        self.coefficients
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .enumerate()
            .map(|(index, c)| {
                let mut q = [0.; 12];
                for (a, b) in q.iter_mut().zip(c) {
                    *a = *b as f32;
                }
                let rounded = std::array::from_fn(|i| q[i] as f64);
                q[10] = (voxel_center_margin(&rounded, self.voxel_m / self.patch_edge_m)
                    + 0.000012 * (1. + rounded.iter().map(|v| v.abs()).sum::<f64>()))
                    as f32
                    * 1.000001;
                if self.is_spline() {
                    q[10] = margins[index / self.quadratics_per_patch] as f32 * 1.000001;
                }
                q
            })
            .collect()
    }
    /// Independent reference: brute force candidate patches; no shader grid traversal.
    pub fn reference(&self, o: [f64; 3], d: [f64; 3]) -> Option<f64> {
        let mut best = f64::INFINITY;
        let e = self.patch_edge_m;
        for y in 0..self.patches_per_axis[1] {
            for z in 0..self.patches_per_axis[2] {
                for x in 0..self.patches_per_axis[0] {
                    let low = [x as f64 * e, y as f64 * e, z as f64 * e];
                    let mut entry = 0f64;
                    let mut exit = best;
                    for a in 0..3 {
                        if d[a].abs() < 1e-14 {
                            if o[a] < low[a] || o[a] > low[a] + e {
                                exit = -1.;
                            }
                            continue;
                        }
                        let t0 = (low[a] - o[a]) / d[a];
                        let t1 = (low[a] + e - o[a]) / d[a];
                        entry = entry.max(t0.min(t1));
                        exit = exit.min(t0.max(t1));
                    }
                    if entry > exit {
                        continue;
                    }
                    let quads: Vec<_> = (0..self.quadratics_per_patch)
                        .map(|i| self.quad(x, y, z, i))
                        .collect();
                    let u = std::array::from_fn(|a| 2. * (o[a] + entry * d[a] - low[a]) / e - 1.);
                    let v = d.map(|x| x * 2. / e);
                    for (piece, q) in quads.iter().enumerate() {
                        let c = value(q, u);
                        let a = q[4] * v[0] * v[0]
                            + q[5] * v[1] * v[1]
                            + q[6] * v[2] * v[2]
                            + q[7] * v[0] * v[1]
                            + q[8] * v[0] * v[2]
                            + q[9] * v[1] * v[2];
                        let grad = gradient(q, u);
                        let b = grad.iter().zip(v).map(|(a, b)| a * b).sum();
                        for dt in roots(a, b, c) {
                            let t = entry + dt;
                            if dt < -1e-8 || t > exit + 1e-8 || t < 0. || t >= best {
                                continue;
                            }
                            let p = std::array::from_fn(|a| u[a] + dt * v[a]);
                            let valid = if self.is_spline() {
                                tetrahedra::BARY[piece].iter().all(|row| {
                                    row[0] + row[1] * p[0] + row[2] * p[1] + row[3] * p[2] >= -1e-7
                                })
                            } else {
                                quads.iter().all(|q| value(q, p) >= -1e-7)
                            };
                            if valid {
                                best = t;
                            }
                        }
                    }
                }
            }
        }
        best.is_finite().then_some(best)
    }
}
pub fn value(q: &[f64; 10], p: [f64; 3]) -> f64 {
    let [x, y, z] = p;
    q[0] + q[1] * x
        + q[2] * y
        + q[3] * z
        + q[4] * x * x
        + q[5] * y * y
        + q[6] * z * z
        + q[7] * x * y
        + q[8] * x * z
        + q[9] * y * z
}
pub fn gradient(q: &[f64; 10], p: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = p;
    [
        q[1] + 2. * q[4] * x + q[7] * y + q[8] * z,
        q[2] + 2. * q[5] * y + q[7] * x + q[9] * z,
        q[3] + 2. * q[6] * z + q[8] * x + q[9] * y,
    ]
}
pub fn roots(a: f64, b: f64, c: f64) -> Vec<f64> {
    if a.abs() < 1e-14 {
        return if b.abs() < 1e-14 {
            vec![]
        } else {
            vec![-c / b]
        };
    }
    let disc = b * b - 4. * a * c;
    if disc < 0. {
        return vec![];
    }
    let q = -0.5 * (b + disc.sqrt().copysign(b));
    if q == 0. {
        return vec![-b / (2. * a)];
    }
    vec![q / a, c / q]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_roots_cover_linear_tangent_and_cancellation() {
        assert_eq!(roots(0., 2., -4.), vec![2.]);
        assert!(roots(1., 0., 1.).is_empty());
        assert_eq!(roots(1., 0., 0.), vec![0.]);
        let r = roots(1., 1e8, 1.);
        assert!((r[1] + 1e-8).abs() < 1e-15);
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;
    fn scene(quads: Vec<[f64; 10]>) -> Scene {
        Scene {
            version: 1,
            representation: "min-of-implicit-quadrics".into(),
            solid: "min(Q)>0".into(),
            feature_order: vec![],
            patch_order: "y,z,x".into(),
            patches_per_axis: [1; 3],
            quadratics_per_patch: quads.len(),
            patch_edge_m: 2.,
            origin_cells: [0; 3],
            voxel_m: 0.0625,
            dimensions: [32; 3],
            normalization: String::new(),
            coefficients_file: None,
            coefficients_dtype: None,
            binary: vec![],
            coefficients: vec![vec![vec![quads]]],
        }
    }
    #[test]
    fn binary_and_json_coefficients_have_identical_reference_hits() {
        let mut s = scene(vec![[0.25, 0., 0., 0., -1., -1., -1., 0., 0., 0.]]);
        let packed = s.packed().into_owned();
        let hit = s.reference([-1., 1., 1.], [1., 0., 0.]);
        let voxel = s.reference_voxels([-1., 1., 1.], [1., 0., 0.]);
        s.binary = packed;
        s.coefficients.clear();
        assert_eq!(hit, s.reference([-1., 1., 1.], [1., 0., 0.]));
        assert_eq!(voxel, s.reference_voxels([-1., 1., 1.], [1., 0., 0.]));
        assert!(matches!(s.packed(), std::borrow::Cow::Borrowed(_)));
    }
    #[test]
    fn sphere_entry_exit_and_miss() {
        let s = scene(vec![[0.25, 0., 0., 0., -1., -1., -1., 0., 0., 0.]]);
        assert!((s.reference([-1., 1., 1.], [1., 0., 0.]).unwrap() - 1.5).abs() < 1e-12);
        assert!((s.reference([1., 1., 1.], [1., 0., 0.]).unwrap() - 0.5).abs() < 1e-12);
        assert_eq!(s.reference([-1., 1.9, 1.], [1., 0., 0.]), None);
    }
    #[test]
    fn inactive_quad_roots_are_rejected() {
        // Solid only where both x > 0 and x > .5 in patch-local space.
        let s = scene(vec![
            [0., 1., 0., 0., 0., 0., 0., 0., 0., 0.],
            [-0.5, 1., 0., 0., 0., 0., 0., 0., 0., 0.],
        ]);
        assert!((s.reference([-1., 1., 1.], [1., 0., 0.]).unwrap() - 2.5).abs() < 1e-12);
    }
}

#[cfg(test)]
mod voxel_tests {
    use super::*;
    #[test]
    fn discrete_plane_has_axis_aligned_boundaries() {
        let q = [0., 1., 0., 0., 0., 0., 0., 0., 0., 0.];
        let s = Scene {
            version: 1,
            representation: String::new(),
            solid: String::new(),
            feature_order: vec![],
            patch_order: String::new(),
            patches_per_axis: [1; 3],
            quadratics_per_patch: 1,
            patch_edge_m: 2.,
            origin_cells: [0; 3],
            voxel_m: 0.0625,
            dimensions: [32; 3],
            normalization: String::new(),
            coefficients_file: None,
            coefficients_dtype: None,
            binary: vec![],
            coefficients: vec![vec![vec![vec![q]]]],
        };
        assert_eq!(s.reference_voxels([-1., 1., 1.], [1., 0., 0.]), Some(2.));
        assert_eq!(s.reference_voxels([3., 1., 1.], [-1., 0., 0.]), Some(1.));
        assert_eq!(s.reference_voxels([-1., 1., 1.], [0., 1., 0.]), None);
        assert_eq!(s.reference_voxels([1.5, 1., 1.], [1., 0., 0.]), Some(0.));
    }
}

/// Max gradient components on the patch, times the local half-cell width.
/// This is an analytic bound; it does not assume density is a distance field.
pub fn voxel_center_margin(q: &[f64; 10], r: f64) -> f64 {
    let gx = q[1].abs() + 2. * q[4].abs() + q[7].abs() + q[8].abs();
    let gy = q[2].abs() + 2. * q[5].abs() + q[7].abs() + q[9].abs();
    let gz = q[3].abs() + 2. * q[6].abs() + q[8].abs() + q[9].abs();
    r * (gx + gy + gz)
}
#[cfg(test)]
mod bound_tests {
    use super::*;
    #[test]
    fn center_envelope_contains_random_quadratics_and_voxels() {
        let mut seed = 42u64;
        let mut rand = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..20000 {
            let q = std::array::from_fn(|_| (rand() * 2. - 1.) * 100.);
            let r = 0.0001 + rand() * 0.25;
            let center = std::array::from_fn(|_| (rand() * 2. - 1.) * (1. - r));
            let p = std::array::from_fn(|a| center[a] + (rand() * 2. - 1.) * r);
            assert!((value(&q, center) - value(&q, p)).abs() <= voxel_center_margin(&q, r) + 1e-10);
        }
    }
}

/// O(1) type-6 tetrahedron locator. No triangles are generated for rendering.
pub fn spline_piece(p: [f64; 3]) -> usize {
    let mut axis = 0;
    for a in 1..3 {
        if p[a].abs() > p[axis].abs() {
            axis = a;
        }
    }
    let a = if axis == 0 { 1 } else { 0 };
    let b = if axis == 2 { 1 } else { 2 };
    let edge = if p[a].abs() >= p[b].abs() {
        if p[a] >= 0. {
            1
        } else {
            3
        }
    } else if p[b] >= 0. {
        2
    } else {
        0
    };
    axis * 8 + if p[axis] >= 0. { 4 } else { 0 } + edge
}

#[cfg(test)]
mod spline_tests {
    use super::*;
    #[test]
    fn locator_and_plane_intersections_match_tetrahedral_domains() {
        let mut seed = 19u64;
        for _ in 0..10000 {
            let p = std::array::from_fn(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((seed >> 11) as f64 / (1u64 << 53) as f64) * 2. - 1.
            });
            let id = spline_piece(p);
            assert!(tetrahedra::BARY[id].iter().all(|row| row[0]
                + row[1] * p[0]
                + row[2] * p[1]
                + row[3] * p[2]
                >= -1e-12));
        }
        let q = [0.1, 0., -1., 0., 0., 0., 0., 0., 0., 0.];
        let s = Scene {
            version: 1,
            representation: "quadratic-super-spline-24".into(),
            solid: "Q>0".into(),
            feature_order: vec![],
            patch_order: "y,z,x".into(),
            patches_per_axis: [1; 3],
            quadratics_per_patch: 24,
            patch_edge_m: 2.,
            origin_cells: [0; 3],
            voxel_m: 0.0625,
            dimensions: [32; 3],
            normalization: String::new(),
            coefficients_file: None,
            coefficients_dtype: None,
            binary: vec![],
            coefficients: vec![vec![vec![vec![q; 24]]]],
        };
        let distance = s.reference([0.9, 3., 1.01], [0., -1., 0.]).unwrap();
        assert!((distance - 1.9).abs() < 1e-12);
        let voxel = s.reference_voxels([0.9, 3., 1.01], [0., -1., 0.]).unwrap();
        assert!((voxel - 1.875).abs() < 1e-12);
        let packed = s.packed();
        assert!(packed.iter().all(|q| q[10] == packed[0][10]));
    }
}
