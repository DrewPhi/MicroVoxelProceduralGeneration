use super::*;
#[test]
fn manual_estimate_ceiling_respects_available_ram() {
    let mib = 1048576;
    assert_eq!(build_budget_limit(usize::MAX, 5300 * mib), 5300 * mib);
    assert_eq!(build_budget_limit(10240 * mib, 5300 * mib), 5300 * mib);
    assert_eq!(build_budget_limit(512 * mib, 5300 * mib), 512 * mib);
    assert_eq!(build_budget_limit(usize::MAX, 0), 0);
}
#[test]
fn camera_basis_matches_raster_projection_without_world_origin_cancellation() {
    let mut camera = Camera {
        position: DVec3::new(-1e9, 300., 1e9),
        yaw: 1.2,
        pitch: -0.3,
        fov_y: 60.,
        far: 300.,
    };
    let basis = ray_basis(camera, 16. / 9.);
    camera.position = DVec3::ZERO;
    assert_eq!(basis, ray_basis(camera, 16. / 9.));
    let forward = glam::Vec3::from_array(basis[0][..3].try_into().unwrap());
    let right = glam::Vec3::from_array(basis[1][..3].try_into().unwrap());
    let up = glam::Vec3::from_array(basis[2][..3].try_into().unwrap());
    for x in [-0.9, 0., 0.9] {
        for y in [-0.9, 0., 0.9] {
            let direction = (forward + x * right * basis[1][3] + y * up * basis[2][3]).normalize();
            let projected = camera
                .matrix(DVec3::ZERO, 16. / 9.)
                .project_point3(direction * 10.);
            assert!((projected.x - x).abs() < 1e-6 && (projected.y - y).abs() < 1e-6);
            let depth = basis[3][2] + basis[3][3] / (10. * direction.dot(forward));
            assert!((depth - projected.z).abs() < 3e-7);
        }
    }
}
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
    gpu_matches_canonical_rays(false, false, false, traversal::Options::default());
}

#[test]
#[ignore = "explicit hardware GPU parity: NAADF 32-bit leaf fallback"]
fn naadf_wide_gpu_matches_canonical_rays() {
    gpu_matches_canonical_rays(true, false, false, traversal::Options::default());
}

#[test]
#[ignore = "explicit hardware GPU parity: cold and warm GPU root cache, before and after ring shift"]
fn naadf_gpu_distance_matches_canonical_rays() {
    gpu_matches_canonical_rays(false, true, false, traversal::Options::default());
}

#[test]
#[ignore = "explicit hardware GPU parity: sparse roots and whole uniform pages"]
fn naadf_sparse_roots_match_canonical_rays() {
    gpu_matches_canonical_rays(false, false, true, traversal::Options::default());
}

#[test]
#[ignore = "explicit hardware GPU parity: node cache, max height bounds, and beam with mixed roots"]
fn naadf_traversal_variants_match_canonical_rays() {
    for options in [
        traversal::Options {
            cache: 1,
            ..Default::default()
        },
        traversal::Options {
            cache: 2,
            ..Default::default()
        },
        traversal::Options {
            height_tile: Some(1),
            ..Default::default()
        },
        traversal::Options {
            height_tile: Some(4),
            ..Default::default()
        },
        traversal::Options {
            height_tile: Some(8),
            ..Default::default()
        },
        traversal::Options {
            height_tile: Some(4),
            height_mips: true,
            ..Default::default()
        },
        traversal::Options {
            height_tile: Some(4),
            height_gate: true,
            ..Default::default()
        },
        traversal::Options {
            height_tile: Some(4),
            height_mips: true,
            height_gate: true,
            ..Default::default()
        },
        traversal::Options {
            beam: true,
            ..Default::default()
        },
    ] {
        gpu_matches_canonical_rays(false, true, false, options);
        gpu_matches_canonical_rays(false, false, true, options);
    }
}

fn gpu_matches_canonical_rays(
    force_wide_pigment: bool,
    gpu_distance: bool,
    sparse_roots: bool,
    options: traversal::Options,
) {
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
        let mut gpu = Gpu::new(
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
        if gpu_distance {
            gpu.visual_settings = Some(crate::settings::VisualSettings::default());
        }
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
        let cpu_root_fallback = gpu_distance && root_distance::cpu_fallback();
        let mut snapshot = data::build_with_root_mode(
            &world,
            eye.to_array(),
            if sparse_roots {
                8
            } else if options.height_mips {
                // Ten roots across produce a partial tile and a padded mip base.
                5
            } else {
                4
            },
            128 * 1048576,
            &AtomicU32::new(0),
            gpu_distance && !cpu_root_fallback,
            || false,
        )
        .unwrap();
        snapshot.gpu_distance = gpu_distance;
        snapshot.cpu_root_fallback = cpu_root_fallback;
        if force_wide_pigment {
            let word = *snapshot
                .voxels
                .iter()
                .find(|&&v| v & (1 << 30) != 0)
                .unwrap();
            let pigment = snapshot.palette[(word ^ (1 << 30)) as usize];
            snapshot.palette.resize(32769, pigment);
            for v in &mut snapshot.voxels {
                if *v == word {
                    *v = (1 << 30) | 32768;
                }
            }
        }
        let mut prepared = leaf::Prepared::new(snapshot, false);
        if sparse_roots {
            prepared.make_sparse_roots(|| false);
            assert!(prepared.sparse_root_dims.is_some());
        }
        assert_eq!(prepared.encoding.is_compact(), !force_wide_pigment);
        let mut pending = Pending::with_traversal(&gpu, prepared, options).unwrap();
        while !pending.advance(&gpu.queue) {}
        gpu.queue.submit(std::iter::empty());
        gpu.device.poll(wgpu::Maintain::Wait);
        let mut volume = pending.volume;
        let mut state = gpu_distance.then(|| {
            stream::State::new_with_encoding(
                pending.snapshot,
                volume.capacities,
                volume.leaf_encoding,
            )
        });
        let shadow_source = crate::adaptive_gpu::AdaptiveGpu::new(&gpu.device, gpu.scene_format, 1);
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
        let initial_camera = camera;
        let mut check_rays = |volume: &Volume| {
            let bounds = volume.bounds;
            let mut tested = 0;
            for (yaw, pitch, shadows) in [(0., -0.65), (1.3, -0.4), (-2.5, -0.8), (3., 0.3)]
                .into_iter()
                .flat_map(|(y, p)| [(y, p, false), (y, p, true)])
            {
                camera.yaw = yaw;
                camera.pitch = pitch;
                let matrix = camera.matrix(DVec3::from_array(bounds[0]), 1.);
                let inverse = matrix.inverse();
                let basis = ray_basis(camera, 1.);
                let local = (eye - DVec3::from_array(bounds[0])).as_vec3();
                gpu.queue.write_buffer(
                    &volume.uniform,
                    0,
                    bytemuck::bytes_of(&Uniform {
                        matrix: matrix.to_cols_array_2d(),
                        inverse: inverse.to_cols_array_2d(),
                        eye: local.extend(0.).to_array(),
                        extent: [volume.n * 16, volume.n * 16, volume.n * 16, volume.n],
                        viewport: [64., 64., 1., if shadows { 2. } else { 1. }],
                        sun: [-0.5, 0.8, -0.3, 0.],
                        sky: [0.; 4],
                        far_matrix: [[0.; 4]; 4],
                        shadow: [0.; 4],
                        forward: basis[0],
                        right: basis[1],
                        up: basis[2],
                        projection: basis[3],
                        ring: [
                            volume.ring[0],
                            volume.ring[1],
                            volume.ring[2],
                            volume.traversal_flags(),
                        ],
                        cache: volume
                            .root_distance
                            .as_ref()
                            .map_or([0; 4], |cache| cache.uniforms(1.)),
                    }),
                );
                let mut encoder = gpu.device.create_command_encoder(&Default::default());
                volume.encode_traversal(&gpu, &mut encoder, [64; 2], true);
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
                    volume.beam.bind(&mut pass);
                    pass.set_bind_group(1, shadow_source.far_shadow_bind(), &[]);
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
                        let direction = (Vec3::from_array(basis[0][..3].try_into().unwrap())
                            + ndc.x
                                * Vec3::from_array(basis[1][..3].try_into().unwrap())
                                * basis[1][3]
                            + ndc.y
                                * Vec3::from_array(basis[2][..3].try_into().unwrap())
                                * basis[2][3])
                            .normalize();
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
                        let hit =
                            frontier_world::raycast(&world, eye.to_array(), d.to_array(), max)
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
                            if shadows {
                                let sun = Vec3::new(-0.5, 0.8, -0.3).normalize().as_dvec3();
                                let origin = eye
                                    + d * hit.distance
                                    + DVec3::from_array(hit.normal.map(|v| v as f64)) * 0.001;
                                let limit = (0..3)
                                    .map(|a| {
                                        if sun[a] > 0. {
                                            (bounds[1][a] - origin[a]) / sun[a]
                                        } else {
                                            (bounds[0][a] - origin[a]) / sun[a]
                                        }
                                    })
                                    .fold(f64::INFINITY, f64::min);
                                let blocked = frontier_world::raycast(
                                    &world,
                                    origin.to_array(),
                                    sun.to_array(),
                                    (limit - 1e-7).max(0.),
                                )
                                .is_some();
                                assert_eq!(
                                    results[0][i * 4] != 0,
                                    blocked,
                                    "shadow {x},{y}, yaw{yaw}"
                                );
                            } else {
                                assert_eq!(
                                    results[0][i * 4],
                                    data::id(hit.material) as u8,
                                    "material {x},{y}"
                                );
                            }

                            let point = local + direction * hit.distance as f32;
                            let clip = matrix * point.extend(1.);
                            assert!((depths[i] - clip.z / clip.w).abs() < 2e-6, "depth {x},{y}");
                            tested += 1;
                        }
                    }
                }
            }
            eprintln!("NAADF GPU oracle: {tested} covered primary/shadow samples across four headings; 32768 coverage decisions checked");
            assert!(tested > 1000);
        };
        check_rays(&volume);
        if let Some(state) = &mut state {
            let warm = |volume: &mut Volume| {
                let cache = volume
                    .root_distance
                    .as_mut()
                    .expect("root cache must be allocated");
                assert_eq!(cache.progress().0, 0);
                cache.advance(
                    &gpu.device,
                    &gpu.queue,
                    volume.n,
                    volume.ring,
                    initial_camera,
                );
                assert_eq!(cache.progress().0, 0);
                cache.advance(
                    &gpu.device,
                    &gpu.queue,
                    volume.n,
                    volume.ring,
                    initial_camera,
                );
                gpu.device.poll(wgpu::Maintain::Wait);
                assert_eq!(cache.progress(), (1, 1));
            };
            warm(&mut volume);
            check_rays(&volume);
            let delta = [-2, 0, 1];
            let expected_ring = std::array::from_fn(|axis| {
                (i64::from(volume.ring[axis]) + delta[axis]).rem_euclid(i64::from(volume.n)) as u32
            });
            let next = stream::Region {
                lo: std::array::from_fn(|a| state.region.lo[a] + delta[a]),
                hi: std::array::from_fn(|a| state.region.hi[a] + delta[a]),
            };
            let change = state
                .prepare(&world, next, 128 * 1048576, &|| false)
                .unwrap();
            let mut shift = upload::PendingShift::new(&gpu, change);
            while !shift.advance(&gpu, &volume).0 {}
            check_rays(&volume);
            let (change, _) = shift.publish(&gpu, &mut volume);
            state.finish(change, true);
            assert_eq!(volume.ring, expected_ring);
            assert_eq!(volume.root_distance.as_ref().unwrap().progress().0, 0);
            check_rays(&volume);
            warm(&mut volume);
            check_rays(&volume);
        }
        eprintln!("NAADF GPU memory: {}-bit leaves, {} leaf bytes, {} initial payload bytes ({} with wide leaves)",
            volume.leaf_encoding.bits(), volume.leaf_bytes, volume.bytes, volume.wide_bytes);
    });
}
