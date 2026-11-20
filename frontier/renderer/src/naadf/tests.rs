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
        roll: 0.,
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
    let m = naga::front::wgsl::parse_str(&crate::voxel_shader(SHADER)).unwrap();
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
#[ignore = "explicit GPU parity: exact integer decode, mixed-root cache and combined variants"]
fn naadf_exact_fastpaths_match_canonical_rays() {
    for options in [
        traversal::Options {
            integer_fastpath: true,
            ..Default::default()
        },
        traversal::Options {
            cache: 3,
            ..Default::default()
        },
        traversal::Options {
            cache: 3,
            integer_fastpath: true,
            ..Default::default()
        },
    ] {
        gpu_matches_canonical_rays(false, false, false, options);
        gpu_matches_canonical_rays(true, false, false, options);
        gpu_matches_canonical_rays_with_storage(false, false, false, options, true);
    }
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
            local_exit_repair: true,
            diagnostics: true,
            ..Default::default()
        },
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
            height_mips: true,
            height_gate: true,
            height_probe: Some(2),
            diagnostics: true,
            ..Default::default()
        },
        traversal::Options {
            beam: true,
            ..Default::default()
        },
        traversal::Options {
            height_tile: Some(4),
            height_mips: true,
            height_gate: true,
            beam: true,
            diagnostics: true,
            ..Default::default()
        },
    ] {
        gpu_matches_canonical_rays(false, true, false, options);
        gpu_matches_canonical_rays(false, false, true, options);
    }
}

#[test]
#[ignore = "explicit GPU parity: partial residency, shell growth, arena copies, and wrapped following"]
fn naadf_reserved_growth_gpu_matches_canonical_rays() {
    gpu_matches_canonical_rays_with_storage(
        false,
        false,
        false,
        traversal::Options::default(),
        true,
    );
}

#[test]
#[ignore = "hardware parity for occupied-height tiles and beam prefixes during shell growth"]
fn naadf_height_beams_follow_partial_residency() {
    gpu_matches_canonical_rays_with_storage(
        false,
        false,
        false,
        traversal::Options {
            height_tile: Some(4),
            height_gate: true,
            beam: true,
            ..Default::default()
        },
        true,
    );
}

fn gpu_matches_canonical_rays(
    force_wide_pigment: bool,
    gpu_distance: bool,
    sparse_roots: bool,
    options: traversal::Options,
) {
    gpu_matches_canonical_rays_with_storage(
        force_wide_pigment,
        gpu_distance,
        sparse_roots,
        options,
        false,
    );
}
fn gpu_matches_canonical_rays_with_storage(
    force_wide_pigment: bool,
    gpu_distance: bool,
    sparse_roots: bool,
    options: traversal::Options,
    reserved: bool,
) {
    gpu_matches_canonical_rays_for_source(
        force_wide_pigment,
        gpu_distance,
        sparse_roots,
        options,
        reserved,
        false,
    );
}

#[test]
#[ignore = "explicit hardware GPU parity: SeededWorld opaque water, untouched bed, air and material IDs, compact and wide leaves"]
fn naadf_seeded_world_water_bed_air_matches_canonical_rays() {
    for wide in [false, true] {
        gpu_matches_canonical_rays_for_source(
            wide,
            false,
            false,
            traversal::Options::default(),
            false,
            true,
        );
    }
}

fn gpu_matches_canonical_rays_for_source(
    force_wide_pigment: bool,
    gpu_distance: bool,
    sparse_roots: bool,
    options: traversal::Options,
    reserved: bool,
    seeded: bool,
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
        let mut world = World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: if seeded {
                    frontier_world::FbmRecipe::SeededWorld
                } else {
                    frontier_world::FbmRecipe::Legacy
                },
                amplitude: if seeded { 1000. } else { 2. },
                horizontal_scale: if seeded { 1800. } else { 30. },
                octaves: if seeded { 8 } else { 2 },
                ..Default::default()
            },
        )
        .unwrap();
        let mut inspection = None;
        let eye = if seeded {
            use frontier_world::{Cell, Material};
            // Find real source water, rather than constructing a water fixture
            // from edits. The short river depth fits the bounded GPU snapshot.
            let terrain = world.fbm_terrain().unwrap();
            let (x, z, column) = (-3..=3)
                .flat_map(|z| (-3..=3).map(move |x| (x * 38400, z * 38400)))
                .find_map(|(x, z)| {
                    let c = terrain.column_cells(x, z);
                    (32..=80)
                        .contains(&(c.visible_top - c.ground_top))
                        .then_some((x, z, c))
                })
                .expect("seed7 has an active shallow river node");
            // Opaque water hides its bed. A narrow air inspection shaft exposes
            // the UNEDITED canonical bed while preserving surrounding source
            // water. Both surfaces must produce matching GPU depth/material.
            for dz in -6..6 {
                for dx in -6..6 {
                    let c = world.column(x + dx, z + dz);
                    let ground = (c.ground_height_m() / voxel_size()) as i64;
                    let top = (c.height_m() / voxel_size()) as i64;
                    for y in ground..top {
                        world.set(
                            Cell {
                                x: x + dx,
                                y,
                                z: z + dz,
                            },
                            Material::Air,
                        );
                    }
                }
            }
            // Small visible marker caps verify the appended land material IDs
            // alongside naturally sourced water and bed pigments.
            for (dx, material) in [(-12, Material::Sand), (12, Material::Snow)] {
                for zz in -2..2 {
                    for xx in -2..2 {
                        world.set(
                            Cell {
                                x: x + dx + xx,
                                y: column.visible_top,
                                z: z + zz,
                            },
                            material,
                        );
                    }
                }
            }
            inspection = Some([x, z]);
            DVec3::new(
                x as f64 * voxel_size(),
                column.visible_top as f64 * voxel_size() + 1.7,
                z as f64 * voxel_size(),
            )
        } else {
            let height = world.height_m(0., 0.);
            DVec3::new(0., height + 1.7, 0.)
        };
        let mut camera = Camera {
            roll: 0.,
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
            if seeded {
                8
            } else if sparse_roots {
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
        let mut pending = if reserved {
            Pending::with_storage(&gpu, prepared, options, 24)
        } else {
            Pending::with_traversal(&gpu, prepared, options)
        }
        .unwrap();
        while !pending.advance(&gpu.queue) {}
        gpu.queue.submit(std::iter::empty());
        gpu.device.poll(wgpu::Maintain::Wait);
        let mut volume = pending.volume;
        let mut state = (gpu_distance || reserved).then(|| {
            stream::State::with_storage(
                pending.snapshot,
                if reserved {
                    volume.arena_limits(&gpu)
                } else {
                    volume.capacities
                },
                volume.leaf_encoding,
                volume.n as usize,
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
            let mut seen_water = 0;
            let mut seen_bed = 0;
            let mut seen_air = 0;
            let mut seen_materials = [0_usize; 10];
            let views = if seeded {
                vec![(0., -1.55, false), (3., 0.3, false)]
            } else {
                [(0., -0.65), (1.3, -0.4), (-2.5, -0.8), (3., 0.3)]
                    .into_iter()
                    .flat_map(|(y, p)| [(y, p, false), (y, p, true)])
                    .collect()
            };
            for (view_index, (yaw, pitch, shadows)) in views.into_iter().enumerate() {
                // Check post-edit conservative root traversal against native DDA.
                let edited_flag = if seeded && view_index % 2 == 0 { 8 } else { 0 };
                camera.yaw = yaw;
                camera.pitch = pitch;
                let matrix = camera.matrix(DVec3::from_array(bounds[0]), 1.);
                let inverse = matrix.inverse();
                let basis = ray_basis(camera, 1.);
                let local = (eye - DVec3::from_array(bounds[0])).as_vec3();
                volume.beam.set_view(local, basis);
                gpu.queue.write_buffer(
                    &volume.uniform,
                    0,
                    bytemuck::bytes_of(&Uniform {
                        matrix: matrix.to_cols_array_2d(),
                        inverse: inverse.to_cols_array_2d(),
                        eye: local.extend(0.).to_array(),
                        extent: [
                            ((bounds[1][0] - bounds[0][0]) * 16.) as u32,
                            ((bounds[1][1] - bounds[0][1]) * 16.) as u32,
                            ((bounds[1][2] - bounds[0][2]) * 16.) as u32,
                            volume.n,
                        ],
                        viewport: [64., 64., 1., if shadows { 2. } else { 1. }],
                        sun: [-0.5, 0.8, -0.3, 0.],
                        sky: [0.; 4],
                        far_matrix: [[0.; 4]; 4],
                        shadow: [0.; 4],
                        forward: basis[0],
                        right: basis[1],
                        up: basis[2],
                        projection: basis[3],
                        edit_min: [i32::MIN; 4],
                        edit_max: [i32::MAX; 4],
                        water: [0.; 4],
                        ring: [
                            volume.ring[0],
                            volume.ring[1],
                            volume.ring[2],
                            volume.traversal_flags() | edited_flag,
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
                    pass.set_pipeline(volume.pipeline_for_view(false));
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
                if let Some(stages) = &gpu.naadf_gpu_stages {
                    stages.submitted_render();
                }
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
                if let Some(stages) = &gpu.naadf_gpu_stages {
                    stages.poll(&gpu.device);
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
                        if seeded && hit.is_none() {
                            seen_air += 1;
                        }
                        if let Some(hit) = hit {
                            if seeded && !shadows {
                                seen_materials[data::id(hit.material) as usize] += 1;
                                seen_water +=
                                    usize::from(hit.material == frontier_world::Material::Water);
                                if let Some([sx, sz]) = inspection {
                                    if (sx - 6..sx + 6).contains(&hit.cell.x)
                                        && (sz - 6..sz + 6).contains(&hit.cell.z)
                                    {
                                        let ground =
                                            (world.column(hit.cell.x, hit.cell.z).ground_height_m()
                                                / voxel_size())
                                                as i64;
                                        seen_bed += usize::from(hit.cell.y == ground - 1);
                                    }
                                }
                            }
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

                            // Compare canonical hit distance using the stable camera-depth
                            // equation. Multiplying an f32 translated VP by a larger
                            // local point loses precision as the visible origin grows.
                            let cosine = direction
                                .dot(Vec3::from_array(basis[0][..3].try_into().unwrap()))
                                as f64;
                            let expected_depth =
                                basis[3][2] as f64 + basis[3][3] as f64 / (hit.distance * cosine);
                            assert!((depths[i] as f64 - expected_depth).abs() < 2e-6,
                                "depth {x},{y}: GPU {}, CPU {}, distance {}, cell {:?}, yaw {yaw}, pitch {pitch}, shadows {shadows}, bounds {bounds:?}, ring {:?}",
                                depths[i], expected_depth, hit.distance, hit.cell, volume.ring);
                            tested += 1;
                        }
                    }
                }
            }
            eprintln!("NAADF GPU oracle: {tested} covered primary/shadow samples across four headings; 32768 coverage decisions checked");
            assert!(tested > 1000);
            if seeded {
                assert!(
                    seen_water > 0,
                    "GPU oracle must exercise actual source water"
                );
                assert!(
                    seen_bed > 0,
                    "inspection shaft must expose untouched source bed"
                );
                assert!(seen_air > 0, "upward rays must exercise air misses");
                assert!(
                    seen_materials[data::id(frontier_world::Material::Sand) as usize] > 0,
                    "Sand marker must be visible"
                );
                assert!(
                    seen_materials[data::id(frontier_world::Material::Snow) as usize] > 0,
                    "Snow marker must be visible"
                );
                eprintln!("SeededWorld oracle witnesses: water={seen_water}, source_bed={seen_bed}, air={seen_air}");
            }
            if options.diagnostics {
                let words = volume
                    .diagnostics
                    .read_words(&gpu.device, &gpu.queue)
                    .unwrap()
                    .unwrap();
                for lane in words[..64].chunks_exact(32) {
                    assert!(lane[0] > 0, "primary and shadow rays must be counted");
                    assert_eq!(lane[0], lane[23] + lane[24]);
                    assert_eq!(lane[7], lane[9] + lane[11]);
                    if options.height_probe.is_some() {
                        assert_eq!(
                            lane[8], lane[7],
                            "single probe must issue one height fetch per query"
                        );
                    }
                    assert!(lane[10] <= lane[9]);
                    assert!(lane[4] > 0 && lane[5] > 0 && lane[6] > 0);
                }
                if options.beam {
                    assert!(words[64] > 0);
                    assert_eq!(words[64], words[65] + words[66]);
                    assert!(words[73] <= words[74]);
                } else {
                    assert_eq!(words[64], 0);
                }
            }
        };
        check_rays(&volume);
        if reserved {
            let state = state.as_mut().unwrap();
            for step in 0..3 {
                let next = if step < 2 {
                    startup::next_region(state.region, 12, eye.to_array())
                } else {
                    stream::Region {
                        lo: state.region.lo.map(|v| v - 3),
                        hi: state.region.hi.map(|v| v - 3),
                    }
                };
                let mut change = state
                    .prepare(&world, next, 128 * 1048576, &|| false)
                    .unwrap();
                if step == 0 {
                    // Force a backing-buffer replacement while all existing
                    // primary/shadow hits must retain their exact pigments.
                    change.writes.push(stream::Write {
                        kind: 2,
                        word: volume.capacities[2] / 4,
                        data: vec![0; 4],
                    });
                }
                volume.ensure_arenas(&gpu, &change, 128 * 1048576).unwrap();
                let mut pending = upload::PendingShift::new(&gpu, change);
                while !pending.advance(&gpu, &volume).0 {}
                // Unpublished uploads cannot alter retained camera visibility.
                check_rays(&volume);
                let (change, _) = pending.publish(&gpu, &mut volume);
                state.finish(change, true);
                check_rays(&volume);
            }
            assert!(volume.arena_resizes > 0 && volume.arena_copy_bytes > 0);
            assert_eq!(volume.resident_radius(), 12.);
        }
        if let Some(state) = state.as_mut().filter(|_| gpu_distance) {
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
        if let Some(stages) = &gpu.naadf_gpu_stages {
            use super::gpu_stages::Stage;
            stages.flush(&gpu.device);
            stages.assert_settled();
            if options.height_enabled() || options.beam {
                assert!(stages.samples(Stage::HeightBase) > 0);
            }
            if options.height_mips {
                assert!(stages.samples(Stage::HeightReduce) > 0);
            }
            if options.beam {
                assert!(stages.samples(Stage::Beam) > 0);
            }
            if gpu_distance {
                assert!(stages.samples(Stage::RootRepair) > 0);
            }
            if gpu_distance && root_distance::Mode::requested() == root_distance::Mode::Feedback {
                assert!(stages.samples(Stage::Feedback) > 0);
            }
            if gpu_distance && upload::scatter_allowance().is_some() {
                assert!(stages.samples(Stage::Scatter) > 0);
            }
            eprintln!("Actual NAADF GPU stage hooks: {}", stages.json());
        }
        eprintln!("NAADF GPU memory: {}-bit leaves, {} leaf bytes, {} initial payload bytes ({} with wide leaves)",
            volume.leaf_encoding.bits(), volume.leaf_bytes, volume.bytes, volume.wide_bytes);
    });
}

#[test]
#[ignore = "explicit stage timestamp hooks with FRONTIER_NAADF_GPU_STAGES=1; optionally SCATTER_UPLOAD=1 and feedback schedule"]
fn naadf_gpu_stage_timing_hooks_match_canonical_rays() {
    assert!(gpu_stages::requested(), "set FRONTIER_NAADF_GPU_STAGES=1");
    gpu_matches_canonical_rays(
        false,
        true,
        false,
        traversal::Options {
            height_tile: Some(4),
            height_mips: true,
            beam: true,
            ..Default::default()
        },
    );
}
