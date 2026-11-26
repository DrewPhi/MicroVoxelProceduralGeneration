use super::{
    cache::{Cache, Node, Upload},
    Result, View,
};

pub struct Gpu {
    pub info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    nodes: wgpu::Buffer,
    heights: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    queries: wgpu::QuerySet,
    query_resolve: wgpu::Buffer,
    pixel_bytes: u64,
    display: Option<Display>,
}
impl Gpu {
    pub fn present(&mut self) -> Result<()> {
        if let Some(display) = &mut self.display {
            display.present(&self.device, &self.queue)?;
        }
        Ok(())
    }

    pub async fn new(
        width: u32,
        height: u32,
        nodes: usize,
        capacity_words: u32,
        software: bool,
        stackless: bool,
        window: Option<std::sync::Arc<winit::window::Window>>,
    ) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = window
            .as_ref()
            .map(|w| instance.create_surface(w.clone()))
            .transpose()?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: surface.as_ref(),
                ..Default::default()
            })
            .await
            .ok_or("no Vulkan adapter")?;
        let info = adapter.get_info();
        eprintln!("Adapter {info:?}");
        if !software && info.device_type == wgpu::DeviceType::Cpu {
            return Err("software Vulkan requires --allow-software".into());
        }
        let height_bytes = 4 * u64::from(capacity_words);
        let pixel_bytes = 8 * u64::from(width) * u64::from(height);
        let max_storage = adapter.limits().max_storage_buffer_binding_size;
        if height_bytes > u64::from(max_storage) {
            return Err(
                format!("page cache exceeds adapter's {max_storage}-byte binding limit").into(),
            );
        }
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: wgpu::Limits::default()
                .max_storage_buffer_binding_size
                .max(height_bytes as u32),
            ..Default::default()
        };
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::TIMESTAMP_QUERY,
                    required_limits: limits,
                    ..Default::default()
                },
                None,
            )
            .await?;
        let buffer = |size, usage, label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        use wgpu::BufferUsages as U;
        let uniform = buffer(
            std::mem::size_of::<View>() as u64,
            U::UNIFORM | U::COPY_DST,
            "camera",
        );
        let nodes = buffer(
            (nodes * std::mem::size_of::<Node>()) as u64,
            U::STORAGE | U::COPY_DST,
            "bounds",
        );
        let heights = buffer(height_bytes, U::STORAGE | U::COPY_DST, "native heights");
        let output = buffer(pixel_bytes, U::STORAGE | U::COPY_SRC, "center hits");
        let readback = buffer(
            pixel_bytes + 16,
            U::MAP_READ | U::COPY_DST,
            "hits and timestamps",
        );
        let query_resolve = buffer(16, U::QUERY_RESOLVE | U::COPY_SRC, "timestamps");
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("terrain draw"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("exact terrain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("trace.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("exact ground visibility"),
            layout: None,
            module: &shader,
            entry_point: Some("visibility"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &std::collections::HashMap::from([(
                    "STACKLESS".into(),
                    f64::from(stackless),
                )]),
                ..Default::default()
            },
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("exact ground"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &std::array::from_fn::<_, 4, _>(|i| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: [&uniform, &nodes, &heights, &output][i].as_entire_binding(),
            }),
        });
        let display = if let (Some(window), Some(surface)) = (window, surface) {
            Some(Display::new(
                window, surface, &adapter, &device, &uniform, &output,
            )?)
        } else {
            None
        };
        Ok(Self {
            info,
            device,
            queue,
            pipeline,
            group,
            uniform,
            nodes,
            heights,
            output,
            readback,
            queries,
            query_resolve,
            pixel_bytes,
            display,
        })
    }

    pub fn bounds(&self, cache: &Cache) {
        self.queue
            .write_buffer(&self.nodes, 0, bytemuck::cast_slice(&cache.nodes));
    }

    pub fn publish(&self, cache: &Cache, upload: &Upload) {
        if !upload.columns.is_empty() {
            let begin = 4 * u64::from(upload.first_word);
            self.queue
                .write_buffer(&self.heights, begin, bytemuck::cast_slice(&upload.columns));
        }
        self.bounds(cache);
    }

    pub fn render(&self, view: &View) -> Result<(Vec<[u32; 2]>, f64)> {
        self.queue
            .write_buffer(&self.uniform, 0, bytemuck::bytes_of(view));
        let mut e = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = e.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                    query_set: &self.queries,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }),
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            pass.dispatch_workgroups(view.size[0].div_ceil(8), view.size[1].div_ceil(8), 1);
        }
        e.resolve_query_set(&self.queries, 0..2, &self.query_resolve, 0);
        e.copy_buffer_to_buffer(&self.output, 0, &self.readback, 0, self.pixel_bytes);
        e.copy_buffer_to_buffer(&self.query_resolve, 0, &self.readback, self.pixel_bytes, 16);
        self.queue.submit([e.finish()]);
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |status| {
            let _ = tx.send(status);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let bytes = slice.get_mapped_range();
        let pixels = bytemuck::cast_slice(&bytes[..self.pixel_bytes as usize]).to_vec();
        let stamps: &[u64] = bytemuck::cast_slice(&bytes[self.pixel_bytes as usize..]);
        let ms = stamps[1].saturating_sub(stamps[0]) as f64
            * f64::from(self.queue.get_timestamp_period())
            * 1e-6;
        drop(bytes);
        self.readback.unmap();
        Ok((pixels, ms))
    }
}

struct Display {
    window: std::sync::Arc<winit::window::Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
}
impl Display {
    fn new(
        window: std::sync::Arc<winit::window::Window>,
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        view: &wgpu::Buffer,
        hits: &wgpu::Buffer,
    ) -> Result<Self> {
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface incompatible with selected GPU")?;
        let caps = surface.get_capabilities(adapter);
        if let Some(format) = caps.formats.iter().find(|f| f.is_srgb()) {
            config.format = *format;
        }
        surface.configure(device, &config);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("exact ground display"),
            source: wgpu::ShaderSource::Wgsl(include_str!("display.wgsl").into()),
        });
        let constants = std::collections::HashMap::from([(
            "SRGB_TARGET".into(),
            f64::from(config.format.is_srgb()),
        )]);
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present exact ground"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vert"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("shade"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(config.format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("exact ground display data"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: hits.as_entire_binding(),
                },
            ],
        });
        Ok(Self {
            window,
            surface,
            config,
            pipeline,
            group,
        })
    }

    fn present(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<()> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        if (self.config.width, self.config.height) != (size.width, size.height) {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(device, &self.config);
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        let view = frame.texture.create_view(&Default::default());
        let mut cmd = device.create_command_encoder(&Default::default());
        {
            let mut pass = cmd.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([cmd.finish()]);
        frame.present();
        Ok(())
    }
}
