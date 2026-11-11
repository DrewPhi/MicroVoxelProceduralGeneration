use super::{binding_layout, diagnostics, traversal, Bindings, Uniform};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Case {
    // origin.w: 1 must certify; 0 may conservatively reject; -1 must reject.
    origin: [f32; 4],
    direction: [f32; 4],
    lo: [i32; 4],
    hi: [i32; 4],
    // cell.w: execute end-to-end primary and shadow comparisons.
    cell: [i32; 4],
    times: [f32; 4],
}

const SHADER: &str = concat!(
    "const NAADF_GROUP:u32=0u;const NAADF_BASE:u32=0u;
",
    include_str!("trace.wgsl"),
    r#"
struct Case {
    origin:vec4<f32>,direction:vec4<f32>,lo:vec4<i32>,hi:vec4<i32>,cell:vec4<i32>,times:vec4<f32>
};
@group(1) @binding(0) var<storage,read> cases:array<Case>;
@group(1) @binding(1) var<storage,read_write> results:array<vec4<f32>>;

// Independent unit-grid traversal: advance crossed integer axes, never
// reclassify transverse coordinates using a rounded o+d*t.
fn unit_exit(o:vec3<f32>,d:vec3<f32>,start:vec3<i32>,minimum:f32,tau:f32)->LocalExit {
    var cell=start;
    if !(tau>minimum && abs(tau)<1.e29) {return LocalExit(false,cell,minimum,0u);}
    for(var iteration=0u;iteration<1024u;iteration++) {
        var next=vec3(1e30);
        for(var a=0u;a<3u;a++){
            if d[a]!=0. {next[a]=(f32(cell[a]+select(0,1,d[a]>0.))-o[a])/d[a];}
        }
        let t=min(next.x,min(next.y,next.z));
        if t>tau {break;}
        var faces=0u;
        for(var a=0u;a<3u;a++){
            if next[a]==t {
                cell[a]+=select(-1,1,d[a]>0.);
                faces|=1u<<(a*2u+select(1u,0u,d[a]>0.));
            }
        }
        if t==tau {return LocalExit(true,cell,t,faces);}
    }
    return LocalExit(false,cell,tau,0u);
}
fn hit_sample(hit:VoxelHit)->vec4<f32>{
    return vec4(hit.normal,select(-1.,hit.t,hit.found));
}
@compute @workgroup_size(1)
fn main() {
    for(var index=0u;index<arrayLength(&cases);index++){
        let c=cases[index];let o=c.origin.xyz;let d=normalize(c.direction.xyz);
        var next=vec3(1e30);
        for(var a=0u;a<3u;a++){
            if d[a]!=0. {next[a]=(f32(select(c.lo[a],c.hi[a],d[a]>0.))-o[a])/d[a];}
        }
        let tau=min(next.x,min(next.y,next.z));
        let repaired=local_exit(o,d,c.lo.xyz,c.hi.xyz,c.cell.xyz,c.times.x,next);
        let reference=unit_exit(o,d,c.cell.xyz,c.times.x,tau);
        let base=index*7u;
        results[base]=vec4(vec3<f32>(repaired.cell),select(0.,1.,repaired.valid));
        results[base+1u]=vec4(vec3<f32>(reference.cell),f32(reference.faces));
        results[base+2u]=vec4(repaired.t,f32(repaired.faces),reference.t,select(0.,1.,reference.valid));
        results[base+3u]=vec4(0.);results[base+4u]=vec4(0.);
        results[base+5u]=vec4(0.);results[base+6u]=vec4(0.);
        if c.cell.w!=0 {
            let before_local=atomicLoad(&trace_metrics[19u])+atomicLoad(&trace_metrics[51u]);
            let before_native=atomicLoad(&trace_metrics[30u])+atomicLoad(&trace_metrics[62u]);
            let hit=trace_counted(o,d,c.times.y,false,false);
            let shadow=trace_counted(o,d,c.times.y,true,false);
            let native=trace_native(o,d,c.times.y,false,false);
            let native_shadow=trace_native(o,d,c.times.y,true,false);
            results[base+3u]=hit_sample(hit);
            results[base+4u]=hit_sample(native);
            results[base+5u]=vec4(select(-1.,shadow.t,shadow.found),select(-1.,native_shadow.t,native_shadow.found),f32(hit.pigment),f32(native.pigment));
            results[base+6u]=vec4(
                f32(atomicLoad(&trace_metrics[19u])+atomicLoad(&trace_metrics[51u])-before_local),
                f32(atomicLoad(&trace_metrics[30u])+atomicLoad(&trace_metrics[62u])-before_native),0.,0.);
        }
    }
}
"#
);

fn fixture_cases() -> Vec<Case> {
    let make = |o: [f32; 3],
                d: [f32; 3],
                lo: [i32; 3],
                hi: [i32; 3],
                cell: [i32; 3],
                t: f32,
                min: f32,
                valid: f32,
                end_to_end: bool| {
        Case {
            origin: [o[0], o[1], o[2], valid],
            direction: [d[0], d[1], d[2], 0.],
            lo: [lo[0], lo[1], lo[2], 0],
            hi: [hi[0], hi[1], hi[2], 0],
            cell: [cell[0], cell[1], cell[2], i32::from(end_to_end)],
            times: [t, min, 0., 0.],
        }
    };
    let mut cases = Vec::new();
    for dy in [-1., -1. + 0.000001, -1. - 0.000001] {
        cases.push(make(
            [0., 128., 8.5],
            [1., dy, 0.],
            [0, 64, 0],
            [64, 128, 16],
            [0, 127, 8],
            1.,
            1.,
            1.,
            true,
        ));
    }
    // More than16 native planes from the current cell to the coarse face.
    // o.y+d.y*96 rounds to64, although T_y(64)=96.13037>96:
    // a floor-based transverse handoff incorrectly chooses Y63 instead of64.
    for dy in [-0.00001, -0.00001 * (1. + 0.0001), -0.00001 * (1. - 0.0001)] {
        cases.push(make(
            [0., 64.00096130371094, 8.5],
            [1., dy, 0.],
            [0, 64, 0],
            [96, 80, 16],
            [1, 64, 8],
            1.,
            1.,
            1.,
            true,
        ));
    }
    cases.push(make(
        [0., 64.5, 8.5],
        [1., 0., 0.],
        [0, 64, 0],
        [96, 80, 16],
        [1, 64, 8],
        1.,
        1.,
        1.,
        true,
    ));
    cases.push(make(
        [-32., 64.00096130371094, 8.5],
        [1., -0.00001, 0.],
        [0, 64, 0],
        [96, 80, 16],
        [1, 64, 8],
        33.,
        1.,
        0.,
        true,
    ));
    cases.push(make(
        [-1.0e9, 65.00001, 8.5],
        [1., -0.000000001, 0.],
        [0, 64, 0],
        [96, 80, 16],
        [0, 64, 8],
        1.0e9,
        1.,
        -1.,
        true,
    ));
    cases.push(make(
        [0., 64.5, 300.],
        [1., 0., 0.],
        [0, 64, 0],
        [96, 80, 16],
        [1, 64, 8],
        1.,
        1.,
        -1.,
        true,
    ));
    cases.push(make(
        [f32::NAN, 64.5, 8.5],
        [1., 0., 0.],
        [0, 64, 0],
        [96, 80, 16],
        [1, 64, 8],
        1.,
        1.,
        -1.,
        false,
    ));
    cases
}

#[test]
fn local_exit_fixture_shader_validates() {
    let module = naga::front::wgsl::parse_str(SHADER).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
#[ignore = "explicit GPU parity: locally repaired coarse exits match independent native-plane events and guarded native fallback"]
fn local_exit_matches_native_events_and_hit_material_shadow() {
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
        const N: usize = 16;
        let mut roots = vec![0u32; N.pow(3)];
        for z in 0..N {
            roots[(z * N + 4) * N + 3] = (1 << 30) | 7;
            roots[(z * N + 6) * N + 4] = (1 << 30) | 11;
            // Two explicitly certified air cuboids, then occupied roots just
            // beyond their faces. The tag models direct GPU publication.
            roots[(z * N) * N + 7] = 0xc0000000 | (3 << 5) | (3 << 10);
            roots[(z * N) * N + 4] = 0xc0000000 | (5 << 5);
        }
        let upload = |side: u32, words: &[u32]| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("local exit canonical root fixture"),
                size: wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: side,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(words),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(side * 4),
                    rows_per_image: Some(side),
                },
                texture.size(),
            );
            texture.create_view(&Default::default())
        };
        let diagnostics = diagnostics::Diagnostics::new(&device, true);
        let buffer = |contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents,
                usage,
            })
        };
        let empty = buffer(&[0; 16], wgpu::BufferUsages::STORAGE);
        let resources = Bindings {
            uniform: buffer(
                bytemuck::bytes_of(&Uniform {
                    extent: [256, 256, 256, N as u32],
                    ..Uniform::zeroed()
                }),
                wgpu::BufferUsages::UNIFORM,
            ),
            chunks: upload(N as u32, &roots),
            distance: upload(1, &[0]),
            height: traversal::empty_height(&device),
            diagnostics: diagnostics.buffer.clone(),
            blocks: empty.clone(),
            voxels: empty.clone(),
            palette: empty,
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
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: binding == 0,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
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
            label: Some("local native-plane exit oracle"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &std::collections::HashMap::from([
                    ("traversal_direct_roots".into(), 1.),
                    ("traversal_local_exit_repair".into(), 1.),
                    ("traversal_metrics".into(), 1.),
                ]),
                ..Default::default()
            },
            cache: None,
        });
        let cases = fixture_cases();
        let input = buffer(bytemuck::cast_slice(&cases), wgpu::BufferUsages::STORAGE);
        let bytes = (cases.len() * 7 * 16) as u64;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let hierarchy = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &hierarchy_layout,
            entries: &resources.entries(0),
        });
        let result_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &output_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &hierarchy, &[]);
            pass.set_bind_group(1, &result_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        let rows: &[[f32; 4]] = bytemuck::cast_slice(&mapped);
        for (index, case) in cases.iter().enumerate() {
            let row = &rows[index * 7..index * 7 + 7];
            let valid = row[0][3] != 0.;
            if case.origin[3] > 0. {
                assert!(valid, "fixture {index} must certify: {row:?}");
            }
            if case.origin[3] < 0. {
                assert!(!valid, "fixture {index} must reject: {row:?}");
            }
            if valid {
                assert_eq!(row[2][3], 1., "reference failed fixture {index}: {row:?}");
                assert_eq!(row[0][..3], row[1][..3], "cell fixture {index}");
                assert_eq!(row[2][0], row[2][2], "time fixture {index}");
                assert_eq!(row[2][1], row[1][3], "faces fixture {index}");
            }
            if case.cell[3] != 0 {
                assert_eq!(row[3], row[4], "primary fixture {index}: {row:?}");
                assert_eq!(row[5][0], row[5][1], "shadow fixture {index}: {row:?}");
                assert_eq!(row[5][2], row[5][3], "material fixture {index}: {row:?}");
            }
        }
        // The large grazing cube actually uses local handoff instead of a
        // whole-ray native replay, while a failed guard keeps the fallback.
        assert!(
            rows[3 * 7 + 6][0] > 0.,
            "grazing fixture did no local repair"
        );
        assert_eq!(
            rows[3 * 7 + 6][1],
            0.,
            "grazing fixture still used global replay"
        );
        assert!(
            rows[8 * 7 + 6][1] > 0.,
            "uncertifiable huge-origin case lost fallback"
        );
    });
}
