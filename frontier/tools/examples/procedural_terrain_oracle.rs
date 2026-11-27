//! Independent f64 native-column traversal for the Alpine seed 7 diagnostic.
//! Arguments are camera eye, target, width/height/FOV/far, and raw RGB path.
//! The raw RGB must come from FRONTIER_PROCEDURAL_HIT_CAPTURE=1 and --capture-linear.
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain, VOXEL_SIZE};
use std::sync::atomic::{AtomicUsize, Ordering};

fn ray(source: &FbmTerrain, o: [f64; 3], d: [f64; 3], near: f64, far: f64) -> Option<f64> {
    let mut t = near;
    let mut cell = [0, 2].map(|a| ((o[a] + d[a] * near) / VOXEL_SIZE).floor() as i64);
    let step = [0, 2].map(|a| if d[a] > 0. { 1 } else { -1 });
    while t < far {
        let edge = std::array::from_fn::<_, 2, _>(|j| {
            let a = j * 2;
            if d[a] == 0. {
                return f64::INFINITY;
            }
            let plane = (cell[j] + i64::from(step[j] > 0)) as f64 * VOXEL_SIZE;
            (plane - o[a]) / d[a]
        });
        let exit = far.min(edge[0].min(edge[1]));
        if exit > t {
            let h = source.height_cells(cell[0], cell[1]) as f64 * VOXEL_SIZE;
            if o[1] + t * d[1] <= h {
                return Some(t);
            }
            if d[1] < 0. {
                let top = (h - o[1]) / d[1];
                if top >= t && top < exit {
                    return Some(top);
                }
            }
        }
        if exit >= far {
            return None;
        }
        if edge[0] <= edge[1] {
            cell[0] += step[0];
        }
        if edge[1] <= edge[0] {
            cell[1] += step[1];
        }
        t = exit;
    }
    None
}

fn main() {
    let mut it = std::env::args().skip(1);
    let floats = |s: String| {
        s.split(',')
            .map(|v| v.parse::<f64>().unwrap())
            .collect::<Vec<_>>()
    };
    let eye = floats(it.next().unwrap());
    let target = floats(it.next().unwrap());
    let field = floats(it.next().unwrap());
    let rgb = std::fs::read(it.next().expect("expected the float RGB capture path")).unwrap();
    let o = [eye[0], eye[1], eye[2]];
    let (w, h) = (field[0] as usize, field[1] as usize);
    assert_eq!(rgb.len(), w * h * 12);
    let normalize = |v: [f64; 3]| {
        let len = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        v.map(|x| x / len)
    };
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let forward = normalize([target[0] - o[0], target[1] - o[1], target[2] - o[2]]);
    let right = normalize(cross(forward, [0., 1., 0.]));
    let up = cross(right, forward);
    let fov = (field[2].to_radians() / 2.).tan();
    let source = FbmTerrain::new(
        7,
        FbmParams {
            recipe: FbmRecipe::Alpine,
            ..Default::default()
        },
    )
    .unwrap();
    let pixel = |x: usize, y: usize, c: usize| {
        let i = ((y * w + x) * 3 + c) * 4;
        f32::from_le_bytes(rgb[i..i + 4].try_into().unwrap())
    };
    let mut samples = Vec::new();
    for x in (8..w).step_by((w / 40).max(1)) {
        for y in (8..h).step_by((h / 23).max(1)) {
            samples.push((x, y));
        }
        if let Some(y) = (0..h).find(|&y| pixel(x, y, 0) > 1. && pixel(x, y, 2) == 0.) {
            for iy in y.saturating_sub(3)..(y + 4).min(h) {
                samples.push((x, iy));
            }
        }
    }
    samples.sort_unstable();
    samples.dedup();
    let next = AtomicUsize::new(0);
    println!("x,y,cpu_distance,gpu_distance,gpu_status");
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(x, y)) = samples.get(i) else {
                    break;
                };
                let ndx = (2. * (x as f64 + 0.5) / w as f64 - 1.) * fov * w as f64 / h as f64;
                let ndy = (1. - 2. * (y as f64 + 0.5) / h as f64) * fov;
                let d = normalize(std::array::from_fn(|j| {
                    forward[j] + right[j] * ndx + up[j] * ndy
                }));
                let factor = (0..3).map(|j| forward[j] * d[j]).sum::<f64>();
                let cpu = ray(&source, o, d, 0.025 / factor, field[3] / factor);
                let gpu = pixel(x, y, 0);
                let status = if pixel(x, y, 2) == 1. && gpu == 1. {
                    "unresolved"
                } else if pixel(x, y, 2) == 0. {
                    "hit"
                } else {
                    "sky"
                };
                println!("{x},{y},{},{},{}", cpu.unwrap_or(-1.), gpu, status);
            });
        }
    });
}
