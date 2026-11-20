use super::*;
const SHADER: &str = concat!(
    "const NAADF_GROUP:u32=0u;const NAADF_BASE:u32=0u;\n",
    include_str!("trace.wgsl"),
    r#"
@group(1) @binding(0) var<storage,read_write> results:array<vec4<f32>>;
@compute @workgroup_size(1) fn main() {
    for(var i=0u;i<2u;i++) {
        let origin=vec3(-1.,select(8.5,20.5,i==1u),8.5);let direction=vec3(1.,0.,0.);
        let h=trace_voxels(origin,direction,0.,false);
        let s=trace_voxels(origin,direction,0.,true);
        results[i]=vec4(select(-1.,h.t,h.found),f32(h.pigment),select(0.,1.,s.found),s.t);
        let ordinary=trace_static_prefixed(origin,direction,0.,0.,false,false);
        results[2u+i]=vec4(select(-1.,ordinary.t,ordinary.found),f32(ordinary.pigment),ordinary.normal.x,ordinary.normal.y);
        results[4u+i]=vec4(select(-1.,h.t,h.found),f32(h.pigment),h.normal.x,h.normal.y);
        let through_water=trace_water_optics(origin,direction,40.);
        let short=trace_water_optics(origin,direction,24.);
        results[6u+i]=vec4(select(-1.,through_water.t,through_water.found),
            f32(through_water.pigment),select(0.,1.,short.found),through_water.normal.x);
    }
}
"#,
);
#[test]
fn dynamic_water_shader_validates() {
    let module = naga::front::wgsl::parse_str(SHADER).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}
#[test]
#[ignore = "explicit GPU parity: native water additions, coarse source-water tombstones, shadows and negative origins"]
fn dynamic_water_primary_and_shadow_gpu_parity() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter
            .request_device(&Default::default(), None)
            .await
            .unwrap();
        let buffer = |data: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: data,
                usage,
            })
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 3,
                height: 3,
                depth_or_array_layers: 3,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // Y-fast: source Water in the first root, Stone immediately behind it.
        let mut roots = [0u32; 27];
        roots[3] = 0x40000007;
        roots[6] = 0x40000003;
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&roots),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(12),
                rows_per_image: Some(3),
            },
            texture.size(),
        );
        let mut pigments = vec![
            data::Pigment {
                rgb: [0.; 3],
                material: 0
            };
            10
        ];
        pigments[3].material = 3;
        pigments[7].material = 7;
        let mut resources = Bindings {
            uniform: buffer(
                bytemuck::bytes_of(&Uniform {
                    extent: [48, 48, 48, 3],
                    ..Uniform::zeroed()
                }),
                wgpu::BufferUsages::UNIFORM,
            ),
            chunks: texture.create_view(&Default::default()),
            distance: root_distance::empty(&device),
            height: traversal::empty_height(&device),
            diagnostics: diagnostics::Diagnostics::new(&device, false).buffer,
            blocks: buffer(&[0; 4], wgpu::BufferUsages::STORAGE),
            voxels: buffer(&[0; 4], wgpu::BufferUsages::STORAGE),
            palette: buffer(bytemuck::cast_slice(&pigments), wgpu::BufferUsages::STORAGE),
            dynamic_water: dynamic_water::empty(&device),
            ground_light: crate::terrain_light::buffer(&device),
        };
        let mut entries = binding_layout(0);
        for entry in &mut entries {
            entry.visibility = wgpu::ShaderStages::COMPUTE;
        }
        let hierarchy_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &entries,
        });
        let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&hierarchy_layout, &output_layout],
            push_constant_ranges: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 128,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let output_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &output_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            }],
        });
        for mode in 0..5 {
            // Mode 4 models recentering: the same complete occupancy snapshot
            // starts at relative -16 after a one-root native window shift.
            let origin = if mode == 4 { -16i32 } else { 0 };
            let size = [if mode == 4 { 48usize } else { 32 }, 32, 16];
            let mut words = vec![
                origin as u32,
                0,
                0,
                u32::from(mode == 2 || mode == 3),
                size[0] as u32,
                32,
                16,
                0,
            ];
            let mut states = vec![if mode == 0 { 0u32 } else { 1 }; size.into_iter().product()];
            if mode == 2 || mode == 3 {
                let y = if mode == 2 { 8 } else { 20 };
                states[8 + size[0] * (y + 32 * 8)] = 2;
            }
            words.extend(states);
            resources.dynamic_water =
                buffer(bytemuck::cast_slice(&words), wgpu::BufferUsages::STORAGE);
            if mode == 4 {
                // Exercise the exact immutable GPU copy used by native-window
                // publication, including preserved shape/occupancy header.
                let mut encoder = device.create_command_encoder(&Default::default());
                let mut original = words.clone();
                original[0] = 0;
                let previous = buffer(
                    bytemuck::cast_slice(&original),
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                );
                resources.dynamic_water = dynamic_water::retarget_buffer(
                    &device,
                    &queue,
                    &mut encoder,
                    &previous,
                    frontier_world::Cell { x: -16, y: 0, z: 0 },
                );
                queue.submit([encoder.finish()]);
            }
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &hierarchy_layout,
                entries: &resources.entries(0),
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 128,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.set_bind_group(1, &output_bind, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 128);
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                tx.send(r).unwrap();
            });
            device.poll(wgpu::Maintain::Wait);
            rx.recv().unwrap().unwrap();
            let mapped = readback.slice(..).get_mapped_range();
            let rows: &[[f32; 4]] = bytemuck::cast_slice(&mapped);
            let expected = match mode {
                0 => [17., 7., 1., 17.],
                2 => [9., 7., 1., 9.],
                _ => [33., 3., 1., 33.],
            };
            assert_eq!(rows[0], expected, "mode {mode} primary/shadow");
            if mode == 3 {
                assert_eq!(rows[1], [9., 7., 1., 9.]);
            } else {
                assert_eq!(rows[1][0], -1.);
                assert_eq!(rows[1][2], 0.);
            }
            assert_eq!(rows[6], [33., 3., 0., -1.], "mode {mode} optical ray");
            assert_eq!(rows[7][0], -1.);
            if mode == 0 {
                assert_eq!(rows[2], rows[4]);
                assert_eq!(rows[3], rows[5]);
            }
        }
    });
}
