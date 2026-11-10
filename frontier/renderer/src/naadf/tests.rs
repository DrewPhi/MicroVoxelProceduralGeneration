use super::*;
#[test]
fn shader_is_valid() {
    let m = naga::front::wgsl::parse_str(SHADER).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&m)
    .unwrap();
}

#[test]
#[ignore = "explicit hardware GPU parity: NAADF first-hit cells versus independent canonical DDA"]
fn naadf_gpu_matches_canonical_rays() {
    use glam::{Vec3, Vec4};
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let mesh = crate::mesh::Mesh {
            vertices: vec![],
            indices: vec![],
            origin: frontier_world::Cell::default(),
            exposed_native_faces: 0,
        };
        let gpu = Gpu::new(
            &instance,
            None,
            Some(wgpu::TextureFormat::Rgba8Unorm),
            &mesh,
            1,
            false,
            false,
        )
        .await
        .unwrap();
        let world = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::Legacy,
                amplitude: 2.,
                horizontal_scale: 30.,
                octaves: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let height = world.height_m(0., 0.);
        let eye = DVec3::new(0., height + 1.7, 0.);
        let mut camera = Camera {
            position: eye,
            far: 100.,
            fov_y: 60.,
            yaw: 0.,
            pitch: 0.,
        };
        let snapshot = data::build(
            &world,
            eye.to_array(),
            4,
            128 * 1048576,
            &AtomicU32::new(0),
            || false,
        )
        .unwrap();
        let bounds = snapshot.bounds();
        let mut pending = Pending::new(&gpu, snapshot).unwrap();
        while !pending.advance(&gpu.queue) {}
        gpu.queue.submit(std::iter::empty());
        gpu.device.poll(wgpu::Maintain::Wait);
        let volume = pending.volume;
        let texture = |format| {
            gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("NAADF parity"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let color = texture(wgpu::TextureFormat::Rgba8Unorm);
        let depth = texture(wgpu::TextureFormat::Depth32Float);
        let cv = color.create_view(&Default::default());
        let dv = depth.create_view(&Default::default());
        let buffers: Vec<_> = (0..2)
            .map(|_| {
                gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("NAADF parity readback"),
                    size: 64 * 64 * 4,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let mut tested = 0;
        for (yaw, pitch) in [(0., -0.65), (1.3, -0.4), (-2.5, -0.8), (3.0, 0.3)] {
            camera.yaw = yaw;
            camera.pitch = pitch;
            let matrix = camera.matrix(DVec3::from_array(bounds[0]), 1.);
            let inverse = matrix.inverse();
            let local = (eye - DVec3::from_array(bounds[0])).as_vec3();
            gpu.queue.write_buffer(
                &volume.uniform,
                0,
                bytemuck::bytes_of(&Uniform {
                    matrix: matrix.to_cols_array_2d(),
                    inverse: inverse.to_cols_array_2d(),
                    eye: local.extend(0.).to_array(),
                    extent: [volume.n * 16, volume.n * 16, volume.n * 16, volume.n],
                    viewport: [64., 64., 1., 1.],
                    sun: [0.; 4],
                    sky: [0.; 4],
                }),
            );
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("NAADF oracle"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &cv,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &dv,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&volume.pipeline);
                pass.set_bind_group(0, &volume.bind, &[]);
                pass.draw(0..3, 0..1);
            }
            for (tex, buffer) in [(&color, &buffers[0]), (&depth, &buffers[1])] {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(64),
                        },
                    },
                    wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    },
                );
            }
            gpu.queue.submit(Some(encoder.finish()));
            let mut results = Vec::new();
            for buffer in &buffers {
                let (tx, rx) = mpsc::channel();
                buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    tx.send(r).unwrap();
                });
                gpu.device.poll(wgpu::Maintain::Wait);
                rx.recv().unwrap().unwrap();
                results.push(buffer.slice(..).get_mapped_range().to_vec());
                buffer.unmap();
            }
            let depths: Vec<f32> = results[1]
                .chunks_exact(4)
                .map(|v| f32::from_ne_bytes(v.try_into().unwrap()))
                .collect();
            for y in 0..64 {
                for x in 0..64 {
                    let ndc = Vec4::new(
                        (x as f32 + 0.5) / 32. - 1.,
                        1. - (y as f32 + 0.5) / 32.,
                        0.5,
                        1.,
                    );
                    let p = inverse * ndc;
                    let direction = (p.truncate() / p.w - local).normalize();
                    let d = direction.as_dvec3();
                    let max = (0..3)
                        .filter_map(|a| {
                            if d[a] > 0. {
                                Some((bounds[1][a] - eye[a]) / d[a])
                            } else if d[a] < 0. {
                                Some((bounds[0][a] - eye[a]) / d[a])
                            } else {
                                None
                            }
                        })
                        .fold(f64::INFINITY, f64::min);
                    let hit = frontier_world::raycast(&world, eye.to_array(), d.to_array(), max)
                        .filter(|h| {
                            let p = h.cell.center_m();
                            (0..3).all(|a| p[a] >= bounds[0][a] && p[a] < bounds[1][a])
                        });
                    let i = y * 64 + x;
                    assert_eq!(
                        results[0][i * 4 + 3] != 0,
                        hit.is_some(),
                        "coverage pixel {x},{y}, yaw{yaw},pitch{pitch}"
                    );
                    if let Some(hit) = hit {
                        assert_eq!(
                            results[0][i * 4],
                            data::id(hit.material) as u8,
                            "material {x},{y}"
                        );
                        let point = local + direction * hit.distance as f32;
                        let clip = matrix * point.extend(1.);
                        assert!((depths[i] - clip.z / clip.w).abs() < 2e-6, "depth {x},{y}");
                        tested += 1;
                    }
                }
            }
        }
        eprintln!("NAADF exact GPU oracle: {tested} covered rays across4headings; all16384coverage decisions checked");
        assert!(tested > 1000);
        let _ = Vec3::ZERO;
    });
}
