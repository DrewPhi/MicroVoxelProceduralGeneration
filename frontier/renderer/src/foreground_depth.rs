//! Same-frame conservative occlusion for the rolling terrain fragment pass.
use std::cell::RefCell;

const TILE: u32 = 4;

const COPY: &str = r#"
@group(0) @binding(0) var prior: DEPTH_TYPE;
@group(0) @binding(1) var reduced: texture_storage_2d<r32float,write>;
@compute @workgroup_size(8,8) fn build(@builtin(global_invocation_id) i:vec3u) {
    if any(i.xy>=textureDimensions(reduced)) {return;}
    let size=vec2i(textureDimensions(prior));
    var far=0.;
    for (var y=0i;y<4i;y+=1i) {
        for (var x=0i;x<4i;x+=1i) {
            let p=vec2i(i.xy*4u)+vec2i(x,y);
            if all(p<size) {far=max(far,DEPTH_LOAD);}
        }
    }
    textureStore(reduced,vec2i(i.xy),vec4f(far,0.,0.,0.));
}"#;

pub(crate) const SAMPLE: &str = r#"
@group(1) @binding(0) var foreground_depth: texture_2d<f32>;
fn foreground_at(pixel:vec2u)->f32 {
    let size=textureDimensions(foreground_depth);
    if size.x<=1u {return 1.;}
    return textureLoad(foreground_depth,vec2i(pixel/4u),0).x;
}"#;

struct Frame {
    size: [u32; 2],
    texture: wgpu::TextureView,
    read: wgpu::BindGroup,
}

pub(crate) struct ForegroundDepth {
    pub read_layout: wgpu::BindGroupLayout,
    fallback: wgpu::BindGroup,
    write_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    frame: RefCell<Option<Frame>>,
}

impl ForegroundDepth {
    pub(crate) fn new(device: &wgpu::Device, samples: u32) -> Self {
        let texture = |binding, visibility, ty, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: ty,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let read_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain foreground depth reads"),
            entries: &[texture(
                0,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::TextureSampleType::Float { filterable: false },
                false,
            )],
        });
        let write_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain foreground depth reduction"),
            entries: &[
                texture(
                    0,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::TextureSampleType::Depth,
                    samples > 1,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let (depth_type, load) = if samples > 1 {
            ("texture_depth_multisampled_2d",
             "max(max(textureLoad(prior,p,0),textureLoad(prior,p,1)),max(textureLoad(prior,p,2),textureLoad(prior,p,3)))")
        } else {
            ("texture_depth_2d", "textureLoad(prior,p,0)")
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain foreground depth reduction"),
            source: wgpu::ShaderSource::Wgsl(
                COPY.replace("DEPTH_TYPE", depth_type)
                    .replace("DEPTH_LOAD", load)
                    .into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain foreground depth reduction"),
            bind_group_layouts: &[&write_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("terrain foreground depth reduction"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("build"),
            compilation_options: Default::default(),
            cache: None,
        });
        let fallback = Self::frame(device, &read_layout, [1, 1]).read;
        Self {
            read_layout,
            fallback,
            write_layout,
            pipeline,
            frame: RefCell::new(None),
        }
    }

    fn frame(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, size: [u32; 2]) -> Frame {
        let texture = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("conservative terrain foreground depth"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let read = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("read conservative terrain foreground depth"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&texture),
            }],
        });
        Frame {
            size,
            texture,
            read,
        }
    }

    pub(crate) fn fallback(&self) -> &wgpu::BindGroup {
        &self.fallback
    }

    pub(crate) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> wgpu::BindGroup {
        let size = [width.div_ceil(TILE), height.div_ceil(TILE)];
        let mut cache = self.frame.borrow_mut();
        if cache.as_ref().is_none_or(|old| old.size != size) {
            *cache = Some(Self::frame(device, &self.read_layout, size));
        }
        let frame = cache.as_ref().unwrap();
        let write = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("capture current terrain foreground depth"),
            layout: &self.write_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&frame.texture),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("reduce foreground depth for rolling terrain"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &write, &[]);
        pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        frame.read.clone()
    }

    pub(crate) fn bytes(&self) -> usize {
        self.frame
            .borrow()
            .as_ref()
            .map_or(4, |f| f.size[0] as usize * f.size[1] as usize * 4 + 4)
    }
}
