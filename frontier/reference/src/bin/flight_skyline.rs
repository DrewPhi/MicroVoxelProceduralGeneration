//! Compare an opaque coverage capture with the canonical unedited landscape
//! skyline in flight. Repeat at half the sampling interval to check convergence.
#[path = "../skyline.rs"]
mod skyline;
use frontier_reference::landscape::{LandscapeRays, SparseLandscape};
use frontier_world::{voxel_size, FbmParams, FbmRecipe, FbmTerrain};
use std::error::Error;

fn vector<const N: usize>(s: &str) -> Result<[f64; N], Box<dyn Error>> {
    let values = s
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<f64>, _>>()?;
    if values.len() != N || !values.iter().all(|x| x.is_finite()) {
        return Err("invalid finite vector".into());
    }
    Ok(values.try_into().unwrap())
}

fn main() -> Result<(), Box<dyn Error>> {
    let usage="flight_skyline COVERAGE.f32 WIDTH HEIGHT EYE_X,Y,Z TARGET_X,Y,Z FOV_Y FAR_M SEED [FORWARD_STEP_M] [PIXEL_STRIDE] [--terrain-recipe highlands|watershed] [--roll-degrees BANK] [--voxel-size METERS] [--depth DEPTH.f32 --depth-stride 40]\nUse an unedited world of the selected recipe (default Highlands) with vegetation and the exterior aircraft off. Banked or pitched views measure displacement along projected vertical world planes.";
    let mut input = frontier_world::voxel_grid_args(std::env::args().skip(1))?;
    let mut args = Vec::new();
    let mut recipe = FbmRecipe::Highlands;
    let mut roll = 0_f64;
    let mut depth_path = None;
    let mut depth_stride = 40usize;
    while let Some(arg) = input.next() {
        if arg == "--roll-degrees" {
            roll = input.next().ok_or("missing bank angle")?.parse()?;
        } else if arg == "--terrain-recipe" {
            recipe = match input.next().as_deref() {
                Some("highlands") => FbmRecipe::Highlands,
                Some("watershed") => FbmRecipe::Watershed,
                _ => return Err("expected --terrain-recipe highlands|watershed".into()),
            };
        } else if arg == "--depth" {
            depth_path = Some(input.next().ok_or("missing depth file")?);
        } else if arg == "--depth-stride" {
            depth_stride = input.next().ok_or("missing depth sample stride")?.parse()?;
            if depth_stride == 0 {
                return Err("zero depth sample stride".into());
            }
        } else {
            args.push(arg);
        }
    }
    if args.first().is_some_and(|s| s == "--help") {
        println!("{usage}");
        return Ok(());
    }
    if !(8..=10).contains(&args.len()) {
        return Err(usage.into());
    }
    let path = &args[0];
    let width = args[1].parse::<usize>()?;
    let height = args[2].parse::<usize>()?;
    let eye = vector::<3>(&args[3])?;
    let target = vector::<3>(&args[4])?;
    let fov = args[5].parse::<f64>()?;
    let far = args[6].parse::<f64>()?;
    let seed = args[7].parse::<u64>()?;
    let step = args.get(8).map_or(Ok(1.), |v| v.parse::<f64>())?;
    let stride = args.get(9).map_or(Ok(8), |v| v.parse::<usize>())?;
    let dx = target[0] - eye[0];
    let dz = target[2] - eye[2];
    let length = dx.hypot(dz);
    if !length.is_finite()
        || length <= 0.
        || !roll.is_finite()
        || width == 0
        || height == 0
        || !(0.1..179.).contains(&fov)
        || !far.is_finite()
        || far <= 0.025
        || !step.is_finite()
        || step <= 0.
        || far + step == far
        || stride == 0
        || stride > width
    {
        return Err(
            "expected a finite camera direction, positive spacing and valid image dimensions"
                .into(),
        );
    }
    let coverage = std::fs::read(path)?;
    if width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(coverage.len()) {
        return Err("coverage size does not match image".into());
    }
    let source = FbmTerrain::new(
        seed,
        FbmParams {
            recipe,
            ..Default::default()
        },
    )?;
    let scale = (fov.to_radians() * 0.5).tan();
    let mut errors = Vec::new();
    let mut signed = 0.;
    let mut over_five = 0;
    let tilted = roll != 0. || (target[1] - eye[1]).abs() > 1e-7 * length;
    let view = skyline::View::new(eye, target, roll, fov, width, height, far);
    let occupied = |x: usize, y: usize| {
        let i = 4 * (y * width + x);
        f32::from_le_bytes(coverage[i..i + 4].try_into().unwrap()) > 0.5
    };
    for x in (stride / 2..width).step_by(stride) {
        let error = if tilted {
            let Some(line) = view.meridian(x) else {
                continue;
            };
            line.measured(occupied) - line.reference(step, |x, z| source.height_m(x, z))
        } else {
            let lateral =
                (2. * (x as f64 + 0.5) / width as f64 - 1.) * scale * width as f64 / height as f64;
            // Camera uses a right-handed view along +Z, so positive screen X
            // points toward world -X at zero yaw.
            let ray = [(dx - lateral * dz) / length, (dz + lateral * dx) / length];
            let mut skyline = height as f64;
            let mut t = 0.025;
            loop {
                let h = source.height_m(eye[0] + t * ray[0], eye[2] + t * ray[1]);
                let row = (1. - (h - eye[1]) / (t * scale)) * 0.5 * height as f64;
                skyline = skyline.min(row);
                if t >= far || skyline <= 0. {
                    break;
                }
                t = (t + step).min(far);
            }
            let measured = (0..height).find(|&y| occupied(x, y)).unwrap_or(height);
            measured as f64 - skyline.max(0.)
        };
        signed += error;
        over_five += usize::from(error.abs() > 5.);
        errors.push(error.abs());
    }
    if errors.is_empty() {
        return Err("no visible sample planes".into());
    }
    let metric = if tilted {
        "projected_world_vertical"
    } else {
        "screen_vertical"
    };
    let spacing = if tilted {
        "radial_step_m"
    } else {
        "forward_step_m"
    };
    errors.sort_by(f64::total_cmp);
    let pct = |q: f64| errors[((errors.len() - 1) as f64 * q).round() as usize];
    println!("{{\"columns\":{},\"metric\":\"{metric}\",\"{spacing}\":{step},\"pixel_stride\":{stride},\"p50_abs_px\":{:.4},\"p95_abs_px\":{:.4},\"max_abs_px\":{:.4},\"mean_signed_px\":{:.4},\"columns_over_five_px\":{over_five}}}",
        errors.len(),pct(0.5),pct(0.95),errors.last().unwrap(),signed/errors.len() as f64);
    if let Some(path) = depth_path {
        let bytes = std::fs::read(path)?;
        if bytes.len() != coverage.len() {
            return Err("depth file size differs from coverage".into());
        }
        let mut min = [eye[0], eye[2]];
        let mut max = min;
        for x in [0., width as f64] {
            for y in [0., height as f64] {
                let d = view.ray(x, y);
                for (i, axis) in [0, 2].into_iter().enumerate() {
                    let q = eye[axis] + far * d[axis];
                    min[i] = min[i].min(q);
                    max[i] = max[i].max(q);
                }
            }
        }
        let origin = min.map(|v| (v / voxel_size()).floor() as i64 - 2);
        let size = (0..2)
            .map(|i| ((max[i] / voxel_size()).ceil() as i64 - origin[i] + 2) as usize)
            .max()
            .unwrap()
            .max(64)
            .next_power_of_two();
        let terrain = SparseLandscape::new(source, origin, size)?;
        let mut errors = Vec::new();
        let mut disagreement = 0;
        let mut count = 0;
        for y in (depth_stride / 2..height).step_by(depth_stride) {
            for x in (depth_stride / 2..width).step_by(depth_stride) {
                count += 1;
                let d = view.ray(x as f64 + 0.5, y as f64 + 0.5);
                let length = d.iter().map(|v| v * v).sum::<f64>().sqrt();
                let native = terrain.ray(eye, d, 0.025 * length, far * length);
                if native.is_some() != occupied(x, y) {
                    disagreement += 1;
                }
                if let Some(hit) = native.filter(|_| occupied(x, y)) {
                    let i = 4 * (y * width + x);
                    let value = f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
                    if !value.is_finite() || !(0. ..1.).contains(&value) {
                        return Err("invalid occupied depth".into());
                    }
                    // Captures convert reversed Z to forward f32. Give the GPU
                    // the whole rounding bin, including the last occupied bin.
                    let lo = 0.5 * (f64::from(value) + f64::from(value.next_down()));
                    let hi = if value.next_up() == 1. {
                        1.
                    } else {
                        0.5 * (f64::from(value) + f64::from(value.next_up()))
                    };
                    let reconstruct = |z: f64| 0.025 * far / (far - z * (far - 0.025));
                    let native_depth = hit.distance / length;
                    errors.push(
                        (reconstruct(lo) - native_depth)
                            .max(native_depth - reconstruct(hi))
                            .max(0.),
                    );
                }
            }
        }
        errors.sort_by(f64::total_cmp);
        if errors.is_empty() {
            return Err("no matching terrain depth samples".into());
        }
        let pct = |q: f64| errors[((errors.len() - 1) as f64 * q).round() as usize];
        println!("{{\"depth_metric\":\"forward_metres_outside_f32_rounding_bin\",\"depth_stride\":{depth_stride},\"rays\":{count},\"both_hit\":{},\"hit_mismatch\":{disagreement},\"p50_m\":{:.4},\"p95_m\":{:.4},\"max_m\":{:.4}}}",errors.len(),pct(0.5),pct(0.95),pct(1.));
    }
    Ok(())
}
