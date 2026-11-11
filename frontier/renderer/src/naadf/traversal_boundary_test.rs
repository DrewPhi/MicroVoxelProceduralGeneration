use super::{data, Uniform};
use bytemuck::Zeroable;
use std::collections::HashMap;
use wgpu::util::DeviceExt;

const SHADER: &str = concat!(
    "const NAADF_GROUP:u32=0u;const NAADF_BASE:u32=0u;\n",
    include_str!("trace.wgsl"),
    r#"
@group(0) @binding(7) var<storage,read_write> result:array<vec4<f32>>;
fn sample(h:VoxelHit)->vec4<f32>{
    return vec4(h.normal,select(-1.,h.t,h.found && h.pigment==0u));
}
@compute @workgroup_size(1)
fn main(){
    let origin=vec3(0.,128.,8.5);
    let direction=normalize(vec3(1.,-1.,0.));
    let minimum=1.;
    result[0]=sample(trace_impl(origin,direction,minimum,false,false,false));
    result[1]=sample(trace_impl(origin,direction,minimum,false,false,true));
    result[2]=sample(trace_counted(origin,direction,minimum,false,false));
    // A prefix of 64 native units ends safely in air; this corner is
    // reached at ~90.5 units. Compare the same resumed primary path.
    result[3]=sample(trace_impl(origin,direction,64.,false,false,false));
    result[4]=sample(trace_prefixed(origin,direction,minimum,64.,false,false));
    result[5]=sample(trace_impl(origin,direction,minimum,true,false,false));
    result[6]=sample(trace_counted(origin,direction,minimum,true,false));
    result[7]=sample(trace_prefixed(origin,direction,minimum,64.,true,false));
}
"#
);

#[test]
#[ignore = "explicit GPU parity: height and prefix shortcuts preserve exact corner faces"]
fn accelerated_primary_replays_ambiguous_corner_face() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("GPU oracle needs a Vulkan adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .unwrap();

        const N: usize = 16;
        let mut roots = vec![0_u32; N.pow(3)];
        for z in 0..N {
            for (x, y) in [(4, 3), (6, 4)] {
                roots[z * N * N + x * N + y] = 1 << 30;
            }
        }
        data::bounds(&mut roots, [N; 3], [N, 1, N * N], 5, &|| false).unwrap();
        let upload = |dimension, size: [u32; 3], words: &[u32]| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("NAADF exact corner fixture"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: size[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(words),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * size[0]),
                    rows_per_image: Some(size[1]),
                },
                texture.size(),
            );
            texture.create_view(&Default::default())
        };
        let roots = upload(wgpu::TextureDimension::D3, [N as u32; 3], &roots);
        let distance = upload(wgpu::TextureDimension::D3, [1; 3], &[0]);
        let mut heights = [0_u32; 16];
        // Four-root x/z tiles. Both occupied x roots are in tile x=1.
        for row in heights.chunks_exact_mut(4) {
            row[1] = 5 * 16;
        }
        let heights = upload(wgpu::TextureDimension::D2, [4, 4, 1], &heights);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&Uniform {
                extent: [256, 256, 256, N as u32],
                ..Uniform::zeroed()
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let empty = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0; 4],
            usage: wgpu::BufferUsages::STORAGE,
        });
        let buffer = |usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 8 * 16,
                usage,
                mapped_at_creation: false,
            })
        };
        let output = buffer(wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        let readback = buffer(wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("NAADF exact corner oracle"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("NAADF exact corner oracle"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &HashMap::from([
                    ("traversal_height_bounds".into(), 1.),
                    ("traversal_height_tile".into(), 4.),
                ]),
                ..Default::default()
            },
            cache: None,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                (0, uniform.as_entire_binding()),
                (1, wgpu::BindingResource::TextureView(&roots)),
                (2, empty.as_entire_binding()),
                (3, empty.as_entire_binding()),
                (5, wgpu::BindingResource::TextureView(&distance)),
                (6, wgpu::BindingResource::TextureView(&heights)),
                (7, output.as_entire_binding()),
            ]
            .map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource }),
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, readback.size());
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let bytes = readback.slice(..).get_mapped_range();
        let hits = bytemuck::cast_slice::<u8, [f32; 4]>(&bytes);
        assert_eq!(hits[0][..3], [-1., 0., 0.], "{hits:?}");
        assert!((hits[0][3] - 64. * std::f32::consts::SQRT_2).abs() < 1e-4);
        // The unguarded paths can pick another face or miss entirely,
        // depending on floating-point contraction at the corner.
        assert_ne!(hits[1], hits[0], "{hits:?}");
        assert_ne!(hits[3], hits[0], "{hits:?}");
        assert_eq!(hits[2], hits[0], "{hits:?}");
        assert_eq!(hits[4], hits[0], "{hits:?}");
        assert_eq!(hits[5][3], hits[0][3], "{hits:?}");
        assert_eq!(hits[6], hits[5], "{hits:?}");
        assert_eq!(hits[7], hits[5], "{hits:?}");
    });
}
