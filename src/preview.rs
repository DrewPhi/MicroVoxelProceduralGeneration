//! On-demand diagnostic images. The local view is an exact voxel DDA reference,
//! not the proposed analytic renderer or a distance-based geometry LOD system.
use crate::terrain::{Params, Terrain};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    io::{BufWriter, Write},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum View {
    Isometric,
    Overview,
    Local,
    Slice,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Inspection {
    pub x: f64,
    pub z: f64,
    pub span: f64,
    pub yaw: f64,
    pub elevation: f64,
    pub slice_depth: f64,
}
impl Default for Inspection {
    fn default() -> Self {
        Self {
            x: 5000.,
            z: 5000.,
            span: 32.,
            yaw: 135.,
            elevation: 25.,
            slice_depth: 100.,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub version: u32,
    pub terrain: Params,
    pub inspection: Inspection,
    pub view: View,
    pub width: usize,
}
impl Default for Recipe {
    fn default() -> Self {
        Self {
            version: 1,
            terrain: Params::default(),
            inspection: Inspection::default(),
            view: View::Overview,
            width: 512,
        }
    }
}
impl Recipe {
    pub fn validate(&self) -> Result<(), String> {
        self.terrain.validate()?;
        let i = &self.inspection;
        if self.version != 1
            || !(64..=1024).contains(&self.width)
            || [i.x, i.z, i.span, i.yaw, i.elevation, i.slice_depth]
                .iter()
                .any(|v| !v.is_finite())
            || i.x < 0.
            || i.z < 0.
            || i.x >= self.terrain.world_m
            || i.z >= self.terrain.world_m
            || !(4. ..=256.).contains(&i.span)
            || !(5. ..=85.).contains(&i.elevation)
            || !(1. ..=1000.).contains(&i.slice_depth)
        {
            return Err("Invalid inspection settings or recipe version".into());
        }
        Ok(())
    }
}
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub description: String,
}
fn cancelled(token: &AtomicU64, id: u64) -> bool {
    token.load(Ordering::Relaxed) != id
}
pub fn render(recipe: &Recipe, token: Arc<AtomicU64>, id: u64) -> Result<Picture, String> {
    recipe.validate()?;
    let terrain = Terrain::new(recipe.terrain.clone());
    let w = recipe.width;
    let h = if recipe.view == View::Overview {
        w
    } else {
        w * 3 / 4
    };
    let mut rgba = vec![0u8; w * h * 4];
    let i = &recipe.inspection;
    let s = terrain.p.voxel_m();
    let target = [i.x, terrain.base_height(i.x, i.z), i.z];
    let yaw = i.yaw.to_radians();
    let el = if recipe.view == View::Isometric {
        (1.0f64 / 2.0f64.sqrt()).atan()
    } else {
        i.elevation.to_radians()
    };
    let eye = add(
        target,
        [
            i.span * 1.3 * yaw.cos() * el.cos(),
            i.span * 1.3 * el.sin(),
            i.span * 1.3 * yaw.sin() * el.cos(),
        ],
    );
    let forward = norm(sub(target, eye));
    let right = norm(cross(forward, [0., 1., 0.]));
    let up = cross(right, forward);
    let low = [
        ((i.x - i.span / 2.).max(0.) / s).ceil() * s,
        ((target[1] - i.span).max(0.) / s).ceil() * s,
        ((i.z - i.span / 2.).max(0.) / s).ceil() * s,
    ];
    let high = [
        ((i.x + i.span / 2.).min(terrain.p.world_m) / s).floor() * s,
        ((target[1] + i.span).min(terrain.p.ceiling_m) / s).floor() * s,
        ((i.z + i.span / 2.).min(terrain.p.world_m) / s).floor() * s,
    ];
    rgba.par_chunks_mut(w * 4)
        .enumerate()
        .for_each(|(py, row)| {
            if cancelled(&token, id) {
                return;
            }
            for (px, pixel) in row.chunks_exact_mut(4).enumerate() {
                if px % 16 == 0 && cancelled(&token, id) {
                    return;
                }
                let color = match recipe.view {
                    View::Overview => {
                        let x = (px as f64 + 0.5) / w as f64 * terrain.p.world_m;
                        let z = (py as f64 + 0.5) / h as f64 * terrain.p.world_m;
                        let y = terrain.base_height(x, z);
                        let e = terrain.p.world_m / w as f64;
                        let dx = (terrain.base_height(x + e, z) - terrain.base_height(x - e, z))
                            / (2. * e);
                        let dz = (terrain.base_height(x, z + e) - terrain.base_height(x, z - e))
                            / (2. * e);
                        shade(surface_palette(y), norm([-dx, 1., -dz]), 0)
                    }
                    View::Slice => {
                        let x = i.x + ((px as f64 + 0.5) / w as f64 - 0.5) * i.span;
                        let y = target[1] + i.span * 0.25
                            - (py as f64 + 0.5) / h as f64 * i.slice_depth;
                        let c = [
                            (x / s).floor() as i64,
                            (y / s).floor() as i64,
                            (i.z / s).floor() as i64,
                        ];
                        if terrain.solid(c) {
                            shade(voxel_palette(&terrain, c), [0., 0., 1.], hash(c))
                        } else {
                            [22, 31, 43]
                        }
                    }
                    View::Local | View::Isometric => {
                        let u =
                            (2. * (px as f64 + 0.5) / w as f64 - 1.) * (w as f64 / h as f64) * 0.55;
                        let v = (1. - 2. * (py as f64 + 0.5) / h as f64) * 0.55;
                        let (origin, d) = if recipe.view == View::Isometric {
                            // Parallel rays; the pixel changes origin, not direction.
                            (
                                add(
                                    eye,
                                    add(mul(right, u * i.span * 2.1), mul(up, v * i.span * 2.1)),
                                ),
                                forward,
                            )
                        } else {
                            (eye, norm(add(forward, add(mul(right, u), mul(up, v)))))
                        };
                        match cast(&terrain, origin, d, low, high, &token, id) {
                            Some((c, n)) => shade(voxel_palette(&terrain, c), n, hash(c)),
                            None => [155, 190, 215],
                        }
                    }
                };
                pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
            }
        });
    if cancelled(&token, id) {
        return Err("Cancelled".into());
    }
    let description=match recipe.view {
        View::Overview=>format!("10D height diagnostic: {:.2} km square, {:.2} m/pixel; base height only (caves/overhangs in local views)",terrain.p.world_m/1000.,terrain.p.world_m/w as f64).replace("10D","2D"),
        View::Isometric=>format!("Isometric voxel inspection · {:.3} cm cells · {:.1} m cropped region · fixed 35.26° elevation",terrain.p.voxel_cm,i.span),
        View::Local=>format!("Exact {:.3} cm voxel occupancy · {:.1} m wide cropped region · CPU reference",terrain.p.voxel_cm,i.span),
        View::Slice=>format!("Voxel occupancy slice at Z={:.2} m · {:.1} m wide × {:.1} m deep",i.z,i.span,i.slice_depth)
    };
    Ok(Picture {
        width: w,
        height: h,
        rgba,
        description,
    })
}
/// Exact cell-boundary traversal through a finite inspection box. No density
/// interpolation or sphere-tracing assumption. Ties advance all crossed axes.
fn cast(
    t: &Terrain,
    o: [f64; 3],
    d: [f64; 3],
    low: [f64; 3],
    high: [f64; 3],
    token: &AtomicU64,
    id: u64,
) -> Option<([i64; 3], [f64; 3])> {
    if (0..3).any(|a| low[a] >= high[a]) {
        return None;
    }
    let (mut enter, mut exit) = (0f64, f64::INFINITY);
    let mut normal = [0.; 3];
    for a in 0..3 {
        if d[a].abs() < 1e-12 {
            if o[a] < low[a] || o[a] >= high[a] {
                return None;
            }
            continue;
        }
        let a0 = (low[a] - o[a]) / d[a];
        let a1 = (high[a] - o[a]) / d[a];
        if a0.min(a1) > enter {
            enter = a0.min(a1);
            normal = [0.; 3];
            normal[a] = -d[a].signum();
        }
        exit = exit.min(a0.max(a1));
    }
    if enter >= exit {
        return None;
    }
    let s = t.p.voxel_m();
    let p = add(o, mul(d, enter + s * 1e-7));
    let mut c = p.map(|v| (v / s).floor() as i64);
    let step = d.map(|v| {
        if v > 0. {
            1
        } else if v < 0. {
            -1
        } else {
            0
        }
    });
    let delta = d.map(|v| if v == 0. { f64::INFINITY } else { s / v.abs() });
    let mut next = std::array::from_fn::<_, 3, _>(|a| {
        if step[a] == 0 {
            f64::INFINITY
        } else {
            ((c[a] + if step[a] > 0 { 1 } else { 0 }) as f64 * s - o[a]) / d[a]
        }
    });
    let mut counter = 0;
    loop {
        if counter % 64 == 0 && cancelled(token, id) {
            return None;
        }
        counter += 1;
        if t.solid(c) {
            return Some((c, normal));
        }
        let hit = next.into_iter().fold(f64::INFINITY, f64::min);
        if hit >= exit {
            return None;
        }
        normal = [0.; 3];
        let mut chosen = false;
        for a in 0..3 {
            if next[a] <= hit + 1e-10 {
                c[a] += step[a];
                next[a] += delta[a];
                if !chosen {
                    normal[a] = -step[a] as f64;
                    chosen = true;
                }
            }
        }
    }
}
fn hash(c: [i64; 3]) -> u64 {
    let mut h = (c[0] as u64).wrapping_mul(73856093)
        ^ (c[1] as u64).wrapping_mul(19349663)
        ^ (c[2] as u64).wrapping_mul(83492791);
    h ^= h >> 13;
    h.wrapping_mul(1274126177)
}
/// Albedo depends on the voxel, never on the face being rendered.
fn voxel_palette(t: &Terrain, c: [i64; 3]) -> [f64; 3] {
    let s = t.p.voxel_m();
    let point = c.map(|v| (v as f64 + 0.5) * s);
    if t.surface_depth(point) > 0.15 {
        [140., 140., 145.]
    } else {
        surface_palette(point[1])
    }
}
fn surface_palette(y: f64) -> [f64; 3] {
    if y > 510. {
        [220., 224., 222.]
    } else if y > 410. {
        [144., 140., 126.]
    } else {
        [110., 140., 78.]
    }
}
fn shade(base: [f64; 3], n: [f64; 3], hash: u64) -> [u8; 3] {
    let light = 0.38 + 0.62 * dot(n, norm([-0.4, 0.8, -0.3])).max(0.);
    let grain = 0.96 + (hash % 100) as f64 * 0.0008;
    base.map(|v| (v * light * grain).clamp(0., 255.) as u8)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn mul(a: [f64; 3], v: f64) -> [f64; 3] {
    a.map(|a| a * v)
}
fn norm(a: [f64; 3]) -> [f64; 3] {
    mul(a, 1. / dot(a, a).sqrt().max(1e-15))
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn save_png(p: &Picture, path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), p.width as u32, p.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .map_err(|e| e.to_string())?
        .write_image_data(&p.rgba)
        .map_err(|e| e.to_string())
}
/// Small native-resolution fixture for fitting: bit x-fastest, then z, then y.
/// Header: magic[8], JSON byte length u32 LE, JSON, packed occupancy (LSB first).
pub fn export_voxels(
    recipe: &Recipe,
    path: &Path,
    token: &AtomicU64,
    id: u64,
) -> Result<(), String> {
    recipe.validate()?;
    let t = Terrain::new(recipe.terrain.clone());
    let s = t.p.voxel_m();
    let n = (16. / s).floor().max(1.) as usize;
    let count = n.checked_pow(3).ok_or("Export too large")?;
    if count > 32 * 1024 * 1024 {
        return Err("16 m fixture exceeds 32 million cells; increase voxel size for export".into());
    }
    let i = &recipe.inspection;
    let origin = [
        ((i.x - 8.) / s).floor() as i64,
        ((t.base_height(i.x, i.z) - 12.) / s).floor() as i64,
        ((i.z - 8.) / s).floor() as i64,
    ];
    let header=serde_json::to_vec(&serde_json::json!({"version":1,"recipe":recipe,"origin_cells":origin,"dimensions":[n,n,n],"order":"x,z,y; LSB-first bits","sampling":"cell-center; density>0; outside world is air"})).map_err(|e|e.to_string())?;
    let mut bytes = vec![0u8; count.div_ceil(8)];
    for (bi, byte) in bytes.iter_mut().enumerate() {
        if bi % 1024 == 0 && cancelled(token, id) {
            return Err("Cancelled".into());
        }
        for bit in 0..8 {
            let a = bi * 8 + bit;
            if a >= count {
                break;
            }
            let c = [
                origin[0] + (a % n) as i64,
                origin[1] + (a / (n * n)) as i64,
                origin[2] + ((a / n) % n) as i64,
            ];
            if t.solid(c) {
                *byte |= 1 << bit;
            }
        }
    }
    if cancelled(token, id) {
        return Err("Cancelled".into());
    }
    let temporary = path.with_extension(format!("fbmvox.{}.tmp", std::process::id()));
    let mut out = BufWriter::new(std::fs::File::create(&temporary).map_err(|e| e.to_string())?);
    out.write_all(b"FBMVOX01").map_err(|e| e.to_string())?;
    out.write_all(&(header.len() as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&header).map_err(|e| e.to_string())?;
    out.write_all(&bytes).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dda_hits_voxel_boundary_of_flat_ground() {
        let p = Params {
            base_m: 10.,
            relief_m: 0.,
            overhang_m: 0.,
            cave_width: 0.,
            voxel_cm: 25.,
            ..Default::default()
        };
        let t = Terrain::new(p);
        let token = AtomicU64::new(1);
        let hit = cast(
            &t,
            [5.125, 20., 5.125],
            [0., -1., 0.],
            [0., 0., 0.],
            [16., 32., 16.],
            &token,
            1,
        )
        .unwrap();
        assert_eq!(hit, ([20, 39, 20], [0., 1., 0.]));
        assert!(cast(
            &t,
            [5., 20., 5.],
            [0., 1., 0.],
            [0., 0., 0.],
            [16., 32., 16.],
            &token,
            1
        )
        .is_none());
    }
    #[test]
    fn recipe_roundtrip_and_cancellation() {
        let r = Recipe {
            width: 64,
            ..Default::default()
        };
        let loaded: Recipe = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r.terrain, loaded.terrain);
        assert!(render(&r, Arc::new(AtomicU64::new(2)), 1).is_err());
    }
    #[test]
    fn dda_matches_explicit_voxel_boxes_for_random_oblique_rays() {
        let t = Terrain::new(Params {
            world_m: 16.,
            ceiling_m: 16.,
            base_m: 3.,
            relief_m: 2.,
            wavelength_m: 10.,
            overhang_m: 1.,
            cave_scale_m: 3.,
            voxel_cm: 50.,
            ..Default::default()
        });
        let token = AtomicU64::new(1);
        let s = t.p.voxel_m();
        let (low, high) = ([4., 0., 4.], [8., 6., 8.]);
        let mut state = 42u64;
        let mut random = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 32) as f64 / u32::MAX as f64
        };
        for _ in 0..150 {
            let o = [2. + random() * 8., 7. + random(), 2. + random() * 8.];
            let d = norm(sub(
                [4. + random() * 4., random() * 3., 4. + random() * 4.],
                o,
            ));
            let mut nearest: Option<(f64, [i64; 3])> = None;
            for x in 8..16 {
                for y in 0..12 {
                    for z in 8..16 {
                        let c = [x, y, z];
                        if !t.solid(c) {
                            continue;
                        }
                        let (mut entry, mut exit) = (0f64, f64::INFINITY);
                        for a in 0..3 {
                            let first = (c[a] as f64 * s - o[a]) / d[a];
                            let last = ((c[a] + 1) as f64 * s - o[a]) / d[a];
                            entry = entry.max(first.min(last));
                            exit = exit.min(first.max(last));
                        }
                        if entry < exit && nearest.is_none_or(|(best, _)| entry < best) {
                            nearest = Some((entry, c));
                        }
                    }
                }
            }
            assert_eq!(
                cast(&t, o, d, low, high, &token, 1).map(|h| h.0),
                nearest.map(|h| h.1)
            );
        }
    }
    #[test]
    fn exported_fixture_matches_global_occupancy_and_cancel_preserves_existing_file() {
        let r = Recipe {
            terrain: Params {
                voxel_cm: 100.,
                ..Default::default()
            },
            ..Default::default()
        };
        let path =
            std::env::temp_dir().join(format!("fbm-fixture-test-{}.bin", std::process::id()));
        export_voxels(&r, &path, &AtomicU64::new(1), 1).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"FBMVOX01");
        let n = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&bytes[12..12 + n]).unwrap();
        let origin: Vec<i64> = header["origin_cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        let t = Terrain::new(r.terrain.clone());
        let data = &bytes[12 + n..];
        assert_eq!(data.len(), 512);
        for a in 0..4096 {
            let c = [
                origin[0] + (a % 16) as i64,
                origin[1] + (a / 256) as i64,
                origin[2] + ((a / 16) % 16) as i64,
            ];
            assert_eq!(data[a / 8] & (1 << (a % 8)) != 0, t.solid(c));
        }
        assert!(export_voxels(&r, &path, &AtomicU64::new(2), 1).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn grass_is_a_surface_skin_and_underground_cut_faces_are_stone() {
        let t = Terrain::new(Params {
            base_m: 10.,
            relief_m: 0.,
            overhang_m: 0.,
            cave_width: 0.,
            voxel_cm: 6.25,
            ..Default::default()
        });
        let top = voxel_palette(&t, [80, 159, 80]);
        let buried = voxel_palette(&t, [80, 140, 80]);
        assert!(top[1] > top[0] && top[1] > top[2]);
        assert_eq!(buried, [140., 140., 145.]);
        let mut p = t.p.clone();
        p.cave_width = 0.4;
        assert_eq!(voxel_palette(&Terrain::new(p), [80, 140, 80]), buried);
    }
}
