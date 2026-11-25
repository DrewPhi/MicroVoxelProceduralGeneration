//! Ignored hardware test: freeze canonical geometry before rendering every backend.
//! Run explicitly with `cargo test -p frontier-renderer frozen_native_gpu_parity --lib -- --ignored --nocapture`.
use super::*;
use crate::vegetation::{VegetationBatch, VegetationMesh};
use frontier_world::vegetation::{generate_edelweiss, generate_grass, generate_rowan, PlantModel};
use glam::{Mat4, Vec3};
use std::sync::{mpsc, Arc};

const SIZE: u32 = 256;
const ORIGIN: [i64; 3] = [-8192, 0, -16384];

fn frozen_batch(model: PlantModel, species: u8, seed: u64) -> VegetationBatch {
    // Freeze the real seeded canonical plants at their native 6.25cm lattice.
    // The reference extractor avoids any scheduler/LOD history or async timing.
    let mesh: VegetationMesh = crate::vegetation::mesh_cells_scaled_reference(
        model
            .voxels
            .iter()
            .map(|v| (v.cell, v.material as u8, v.color)),
        1,
        1,
        || false,
    );
    assert!(!mesh.indices.is_empty());
    let instances = (0..4)
        .map(|turn| {
            let offset = if species == 2 {
                [[-112, 0, -96], [80, 0, -96], [-112, 0, 96], [80, 0, 96]][turn]
            } else {
                [
                    -112 + turn as i64 * 64,
                    0,
                    if species == 0 { 192 } else { 224 },
                ]
            };
            VegetationInstance {
                root_cell: std::array::from_fn(|d| ORIGIN[d] + offset[d]),
                quarter_turn: turn as u32,
                // Include canonical-cell dither without introducing temporal noise.
                fade: if turn % 2 == 0 { 1. } else { 0.73 },
            }
        })
        .collect();
    VegetationBatch {
        key: MeshKey {
            species,
            variant: 1,
            seed,
            tile: [0; 2],
            lod: 1,
            edit_id: 0,
        },
        mesh: Arc::new(mesh),
        instances,
    }
}
fn frozen_frame() -> VegetationFrame {
    VegetationFrame {
        shadow_transitions: Vec::new(),
        shadow_inspection: Vec::new(),
        shadow_coverage: Vec::new(),
        origin: ORIGIN,
        stats: Default::default(),
        shadow_batches: Vec::new(),
        batches: vec![
            frozen_batch(generate_rowan(0x8192_f00d, 1), 2, 0x8192_f00d),
            frozen_batch(generate_grass(0x3017, 1), 0, 0x3017),
            frozen_batch(generate_edelweiss(0x8123, 1), 1, 0x8123),
        ],
    }
}
fn texture(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frozen vegetation parity target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn readback(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let result = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    result
}

#[test]
#[ignore = "requires an explicitly selected hardware GPU; renders a frozen native plant frame"]
fn frozen_native_gpu_parity() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .expect("headless Vulkan adapter");
        eprintln!("frozen vegetation parity adapter: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("frozen vegetation parity"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await
            .unwrap();
        let mut frame = frozen_frame();
        // A distinct cached revision wholly outside the frustum must be culled
        // by the actual compute pipeline while preserving the frozen pixels.
        let mut outside = frame.batches[0].clone();
        outside.key.edit_id += 1;
        outside.instances.truncate(1);
        outside.instances[0].root_cell[0] += 16 * 100_000;
        frame.batches.push(outside);
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("parity camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let eye = Vec3::new(23., 16., 30.);
        let matrix = Mat4::perspective_rh(55f32.to_radians(), 1., 0.1, 100.)
            * Mat4::look_at_rh(eye, Vec3::new(0., 3., 2.), Vec3::Y);
        let mut camera_bytes = Vec::new();
        camera_bytes.extend_from_slice(bytemuck::cast_slice(&matrix.to_cols_array()));
        camera_bytes.extend_from_slice(bytemuck::cast_slice(&[eye.x, eye.y, eye.z, 0.]));
        camera_bytes.extend_from_slice(bytemuck::cast_slice(&[
            ORIGIN[0] as u32,
            ORIGIN[1] as u32,
            ORIGIN[2] as u32,
            0,
        ]));
        assert_eq!(camera_bytes.len(), 96);
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("parity frozen camera"),
            contents: &camera_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("parity frozen camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("inactive parity near shadow"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("inactive parity near shadow"),
            contents: &[0; 112],
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let shadow_layout = crate::adaptive_gpu::adaptive_shadow::near_sampling_layout(&device);
        let shadow = crate::adaptive_gpu::adaptive_shadow::near_sampling_bind(
            &device,
            &shadow_view,
            &sampler,
            &shadow_uniform,
            &crate::naadf::Bindings::empty(&device),
        );
        let color = texture(&device, wgpu::TextureFormat::Rgba8Unorm);
        let depth = texture(&device, wgpu::TextureFormat::Depth32Float);
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let buffers: Vec<_> = (0..2)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("parity readback"),
                    size: u64::from(SIZE * SIZE * 4),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let mut baseline: Option<(Vec<u8>, Vec<u8>)> = None;
        for mode in ["expanded", "compact", "compact-indexed", "pooled"] {
            let mut plants = VegetationGpu::new_with_mode(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
                1,
                &camera_layout,
                mode,
            );
            plants.set_indirect_supported(
                adapter
                    .get_downlevel_capabilities()
                    .flags
                    .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION),
            );
            let sun = Vec3::new(-0.4, 0.8, -0.2).normalize();
            queue.write_buffer(
                &plants.lighting_uniform,
                0,
                bytemuck::cast_slice(&[sun.x, sun.y, sun.z, 1., 0.2, 1., 0., 0.]),
            );
            plants
                .update(&device, &queue, &frame, ORIGIN, eye.to_array())
                .unwrap();
            let mut encoder = device.create_command_encoder(&Default::default());
            plants.encode_culling(&mut encoder, &camera);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("frozen native parity render"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                plants.draw(&mut pass, &camera, &shadow);
            }
            for (target, buffer) in [&color, &depth].into_iter().zip(&buffers) {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: target,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(SIZE * 4),
                            rows_per_image: Some(SIZE),
                        },
                    },
                    wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: 1,
                    },
                );
            }
            let commands = if plants.cull.encoded.get() {
                let bytes = (plants.color_draws.len() * 16) as u64;
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frozen cull command readback"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                encoder.copy_buffer_to_buffer(&plants.cull.output, 0, &buffer, 0, bytes);
                Some(buffer)
            } else {
                None
            };
            queue.submit(Some(encoder.finish()));
            if let Some(commands) = commands {
                let bytes = readback(&device, &commands);
                let counts: Vec<_> = bytes
                    .chunks_exact(16)
                    .map(|c| u32::from_le_bytes(c[4..8].try_into().unwrap()))
                    .collect();
                assert_eq!(
                    counts.last(),
                    Some(&0),
                    "wholly outside cluster must be rejected"
                );
                assert!(
                    counts[..counts.len() - 1].iter().any(|count| *count > 0),
                    "visible content must survive culling"
                );
            }
            let rgba = readback(&device, &buffers[0]);
            let depths = readback(&device, &buffers[1]);
            let covered = rgba.chunks_exact(4).filter(|p| p[3] != 0).count();
            let depth_covered = depths
                .chunks_exact(4)
                .filter(|p| f32::from_le_bytes((*p).try_into().unwrap()) < 1.)
                .count();
            assert!(
                covered > 500,
                "{mode}: empty or trivial color render ({covered} pixels)"
            );
            assert_eq!(
                covered, depth_covered,
                "{mode}: color/depth coverage disagree"
            );
            if let Some((reference_color, reference_depth)) = &baseline {
                let color_differences = rgba
                    .iter()
                    .zip(reference_color)
                    .filter(|(a, b)| a != b)
                    .count();
                let depth_differences = depths
                    .chunks_exact(4)
                    .zip(reference_depth.chunks_exact(4))
                    .filter(|(a, b)| a != b)
                    .count();
                assert_eq!(
                    color_differences, 0,
                    "{mode}: RGBA8 bytes differ from expanded"
                );
                assert_eq!(
                    depth_differences, 0,
                    "{mode}: Depth32 pixels differ from expanded"
                );
            } else {
                baseline = Some((rgba, depths));
            }
            eprintln!(
                "{mode}: {covered} identical covered pixels; {} resident GPU bytes",
                plants.stats().resident_bytes
            );
        }
    });
}
