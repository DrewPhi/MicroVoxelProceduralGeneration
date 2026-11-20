//! Compare an opaque coverage capture with the canonical unedited Highlands
//! skyline on a level, unbanked flight. The sampling interval is in forward
//! metres; repeat at half the interval to measure reference convergence.
use frontier_world::{FbmParams, FbmRecipe, FbmTerrain};
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
    let usage="flight_skyline COVERAGE.f32 WIDTH HEIGHT EYE_X,Y,Z TARGET_X,Y,Z FOV_Y FAR_M SEED [FORWARD_STEP_M] [PIXEL_STRIDE] [--voxel-size METERS]\nUse an unedited default Highlands world with vegetation off, level pitch and zero bank.";
    let args: Vec<_> = frontier_world::voxel_grid_args(std::env::args().skip(1))?.collect();
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
        || (target[1] - eye[1]).abs() > 1e-7 * length
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
            "expected a level camera, finite positive spacing and valid image dimensions".into(),
        );
    }
    let coverage = std::fs::read(path)?;
    if width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(coverage.len()) {
        return Err("coverage size does not match image".into());
    }
    let source = FbmTerrain::new(
        seed,
        FbmParams {
            recipe: FbmRecipe::Highlands,
            ..Default::default()
        },
    )?;
    let scale = (fov.to_radians() * 0.5).tan();
    let mut errors = Vec::new();
    let mut signed = 0.;
    let mut over_five = 0;
    for x in (stride / 2..width).step_by(stride) {
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
        let measured = (0..height)
            .find(|y| {
                let i = 4 * (y * width + x);
                f32::from_le_bytes(coverage[i..i + 4].try_into().unwrap()) > 0.5
            })
            .unwrap_or(height);
        let error = measured as f64 - skyline.max(0.);
        signed += error;
        over_five += usize::from(error.abs() > 5.);
        errors.push(error.abs());
    }
    errors.sort_by(f64::total_cmp);
    let pct = |q: f64| errors[((errors.len() - 1) as f64 * q).round() as usize];
    println!("{{\"columns\":{},\"forward_step_m\":{step},\"pixel_stride\":{stride},\"p50_abs_px\":{:.4},\"p95_abs_px\":{:.4},\"max_abs_px\":{:.4},\"mean_signed_px\":{:.4},\"columns_over_five_px\":{over_five}}}",
        errors.len(),pct(0.5),pct(0.95),errors.last().unwrap(),signed/errors.len() as f64);
    Ok(())
}
