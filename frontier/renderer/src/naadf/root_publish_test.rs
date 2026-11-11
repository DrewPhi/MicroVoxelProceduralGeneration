use super::super::{binding_layout, diagnostics, traversal, Bindings, Uniform};
use super::*;
use bytemuck::Zeroable;

// A reference implementation of the tile update, checked against the raw
// occupancy oracle rather than against another acceleration representation.
fn repair_axis(input: &[u32], axis: usize) -> Vec<u32> {
    let n = SIDE as usize;
    let index = |p: [usize; 3]| p[0] + n * (p[1] + n * p[2]);
    (0..input.len())
        .map(|i| {
            let p = [i % n, i / n % n, i / (n * n)];
            let old = input[i];
            let mut out = old;
            if old & 0xc0000000 != 0 {
                return out;
            }
            for side in axis * 2..axis * 2 + 2 {
                let shift = side * 5;
                let extent = (old >> shift) & 31;
                let positive = side & 1 != 0;
                let coordinate = p[axis] as i32
                    + if positive {
                        extent as i32 + 1
                    } else {
                        -(extent as i32 + 1)
                    };
                if coordinate < 0 || coordinate >= n as i32 {
                    continue;
                }
                let mut neighbor = p;
                neighbor[axis] = coordinate as usize;
                let other = input[index(neighbor)];
                if other & 0xc0000000 != 0 {
                    continue;
                }
                let covers = (0..6)
                    .all(|t| t / 2 == axis || (other >> (t * 5)) & 31 >= (old >> (t * 5)) & 31);
                if covers {
                    let available = if positive { n - 1 - p[axis] } else { p[axis] } as u32;
                    let grown = available.min(extent + 1 + ((other >> shift) & 31));
                    out = (out & !(31 << shift)) | (grown << shift);
                }
            }
            out
        })
        .collect()
}

#[test]
fn seeded_local_repair_grows_strip_proofs_without_changing_geometry() {
    let dims = [40usize, 12, 12];
    let index = |p: [usize; 3]| p[0] + dims[0] * (p[1] + dims[1] * p[2]);
    let mut raw = vec![0u32; dims.iter().product()];
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                if x == 3 || x == 35 || y == 1 || y == 10 || z == 1 || z == 10 {
                    raw[index([x, y, z])] = if (x + y + z) & 1 == 0 {
                        0x80000000 | 19
                    } else {
                        0x40000000 | 7
                    };
                }
            }
        }
    }
    let mut trusted = raw.clone();
    // Generate independent x-strips exactly as the streaming builder does.
    for lo in [0, 20] {
        let mut strip: Vec<_> = (0..12)
            .flat_map(|z| (0..12).flat_map(move |y| (lo..lo + 20).map(move |x| [x, y, z])))
            .map(|p| raw[index(p)])
            .collect();
        super::super::data::bounds(&mut strip, [20, 12, 12], [1, 20, 240], 5, &|| false).unwrap();
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..20 {
                    trusted[index([x + lo, y, z])] = strip[x + 20 * (y + 12 * z)];
                }
            }
        }
    }
    let base = [4i64, -10, -10];
    let n = SIDE as usize;
    let coordinates = |i: usize| -> Point {
        let p = [i % n, i / n % n, i / (n * n)];
        std::array::from_fn(|a| base[a] + p[a] as i64)
    };
    let valid = |p: Point| (0..3).all(|a| p[a] >= 0 && p[a] < dims[a] as i64);
    let mut repaired: Vec<_> = (0..n.pow(3))
        .map(|i| {
            let p = coordinates(i);
            if valid(p) {
                trusted[index(p.map(|v| v as usize))]
            } else {
                0xc0000000
            }
        })
        .collect();
    let seeded = repaired.clone();
    for _ in 0..5 {
        for axis in 0..3 {
            repaired = repair_axis(&repaired, axis);
        }
    }
    let mut grown = 0;
    let mut crosses_tile = 0;
    for (i, (&before, &after)) in seeded.iter().zip(&repaired).enumerate() {
        if before & 0xc0000000 != 0 {
            assert_eq!(before, after, "occupied/root pointer changed");
            continue;
        }
        grown += usize::from(before != after);
        for side in 0..6 {
            assert!((after >> (side * 5)) & 31 >= (before >> (side * 5)) & 31);
        }
        let p = coordinates(i);
        let lo: Point = std::array::from_fn(|a| p[a] - i64::from((after >> (a * 10)) & 31));
        let hi: Point = std::array::from_fn(|a| p[a] + i64::from((after >> (a * 10 + 5)) & 31));
        assert!(valid(lo) && valid(hi));
        crosses_tile +=
            usize::from((0..3).any(|a| lo[a] < base[a] || hi[a] >= base[a] + i64::from(SIDE)));
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    assert_eq!(raw[index([x as usize, y as usize, z as usize])], 0);
                }
            }
        }
    }
    assert!(
        grown > 0,
        "local repair must improve the independent-strip seam"
    );
    // Move the tile boundary into pre-existing proofs: trusted cuboids can
    // extend beyond tile support, and must remain valid rather than shrink.
    // The chosen left edge lies immediately beside the x=3 obstacle, so this
    // fixture's proofs need not cross it; test an explicit translated tile.
    let mut seed = vec![0xc0000000u32; n.pow(3)];
    seed[0] = 7; // independently certified negative-X extent outside the tile
    assert_eq!(repair_axis(&seed, 0)[0], 7);
    eprintln!("{{\"test\":\"seeded_local_repair\",\"improved_roots\":{grown},\"preserved_cross_tile_proofs\":{crosses_tile}}}");
}

#[test]
fn canonical_root_formats_reserve_both_flags_for_direct_provenance() {
    for payload in [0, 1, (1 << 30) - 1] {
        for kind in [0, 1 << 30, 1 << 31] {
            assert_ne!((kind | payload) & REPAIRED_ROOT_TAG, REPAIRED_ROOT_TAG);
        }
    }
    for gpu_distance in [false, true] {
        let snapshot = super::super::data::build_with_root_mode(
            &frontier_world::World::new(7),
            [0., 0., 0.],
            4,
            128 * 1048576,
            &std::sync::atomic::AtomicU32::new(0),
            gpu_distance,
            || false,
        )
        .unwrap();
        assert!(snapshot
            .chunks
            .iter()
            .all(|v| v & REPAIRED_ROOT_TAG != REPAIRED_ROOT_TAG));
    }
}

const CORNER_SHADER: &str = concat!(
    "const NAADF_GROUP:u32=0u;const NAADF_BASE:u32=0u;\n",
    include_str!("trace.wgsl"),
    r#"
@group(1) @binding(0) var<storage,read_write> result:array<vec4<f32>>;
fn corner_sample(h:VoxelHit)->vec4<f32>{return vec4(h.normal,select(-1.,h.t,h.found && h.pigment==0u));}
@compute @workgroup_size(1)
fn direct_corner(){
    let o=vec3(0.,128.,8.5);let d=normalize(vec3(1.,-1.,0.));
    result[0]=corner_sample(trace_counted(o,d,1.,false,false));
    result[1]=corner_sample(trace_counted(o,d,1.,true,false));
    result[2]=corner_sample(trace_native(o,d,1.,false,false));
    result[3]=corner_sample(trace_native(o,d,1.,true,false));
    let up=normalize(vec3(1.,-1.+0.000001,0.));
    let down=normalize(vec3(1.,-1.-0.000001,0.));
    result[4]=corner_sample(trace_counted(o,up,1.,false,false));
    result[5]=corner_sample(trace_native(o,up,1.,false,false));
    result[6]=corner_sample(trace_counted(o,down,1.,false,false));
    result[7]=corner_sample(trace_native(o,down,1.,false,false));
    // Traverse an entire line of tagged uniform-air roots after publication.
    result[8]=corner_sample(trace_counted(o,vec3(0.,0.,1.),1.,false,false));
    result[9]=corner_sample(trace_counted(o,vec3(0.,0.,1.),1.,true,false));
}
"#
);

#[test]
fn direct_corner_shader_validates() {
    let module = naga::front::wgsl::parse_str(CORNER_SHADER).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
#[ignore = "explicit GPU parity: directly published proofs preserve native corner and near-corner primary/shadow hits"]
fn directly_published_roots_match_native_corner_primary_and_shadow() {
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
        const N: u32 = 16;
        let roots = texture(
            &device,
            N,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_DST,
        );
        let mut words = vec![0u32; N.pow(3) as usize];
        for z in 0..N {
            for (x, y) in [(4, 3), (6, 4)] {
                words[(y + N * (x + N * z)) as usize] = 1 << 30;
            }
        }
        queue.write_texture(
            roots.as_image_copy(),
            bytemuck::cast_slice(&words),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(N * 4),
                rows_per_image: Some(N),
            },
            roots.size(),
        );
        let root_view = roots.create_view(&Default::default());
        let (mut cache, distance) = Cache::new_configured(
            &device,
            N,
            Region {
                lo: [0; 3],
                hi: [i64::from(N); 3],
            },
            &root_view,
            true,
            true,
        );
        let diagnostics = diagnostics::Diagnostics::new(&device, true);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&Uniform {
                extent: [N * 16, N * 16, N * 16, N],
                ..Uniform::zeroed()
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let empty = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0; 16],
            usage: wgpu::BufferUsages::STORAGE,
        });
        let resources = Bindings {
            uniform,
            chunks: root_view,
            distance,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&hierarchy_layout, &output_layout],
            push_constant_ranges: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(CORNER_SHADER.into()),
        });
        let make_pipeline = |direct: bool| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("direct_corner"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &std::collections::HashMap::from([
                        ("traversal_direct_roots".into(), f64::from(u8::from(direct))),
                        ("traversal_metrics".into(), 1.),
                    ]),
                    ..Default::default()
                },
                cache: None,
            })
        };
        let pipeline = make_pipeline(true);
        let control = make_pipeline(false);
        let hierarchy = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &hierarchy_layout,
            entries: &resources.entries(0),
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 160,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 160,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let result_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &output_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            }],
        });
        let check = |pipeline: &wgpu::ComputePipeline| {
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &hierarchy, &[]);
                pass.set_bind_group(1, &result_group, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 160);
            queue.submit(Some(encoder.finish()));
            let (tx, rx) = mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::Maintain::Wait);
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range();
            let hits: Vec<[f32; 4]> = bytemuck::cast_slice(&bytes).to_vec();
            drop(bytes);
            readback.unmap();
            hits
        };
        let cold = check(&pipeline);
        let original = check(&control);
        assert_eq!(
            cold, original,
            "unmodified CPU roots must use the exact original traversal"
        );
        let counters = diagnostics.read_words(&device, &queue).unwrap().unwrap();
        assert_eq!(
            counters[30] + counters[62],
            0,
            "untouched proofs must not cause native replay"
        );
        while cache.progress().0 < cache.progress().1 {
            cache.advance(&device, &queue, N, [0; 3], Camera::fixture());
            device.poll(wgpu::Maintain::Wait);
        }
        let warm = check(&pipeline);
        assert_eq!(warm[0][..3], [-1., 0., 0.], "{warm:?}");
        assert!(
            (warm[0][3] - 64. * std::f32::consts::SQRT_2).abs() < 1e-4,
            "{warm:?}"
        );
        assert_eq!(warm[0], warm[2]);
        assert_eq!(warm[1], warm[3]);
        assert_eq!(warm[1][3], warm[0][3]);
        assert_eq!(warm[4], warm[5]);
        assert_eq!(warm[6], warm[7]);
        assert_eq!(
            warm[8],
            [0., 0., 0., -1.],
            "tagged air became primary geometry"
        );
        assert_eq!(
            warm[9],
            [0., 0., 0., -1.],
            "tagged air became shadow geometry"
        );
        assert_eq!(cache.counters.direct_publications, 1);
        let counters = diagnostics.read_words(&device, &queue).unwrap().unwrap();
        assert!(
            counters[30] + counters[62] > 0,
            "corner replay must actually execute"
        );
        assert!(
            counters[31] + counters[63] > 0,
            "native replay must advance cells"
        );
    });
}
