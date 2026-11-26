//! Matched capture of the playable renderer through the normal scene draw path.
use crate::{mesh, settings::VisualSettings, walking, Camera, Gpu, Result};
use frontier_runtime::routes::Route;
use frontier_world::{terrain::alpine_path_center_m, Cell, World};
use glam::DVec3;
use std::{io::Write, path::Path};

pub(super) fn capture(
    world: &World,
    mut camera: Camera,
    visual: &VisualSettings,
    width: u32,
    height: u32,
    frames: u64,
    step: f64,
    speed: f64,
    route: Route,
    directory: &Path,
) -> Result<()> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let empty = mesh::Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
        origin: Cell { x: 0, y: 0, z: 0 },
        exposed_native_faces: 0,
    };
    let mut gpu = pollster::block_on(Gpu::new(
        &instance,
        None,
        None,
        &empty,
        visual.msaa,
        visual.voxel_filter,
        false,
    ))?;
    let mut walker = walking::WalkingCamera::default();
    let fly = matches!(route, Route::Fly | Route::BankReturn);
    let trail = !fly && !route.grounded();
    if route.grounded() {
        camera.position.y =
            world.height_m(camera.position.x, camera.position.z) + walking::EYE_HEIGHT;
    } else if trail {
        camera.position.x = alpine_path_center_m(camera.position.z);
        camera.position.y =
            world.height_m(camera.position.x, camera.position.z) + walking::EYE_HEIGHT;
        update_heading(&mut camera);
    }
    let origin = camera;
    gpu.configure_visuals(world, visual);
    gpu.init_rolling(world, camera)?;
    gpu.settle_naadf(world, camera)?;
    gpu.settle_vegetation(world, camera, width, height)?;
    let frame_dir = directory.join("frames");
    std::fs::create_dir_all(&frame_dir)?;
    let mut poses = std::io::BufWriter::new(std::fs::File::create(directory.join("poses.csv"))?);
    writeln!(poses, "frame,x,y,z,yaw,pitch,roll,naadf_drawable")?;
    let mut probes = if fly {
        std::fs::create_dir_all(directory.join("ownership"))?;
        let mut out = std::io::BufWriter::new(std::fs::File::create(directory.join("probe.csv"))?);
        writeln!(out, "frame,rendered_forward_m,canonical_forward_m,naadf_lo_x,naadf_lo_y,naadf_lo_z,naadf_hi_x,naadf_hi_y,naadf_hi_z")?;
        Some(out)
    } else {
        None
    };
    for frame in 0..frames {
        let t = frame as f64 * step;
        if fly || route.grounded() {
            let sample = |time| {
                route.pose_at(
                    time,
                    origin.position.to_array(),
                    f64::from(origin.yaw),
                    f64::from(origin.pitch),
                    speed,
                )
            };
            let pose = sample(t);
            if fly {
                camera.position = DVec3::from_array(pose.position);
            } else if frame > 0 {
                let previous = sample(t - step);
                camera.position = DVec3::from_array(walker.advance_presented(
                    world,
                    camera.position.to_array(),
                    [
                        (pose.position[0] - previous.position[0]) / step,
                        (pose.position[2] - previous.position[2]) / step,
                    ],
                    false,
                    step,
                    visual.step_smoothing,
                ));
            }
            camera.yaw = pose.yaw as f32;
            camera.pitch = pose.pitch as f32;
            camera.roll = pose.roll as f32;
        } else if frame > 0 {
            let z = camera.position.z + speed * step;
            let target = alpine_path_center_m(z);
            camera.position = DVec3::from_array(walker.advance_presented(
                world,
                camera.position.to_array(),
                [(target - camera.position.x) / step, speed],
                false,
                step,
                visual.step_smoothing,
            ));
            update_heading(&mut camera);
        }
        gpu.update_rolling(world, camera)?;
        gpu.update_vegetation(world, camera, width, height)?;
        gpu.naadf_raster_restored = gpu.vegetation_stats.naadf_retire_unresolved == 0;
        gpu.update_naadf(world, camera);
        let naadf_drawable = gpu
            .naadf
            .as_ref()
            .is_some_and(|n| n.bounds_for(camera).is_some());
        let [x, y, z] = camera.position.to_array();
        writeln!(
            poses,
            "{frame},{x},{y},{z},{},{},{},{naadf_drawable}",
            camera.yaw, camera.pitch, camera.roll
        )?;
        gpu.capture_frame(
            world,
            &camera,
            &frame_dir.join(format!("{frame:04}.png")),
            width,
            height,
            false,
        )?;
        if let Some(probes) = probes.as_mut() {
            gpu.rolling_probe_depth.set(Some(f32::NAN));
            gpu.capture_frame(
                world,
                &camera,
                &directory.join("ownership").join(format!("{frame:04}.png")),
                width,
                height,
                false,
            )?;
            let ndc = gpu.rolling_probe_depth.replace(None).unwrap();
            let clip = f64::from(camera.far);
            let depth = 0.025 * clip / (clip - f64::from(ndc) * (clip - 0.025));
            let ray = centre_ray(camera, width, height);
            let reference = frontier_world::raycast_solid(
                world,
                camera.position.to_array(),
                ray.to_array(),
                clip * ray.length(),
            )
            .map_or(f64::NAN, |hit| hit.distance / ray.length());
            let [[x0, y0, z0], [x1, y1, z1]] = gpu
                .naadf
                .as_ref()
                .and_then(|n| n.bounds_for(camera))
                .unwrap_or([[f64::NAN; 3]; 2]);
            writeln!(
                probes,
                "{frame},{depth},{reference},{x0},{y0},{z0},{x1},{y1},{z1}"
            )?;
        }
    }
    if let Some(probes) = probes.as_mut() {
        probes.flush()?;
    }
    poses.flush()?;
    gpu.capture(world, &camera, &directory.join("final.png"), width, height)?;
    Ok(())
}

fn update_heading(camera: &mut Camera) {
    let z = camera.position.z + 28.;
    camera.yaw = ((alpine_path_center_m(z) - camera.position.x) / 28.).atan() as f32;
    camera.pitch = -0.045;
}

fn centre_ray(camera: Camera, width: u32, height: u32) -> DVec3 {
    let forward = camera.direction();
    let right = forward.cross(camera.view_up()).normalize();
    let up = right.cross(forward).normalize();
    let tan = (camera.fov_y.to_radians() * 0.5).tan();
    let uv = |n: u32| (n / 2) as f32 * 2. / n as f32 + 1. / n as f32 - 1.;
    (forward + right * (tan * width as f32 / height as f32 * uv(width)) - up * (tan * uv(height)))
        .as_dvec3()
}
