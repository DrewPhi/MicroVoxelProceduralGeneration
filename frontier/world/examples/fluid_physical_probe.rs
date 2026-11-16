//! Offline CPU reference probe; standalone rustc build needs no Cargo target.
#[allow(dead_code)]
#[path = "../src/physical_fluid/mod.rs"]
mod physical_fluid;
use physical_fluid::*;
use std::{path::PathBuf, time::Instant};
fn main() {
    if let Err(e) = run() {
        eprintln!("physical reference rejected update: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut cfg = PhysicalConfig::default();
    let mut fixture = "hydrostatic".to_string();
    let mut seconds: f64 = 0.1;
    let mut sample: f64 = 0.01;
    let mut solver_dt: Option<f64> = None;
    let mut frames: Option<PathBuf> = None;
    let mut checkpoint: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or("each flag requires a value")?;
        match arg.as_str() {
            "--fixture" => fixture = value,
            "--seconds" => seconds = value.parse()?,
            "--sample" => sample = value.parse()?,
            "--dt" => solver_dt = Some(value.parse()?),
            "--nx" => cfg.dims[0] = value.parse()?,
            "--ny" => cfg.dims[1] = value.parse()?,
            "--spacing" => cfg.spacing = value.parse()?,
            "--boundary" => {
                cfg.boundary = match value.as_str() {
                    "center" => BoundaryMode::Center,
                    "ghost" => BoundaryMode::Ghost,
                    _ => return Err("boundary must be center or ghost".into()),
                }
            }
            "--nz" => cfg.dims[2] = value.parse()?,
            "--transport" => {
                cfg.transport = match value.as_str() {
                    "geometric" => Transport::Geometric,
                    "donor" => Transport::DonorCell,
                    _ => return Err("transport must be geometric or donor".into()),
                }
            }
            "--advection" => {
                cfg.advection = match value.as_str() {
                    "sl" => VelocityAdvection::SemiLagrangian,
                    "maccormack" => VelocityAdvection::LimitedMacCormack,
                    _ => return Err("advection must be sl or maccormack".into()),
                }
            }
            "--frames" => frames = Some(value.into()),
            "--checkpoint" => checkpoint = Some(value.into()),
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    if !seconds.is_finite() || seconds <= 0. || !sample.is_finite() || sample <= 0. {
        return Err("positive finite seconds/sample required".into());
    }
    let solver_dt = solver_dt.unwrap_or(sample);
    if !solver_dt.is_finite() || solver_dt <= 0. {
        return Err("positive finite dt required".into());
    }
    let [nx, ny, nz] = cfg.dims;
    let init = |p: [usize; 3]| {
        let [x, y, z] = p;
        match fixture.as_str() {
            "hydrostatic" => (false, if y < ny / 2 { 1. } else { 0. }),
            "dam" => (false, if x < nx / 3 && y < 2 * ny / 3 { 1. } else { 0. }),
            "vessels" => {
                let solid = x == nx / 2 && y >= 2;
                (
                    solid,
                    if !solid && ((x < nx / 2 && y < 2 * ny / 3) || (x > nx / 2 && y < ny / 4)) {
                        1.
                    } else {
                        0.
                    },
                )
            }
            "falling" => (
                false,
                if x >= nx / 3
                    && x < 2 * nx / 3
                    && y >= ny / 2
                    && y < 3 * ny / 4
                    && z >= nz / 3
                    && z < 2 * nz / 3
                {
                    1.
                } else {
                    0.
                },
            ),
            "obstacle" => {
                let solid = x >= nx / 2 && x < nx / 2 + 2 && y < ny / 3;
                (
                    solid,
                    if !solid && x < nx / 3 && y < ny / 2 {
                        1.
                    } else {
                        0.
                    },
                )
            }
            _ => (false, 0.),
        }
    };
    if !["hydrostatic", "dam", "vessels", "falling", "obstacle"].contains(&fixture.as_str()) {
        return Err("fixture must be hydrostatic, dam, vessels, falling or obstacle".into());
    }
    let mut world = PhysicalFluid::new(cfg, init)?;
    if let Some(path) = &frames {
        std::fs::create_dir_all(path)?;
    }
    let start = Instant::now();
    let mut t = 0.;
    let mut frame = 0;
    let mut total_substeps = 0;
    let mut total_iterations = 0;
    println!("{{\"fixture\":\"{fixture}\",\"dims\":[{nx},{ny},{nz}],\"spacing_m\":{},\"transport\":\"{:?}\",\"boundary\":\"{:?}\",\"advection\":\"{:?}\",\"experimental\":true,\"gate\":\"M1_not_accepted\"}}",cfg.spacing,cfg.transport,cfg.boundary,cfg.advection);
    loop {
        let stats = world.stats();
        let mut front: f64 = 0.;
        let mut occupied_voxels = 0usize;
        let mut columns = vec![0.; nx];
        for x in 0..nx {
            for y in 0..ny {
                for z in 0..nz {
                    columns[x] += world.fraction([x, y, z]).unwrap();
                    if world.fraction([x, y, z]).unwrap() >= 0.5 {
                        occupied_voxels += 1;
                        front = front.max((x + 1) as f64 * cfg.spacing);
                    }
                }
            }
        }
        let volume_target = columns.iter().sum::<f64>() * 0.99;
        let mut prefix = 0.;
        let mut volume99x = 0.;
        for (x, &volume) in columns.iter().enumerate() {
            if prefix + volume >= volume_target && volume > 0. {
                volume99x = (x as f64 + (volume_target - prefix) / volume) * cfg.spacing;
                break;
            }
            prefix += volume;
        }
        println!("{{\"time_s\":{:.9},\"volume_m3\":{:.15e},\"mass_kg\":{:.15e},\"rendered_voxels\":{},\"render_volume_bias_voxels\":{:.9e},\"volume_error_voxels\":{:.9e},\"roundoff_signed_voxels\":{:.9e},\"roundoff_absolute_voxels\":{:.9e},\"transport_residual_voxels\":{:.9e},\"trace_volume_voxels\":{:.9e},\"unpressurized_volume_voxels\":{:.9e},\"centre_m\":[{:.9},{:.9},{:.9}],\"kinetic_j\":{:.9e},\"max_speed_m_s\":{:.9e},\"front_m\":{:.9},\"volume99_x_m\":{:.9},\"max_div_s\":{:.9e},\"full_max_div_s\":{:.9e},\"interface_max_div_s\":{:.9e},\"rms_div_s\":{:.9e},\"wall_leak_m_s\":{:.9e},\"liquid_cells\":{},\"interface_cells\":{}}}",
            t,stats.liquid_volume,stats.liquid_mass,occupied_voxels,occupied_voxels as f64-stats.liquid_volume/cfg.spacing.powi(3),stats.volume_error_voxels,stats.roundoff_adjustment_voxels,stats.roundoff_absolute_voxels,stats.transport_residual_voxels,stats.trace_volume_voxels,stats.unpressurized_volume_voxels,stats.centre_of_volume[0],stats.centre_of_volume[1],stats.centre_of_volume[2],stats.kinetic_energy,stats.max_speed,front,volume99x,
            stats.max_divergence,stats.full_max_divergence,stats.interface_max_divergence,stats.rms_divergence,stats.boundary_max_normal_speed,stats.liquid_cells,stats.interface_cells);
        if let Some(path) = &frames {
            write_frame(&world, path.join(format!("frame-{frame:05}.ppm")))?;
            std::fs::write(
                path.join(format!("state-{frame:05}.fvphys")),
                world.persist_bytes(),
            )?;
        }
        if t >= seconds {
            break;
        }
        let target = (((frame + 1) as f64) * sample).min(seconds);
        let dt = target - t;
        let mut report = PhysicalStep::default();
        let ratio = dt / solver_dt;
        let divisions = (ratio - 32. * f64::EPSILON * ratio.max(1.)).ceil().max(1.) as usize;
        if divisions > 1_000_000 {
            return Err("probe output interval exceeds step budget".into());
        }
        for part in 0..divisions {
            let end = if part + 1 == divisions {
                target
            } else {
                t + dt * (part + 1) as f64 / divisions as f64
            };
            let r = world.step(end - world.stats().time)?;
            report.advanced += r.advanced;
            report.substeps += r.substeps;
            report.retries += r.retries;
            report.pressure_iterations += r.pressure_iterations;
            report.pressure_relative_residual = report
                .pressure_relative_residual
                .max(r.pressure_relative_residual);
            report.projected_max_divergence = report
                .projected_max_divergence
                .max(r.projected_max_divergence);
            report.max_courant = report.max_courant.max(r.max_courant);
        }
        t = world.stats().time;
        frame += 1;
        total_substeps += report.substeps;
        total_iterations += report.pressure_iterations;
        println!("{{\"step_dt_s\":{:.12e},\"substeps\":{},\"retries\":{},\"pcg_iterations\":{},\"pcg_relative_l2\":{:.9e},\"projected_max_div_s\":{:.9e},\"max_courant\":{:.9e}}}",dt,report.substeps,report.retries,report.pressure_iterations,report.pressure_relative_residual,report.projected_max_divergence,report.max_courant);
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!("{{\"cpu_wall_s\":{elapsed:.6},\"simulated_per_wall\":{:.6},\"substeps_total\":{total_substeps},\"pcg_iterations_total\":{total_iterations}}}",t/elapsed);
    let bytes = world.persist_bytes();
    let restart = PhysicalFluid::from_bytes(&bytes)?;
    if bytes != restart.persist_bytes() {
        return Err("checkpoint readback differs".into());
    }
    if let Some(path) = checkpoint {
        std::fs::write(path, bytes)?;
    }
    Ok(())
}
fn write_frame(world: &PhysicalFluid, path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    // Orthographic, whole-cell flat-color diagnostic. The 0.5 threshold does not
    // change numeric volume and is not a promoted liquid occupancy policy.
    let cfg = world.config();
    let [nx, ny, nz] = cfg.dims;
    let scale = 6;
    let mut out = format!("P6\n{} {}\n255\n", nx * scale, ny * scale).into_bytes();
    for py in 0..ny * scale {
        let y = ny - 1 - py / scale;
        for px in 0..nx * scale {
            let x = px / scale;
            let occupied = (0..nz).any(|z| world.fraction([x, y, z]).unwrap() >= 0.5);
            let solid = (0..nz).any(|z| world.is_solid([x, y, z]).unwrap());
            let color = if solid {
                [95u8, 88, 79]
            } else if occupied {
                [25u8, 90, 160]
            } else {
                [235, 238, 242]
            };
            out.extend_from_slice(&color);
        }
    }
    std::fs::write(path, out)?;
    Ok(())
}
