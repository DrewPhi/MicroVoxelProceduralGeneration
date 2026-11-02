//! Isometric (orthographic) CPU voxel renderer for terrain previews.
//!
//! Voxelizes an implicit 3D density (the source terrain or its fitted quadrics) (a voxel is solid when the density at its
//! centre is > 0), draws every camera-facing exposed face, and lights it with a
//! sun, ray-marched voxel shadows and sky ambient. Wide views use voxels that
//! are a whole multiple of the engine voxel size and can shade with the smooth
//! density normal. Floating voxels (not connected to the ground inside the
//! view) can be highlighted. This is a debugging aid, not the engine renderer.

use crate::color::terrain_color;
use crate::par_map;
use crate::source::VoxelSource;

type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l == 0.0 { a } else { a.map(|v| v / l) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shading {
    /// Smooth when the preview voxels are coarser than the engine voxels.
    Auto,
    /// Flat per-face normals: exactly what the engine's voxels look like.
    Face,
    /// Density-gradient normals: shows the underlying smooth surface.
    Smooth,
}

impl std::str::FromStr for Shading {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::Auto),
            "face" => Ok(Self::Face),
            "smooth" => Ok(Self::Smooth),
            _ => Err(format!("shading must be auto, face or smooth (got {s:?})")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct IsoSettings {
    /// World-space centre of the rendered square, meters.
    pub center: [f64; 2],
    /// Side length of the rendered square, meters.
    pub extent: f64,
    /// Output image width in pixels (height follows from the view).
    pub width: u32,
    /// Upper limit on voxel columns per side; coarser voxels are used beyond it.
    pub max_columns: u32,
    /// Engine voxel size, meters. Preview voxels are a whole multiple of it.
    pub voxel_size: f64,
    /// Camera direction: azimuth (0 = from north, clockwise) and elevation, degrees.
    pub yaw: f64,
    pub pitch: f64,
    /// Vertical scale factor.
    pub exaggeration: f64,
    pub shading: Shading,
    pub shadows: bool,
    /// Tint voxels that are not connected to the ground (within the view).
    pub mark_floaters: bool,
    /// Unit vector toward the sun.
    pub sun: V3,
}

pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub triangles: usize,
    pub columns: usize,
    pub voxel: f64,
    pub faces: usize,
    /// Solid voxels not connected to the ground within the view, if counted.
    pub floating: Option<usize>,
}

const SUPERSAMPLE: usize = 2;

struct Raster {
    w: usize,
    h: usize,
    right: V3,
    up: V3,
    toward: V3,
    scale: f64,
    offset: [f64; 2],
    depth: Vec<f32>,
    color: Vec<[f32; 3]>,
    triangles: usize,
}

impl Raster {
    /// Orthographic camera framing the box `lo..hi`.
    fn new(s: &IsoSettings, toward: V3, lo: V3, hi: V3) -> Self {
        let fwd = toward.map(|v| -v);
        // x = east, z = north: looking north, right is east.
        let right = normalize([fwd[2], 0.0, -fwd[0]]);
        let d = fwd[1];
        let up = normalize([-d * fwd[0], 1.0 - d * fwd[1], -d * fwd[2]]);

        let (mut min, mut max) = ([f64::MAX; 2], [f64::MIN; 2]);
        for k in 0..8 {
            let p = [
                if k & 1 == 0 { lo[0] } else { hi[0] },
                if k & 2 == 0 { lo[1] } else { hi[1] },
                if k & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let sp = [dot(p, right), -dot(p, up)];
            for a in 0..2 {
                min[a] = min[a].min(sp[a]);
                max[a] = max[a].max(sp[a]);
            }
        }
        let w = s.width as usize * SUPERSAMPLE;
        let margin = 0.04 * w as f64;
        let scale = (w as f64 - 2.0 * margin) / (max[0] - min[0]);
        let h = ((max[1] - min[1]) * scale + 2.0 * margin).ceil() as usize;
        let h = h.div_ceil(SUPERSAMPLE) * SUPERSAMPLE;
        let mut color = vec![[0.0f32; 3]; w * h];
        for (row, line) in color.chunks_mut(w).enumerate() {
            let t = row as f32 / h as f32;
            line.fill([0.78 + 0.16 * t, 0.84 + 0.12 * t, 0.91 + 0.06 * t]);
        }
        Self {
            w,
            h,
            right,
            up,
            toward,
            scale,
            offset: [margin - min[0] * scale, margin - min[1] * scale],
            depth: vec![f32::NEG_INFINITY; w * h],
            color,
            triangles: 0,
        }
    }

    fn project(&self, p: V3) -> V3 {
        [
            dot(p, self.right) * self.scale + self.offset[0],
            -dot(p, self.up) * self.scale + self.offset[1],
            dot(p, self.toward),
        ]
    }

    fn tri(&mut self, p: [V3; 3], c: [f32; 3]) {
        let v = p.map(|q| self.project(q));
        let edge =
            |a: V3, b: V3, x: f64, y: f64| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        let area = edge(v[0], v[1], v[2][0], v[2][1]);
        if area.abs() < 1e-12 {
            return;
        }
        self.triangles += 1;
        let lo = |a: usize| v.iter().map(|q| q[a]).fold(f64::MAX, f64::min);
        let hi = |a: usize| v.iter().map(|q| q[a]).fold(f64::MIN, f64::max);
        let x0 = lo(0).floor().max(0.0) as usize;
        let y0 = lo(1).floor().max(0.0) as usize;
        let x1 = (hi(0).ceil().max(0.0) as usize).min(self.w - 1);
        let y1 = (hi(1).ceil().max(0.0) as usize).min(self.h - 1);
        const EPS: f64 = -1e-9;
        for py in y0..=y1 {
            let y = py as f64 + 0.5;
            for px in x0..=x1 {
                let x = px as f64 + 0.5;
                let w0 = edge(v[1], v[2], x, y) / area;
                let w1 = edge(v[2], v[0], x, y) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < EPS || w1 < EPS || w2 < EPS {
                    continue;
                }
                let z = (w0 * v[0][2] + w1 * v[1][2] + w2 * v[2][2]) as f32;
                let i = py * self.w + px;
                if z > self.depth[i] {
                    self.depth[i] = z;
                    self.color[i] = c;
                }
            }
        }
    }

    fn quad(&mut self, p: [V3; 4], c: [f32; 3]) {
        self.tri([p[0], p[1], p[2]], c);
        self.tri([p[0], p[2], p[3]], c);
    }

    fn finish(self) -> (u32, u32, Vec<u8>, usize) {
        let (w, h) = (self.w / SUPERSAMPLE, self.h / SUPERSAMPLE);
        let mut rgb = Vec::with_capacity(w * h * 3);
        let n = (SUPERSAMPLE * SUPERSAMPLE) as f32;
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 3];
                for sy in 0..SUPERSAMPLE {
                    for sx in 0..SUPERSAMPLE {
                        let c = self.color[(y * SUPERSAMPLE + sy) * self.w + x * SUPERSAMPLE + sx];
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                    }
                }
                rgb.extend(acc.map(|v| (v / n * 255.0).clamp(0.0, 255.0).round() as u8));
            }
        }
        (w as u32, h as u32, rgb, self.triangles)
    }
}

/// Sky-ambient plus sun term for a surface with normal `n`.
fn light(n: V3, sun: V3, shadow: f64) -> f64 {
    let ambient = 0.45 * (0.6 + 0.4 * n[1]);
    ambient + 0.7 * dot(n, sun).max(0.0) * shadow
}

const SOIL: [f64; 3] = [0.50, 0.39, 0.29];
const ROCK: [f64; 3] = [0.50, 0.47, 0.44];
const FLOATER: [f64; 3] = [0.90, 0.20, 0.75];

/// One voxel column: solid below `k_lo`, `bits` inside the shell, air above.
#[derive(Clone, Default)]
struct Col<C> {
    column: C,
    k_lo: i64,
    bits: Vec<bool>,
    /// Highest solid voxel index.
    top: i64,
}

impl<C> Col<C> {
    fn solid(&self, k: i64) -> bool {
        k < self.k_lo
            || (k - self.k_lo) < self.bits.len() as i64 && self.bits[(k - self.k_lo) as usize]
    }
}

struct Grid<C> {
    n: usize,
    vs: f64,
    /// World voxel index of the region's first column.
    ci: i64,
    cj: i64,
    k_base: i64,
    k_top: i64,
    cols: Vec<Col<C>>,
}

impl<C> Grid<C> {
    fn col(&self, i: i64, j: i64) -> Option<&Col<C>> {
        let n = self.n as i64;
        (i >= 0 && j >= 0 && i < n && j < n).then(|| &self.cols[(j * n + i) as usize])
    }

    /// Solid test by region-local column index and world layer index.
    fn solid(&self, i: i64, j: i64, k: i64) -> bool {
        self.col(i, j).is_some_and(|c| c.solid(k))
    }

    /// 1 if a world point sees the sun through the voxels, else 0.
    fn sun_visibility(&self, p: V3, sun: V3) -> f64 {
        let step = 0.5 * self.vs;
        let top = (self.k_top + 1) as f64 * self.vs;
        let mut t = step;
        loop {
            let q = [p[0] + sun[0] * t, p[1] + sun[1] * t, p[2] + sun[2] * t];
            if q[1] > top {
                return 1.0;
            }
            let i = (q[0] / self.vs).floor() as i64 - self.ci;
            let j = (q[2] / self.vs).floor() as i64 - self.cj;
            let k = (q[1] / self.vs).floor() as i64;
            match self.col(i, j) {
                None => return 1.0,
                Some(c) if k < self.k_base || c.solid(k) => return 0.0,
                _ => {}
            }
            t += step;
        }
    }

    /// Mark solid voxels reachable from the ground (6-connected) over layers
    /// `k_base..=k_top`. Returns the visited mask and the count of floaters.
    fn flood(&self) -> Option<(Vec<bool>, usize)> {
        let n = self.n;
        let layers = (self.k_top - self.k_base + 1) as usize;
        let size = n * n * layers;
        if size > 150_000_000 {
            return None;
        }
        let idx = |i: usize, j: usize, k: usize| (k * n + j) * n + i;
        let mut seen = vec![false; size];
        let mut stack: Vec<(u32, u32, u32)> = Vec::new();
        for j in 0..n {
            for i in 0..n {
                seen[idx(i, j, 0)] = true;
                stack.push((i as u32, j as u32, 0));
            }
        }
        const NEIGHBORS: [(i64, i64, i64); 6] = [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ];
        while let Some((i, j, k)) = stack.pop() {
            let (i, j, k) = (i as i64, j as i64, k as i64);
            for (di, dj, dk) in NEIGHBORS {
                let (a, b, c) = (i + di, j + dj, k + dk);
                if a < 0 || b < 0 || c < 0 || a >= n as i64 || b >= n as i64 || c >= layers as i64 {
                    continue;
                }
                let id = idx(a as usize, b as usize, c as usize);
                if !seen[id] && self.solid(a, b, c + self.k_base) {
                    seen[id] = true;
                    stack.push((a as u32, b as u32, c as u32));
                }
            }
        }
        let mut floating = 0;
        for j in 0..n {
            for i in 0..n {
                let c = &self.cols[j * n + i];
                for k in c.k_lo.max(self.k_base)..=c.top {
                    if c.solid(k) && !seen[idx(i, j, (k - self.k_base) as usize)] {
                        floating += 1;
                    }
                }
            }
        }
        Some((seen, floating))
    }
}

/// The view's voxel columns: `n × n` columns of size `vs`, starting at world
/// column index `(ci, cj)`. Preview voxels are a whole multiple (`mult`) of the
/// engine voxel size.
pub struct ColumnGrid {
    pub n: usize,
    pub vs: f64,
    pub ci: i64,
    pub cj: i64,
    pub mult: f64,
}

impl ColumnGrid {
    /// World (x, z) of column `(i, j)`'s centre.
    pub fn center(&self, i: usize, j: usize) -> (f64, f64) {
        (
            (self.ci + i as i64) as f64 * self.vs + 0.5 * self.vs,
            (self.cj + j as i64) as f64 * self.vs + 0.5 * self.vs,
        )
    }
}

pub fn column_grid(s: &IsoSettings) -> ColumnGrid {
    let mult = (s.extent / (s.max_columns as f64 * s.voxel_size))
        .ceil()
        .max(1.0);
    let vs = s.voxel_size * mult;
    let n = (s.extent / vs).round().max(4.0) as usize;
    ColumnGrid {
        n,
        vs,
        ci: ((s.center[0] - 0.5 * n as f64 * vs) / vs).floor() as i64,
        cj: ((s.center[1] - 0.5 * n as f64 * vs) / vs).floor() as i64,
        mult,
    }
}

/// Face directions: +y, -z, +z, -x, +x (bottom faces never face the camera).
const DIRS: [(V3, i64, i64, i64); 5] = [
    ([0.0, 1.0, 0.0], 0, 0, 1),
    ([0.0, 0.0, -1.0], 0, -1, 0),
    ([0.0, 0.0, 1.0], 0, 1, 0),
    ([-1.0, 0.0, 0.0], -1, 0, 0),
    ([1.0, 0.0, 0.0], 1, 0, 0),
];

#[derive(Clone, Copy, Default)]
struct Face {
    i: u32,
    j: u32,
    k: i64,
    dir: u8,
}

pub fn render<S: VoxelSource>(src: &S, s: &IsoSettings) -> Result<Rendered, String> {
    if !(s.extent > 0.0 && s.exaggeration > 0.0 && (1.0..=89.0).contains(&s.pitch)) {
        return Err("iso: need extent > 0, exaggeration > 0, pitch in 1..=89".into());
    }
    if !(64..=8192).contains(&s.width) || !(8..=4096).contains(&s.max_columns) {
        return Err("iso: width must be 64..=8192 and max_columns 8..=4096".into());
    }
    let ColumnGrid {
        n,
        vs,
        ci,
        cj,
        mult,
    } = column_grid(s);
    let smooth = match s.shading {
        Shading::Auto => mult > 1.0,
        Shading::Face => false,
        Shading::Smooth => true,
    };

    let cols = par_map(n * n, |idx| {
        let (i, j) = ((idx % n) as i64, (idx / n) as i64);
        let column = src.column(
            (ci + i) as f64 * vs + 0.5 * vs,
            (cj + j) as f64 * vs + 0.5 * vs,
        );
        let (lo, hi) = src.shell(&column);
        // Voxel k spans [k vs, (k+1) vs] and samples its centre.
        let k_lo = (lo / vs - 0.5).ceil() as i64;
        let k_hi = (hi / vs - 0.5).floor() as i64;
        let bits: Vec<bool> = (k_lo..=k_hi)
            .map(|k| src.density(&column, (k as f64 + 0.5) * vs) > 0.0)
            .collect();
        let top = bits
            .iter()
            .rposition(|&b| b)
            .map_or(k_lo - 1, |p| k_lo + p as i64);
        Col {
            column,
            k_lo,
            bits,
            top,
        }
    });
    // Lowest voxel that can be exposed: the lowest air pocket in any shell,
    // otherwise the column top. Everything deeper is solid and hidden.
    let min_lo = cols
        .iter()
        .map(|c| {
            c.bits
                .iter()
                .position(|&b| !b)
                .map_or(c.top, |p| c.k_lo + p as i64 - 1)
                .min(c.top)
        })
        .min()
        .unwrap();
    let min_top = cols.iter().map(|c| c.top).min().unwrap();
    let k_top = cols.iter().map(|c| c.top).max().unwrap();
    let span = (k_top - min_lo).max(1);
    let thickness = 2
        .max((0.15 * span as f64) as i64)
        .max((0.03 * n as f64) as i64);
    let grid = Grid {
        n,
        vs,
        ci,
        cj,
        k_base: min_lo - thickness,
        k_top,
        cols,
    };
    let flood = if s.mark_floaters { grid.flood() } else { None };

    // Collect exposed, camera-facing faces.
    let (yaw, pitch) = (s.yaw.to_radians(), s.pitch.to_radians());
    let toward = [
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        yaw.cos() * pitch.cos(),
    ];
    let visible: Vec<usize> = (0..DIRS.len())
        .filter(|&d| dot(DIRS[d].0, toward) > 0.0)
        .collect();
    let mut faces = Vec::new();
    for j in 0..n as i64 {
        for i in 0..n as i64 {
            let c = grid.col(i, j).unwrap();
            let mut k_start = c.k_lo.min(c.top);
            for &d in &visible {
                let (_, di, dj, _) = DIRS[d];
                k_start = k_start.min(grid.col(i + di, j + dj).map_or(grid.k_base, |nc| nc.k_lo));
            }
            for k in k_start.max(grid.k_base)..=c.top {
                if !c.solid(k) {
                    continue;
                }
                for &d in &visible {
                    let (_, di, dj, dk) = DIRS[d];
                    if !grid.solid(i + di, j + dj, k + dk) {
                        faces.push(Face {
                            i: i as u32,
                            j: j as u32,
                            k,
                            dir: d as u8,
                        });
                    }
                }
            }
        }
    }

    let e = s.exaggeration;
    let height_span = ((k_top - min_top + 1) as f64 * vs).max(1e-9);
    let height_t = |top: i64| (top - min_top + 1) as f64 * vs / height_span;
    let colors = par_map(faces.len(), |f| {
        let Face { i, j, k, dir } = faces[f];
        let (i, j, d) = (i as i64, j as i64, dir as usize);
        let c = grid.col(i, j).unwrap();
        let (fnorm, di, dj, _) = DIRS[d];
        let center = [
            (ci + i) as f64 * vs + 0.5 * vs,
            (k as f64 + 0.5) * vs,
            (cj + j) as f64 * vs + 0.5 * vs,
        ];
        let boundary = d > 0 && grid.col(i + di, j + dj).is_none();
        let dn = normalize(src.gradient(&c.column, center[1]).map(|v| -v));
        let normal = if smooth && !boundary && dot(dn, fnorm) > 0.0 {
            // Render space is y-scaled, so normals scale y by 1/e.
            normalize([dn[0], dn[1] / e, dn[2]])
        } else {
            fnorm
        };
        let floating = flood.as_ref().is_some_and(|(seen, _)| {
            k >= grid.k_base
                && !seen[((k - grid.k_base) as usize * n + j as usize) * n + i as usize]
        });
        let parity = ((ci + i + cj + j + k) & 1) as f64;
        let base = if floating {
            FLOATER
        } else if k == c.top {
            let slope = if boundary {
                0.0
            } else {
                (1.0 - dn[1] * dn[1]).max(0.0).sqrt() / dn[1].max(1e-3)
            };
            terrain_color(height_t(c.top), slope)
        } else if boundary {
            SOIL
        } else {
            ROCK
        };
        let tint = if d == 0 {
            0.965 + 0.07 * parity
        } else {
            0.9 + 0.04 * parity
        };
        let vis = if s.shadows {
            // Start just outside the face so the voxel doesn't shadow itself.
            let p = [
                center[0] + fnorm[0] * 0.51 * vs,
                center[1] + fnorm[1] * 0.51 * vs,
                center[2] + fnorm[2] * 0.51 * vs,
            ];
            grid.sun_visibility(p, s.sun)
        } else {
            1.0
        };
        let l = light(normal, s.sun, vis) * tint;
        base.map(|v| (v * l) as f32)
    });

    let mid = 0.5 * (min_top + k_top + 1) as f64 * vs;
    let ox = ci as f64 * vs - s.center[0];
    let oz = cj as f64 * vs - s.center[1];
    let y_r = |k: i64| (k as f64 * vs - mid) * e;
    let mut r = Raster::new(
        s,
        toward,
        [ox, y_r(grid.k_base), oz],
        [ox + n as f64 * vs, y_r(k_top + 1), oz + n as f64 * vs],
    );
    for (face, color) in faces.iter().zip(&colors) {
        let x0 = ox + face.i as f64 * vs;
        let z0 = oz + face.j as f64 * vs;
        let (x1, z1) = (x0 + vs, z0 + vs);
        let (y0, y1) = (y_r(face.k), y_r(face.k + 1));
        let quad = match face.dir {
            0 => [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
            1 => [[x0, y1, z0], [x1, y1, z0], [x1, y0, z0], [x0, y0, z0]],
            2 => [[x0, y1, z1], [x1, y1, z1], [x1, y0, z1], [x0, y0, z1]],
            3 => [[x0, y1, z0], [x0, y1, z1], [x0, y0, z1], [x0, y0, z0]],
            _ => [[x1, y1, z0], [x1, y1, z1], [x1, y0, z1], [x1, y0, z0]],
        };
        r.quad(quad, *color);
    }
    let (width, height, rgb, triangles) = r.finish();
    Ok(Rendered {
        width,
        height,
        rgb,
        triangles,
        columns: n,
        voxel: vs,
        faces: faces.len(),
        floating: flood.map(|(_, f)| f),
    })
}
