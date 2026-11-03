//! Persistent GPU resources; asynchronous timing readback, no per-frame mesh work.
use super::{scene::Scene, traversal};
use bytemuck::{Pod, Zeroable};
use std::sync::mpsc;
use wgpu::util::DeviceExt;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Frame {
    pub eye: [f32; 4],
    pub forward: [f32; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    pub grid: [u32; 4],
    pub scene: [f32; 4],
    pub options: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Temporal {
    previous: Frame,
    jitter: [f32; 4],
    state: [f32; 4],
    tuning: [f32; 4],
}
struct Slot {
    buffer: wgpu::Buffer,
    pending: bool,
}
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    pub format: wgpu::TextureFormat,
    compute: wgpu::ComputePipeline,
    present: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    coeff: wgpu::Buffer,
    coeff_tail: wgpu::Buffer,
    air: wgpu::Buffer,
    pub traversal_mode: u32,
    pub output: wgpu::Texture,
    bind: [wgpu::BindGroup; 2],
    history: [wgpu::Texture; 2],
    temporal: wgpu::Buffer,
    previous: Option<Frame>,
    previous_jitter: [f32; 2],
    previous_aa: bool,
    previous_strengths: [f32; 4],
    pub aa_enabled: bool,
    pub temporal_strength: f32,
    pub face_smoothing: f32,
    pub spatial_samples: u32,
    screen: wgpu::BindGroup,
    probes: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    pub coefficient_bytes: u64,
    queries: Option<wgpu::QuerySet>,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    tx: mpsc::Sender<(usize, bool)>,
    rx: mpsc::Receiver<(usize, bool)>,
    period: f64,
    pub compute_ms: Option<f64>,
    pub present_ms: Option<f64>,
    pub average_patches: f64,
    pub average_quads: f64,
    pub frames: u64,
    pub cpu_encode_ms: f64,
}
impl Gpu {
    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        scene: &Scene,
        w: u32,
        h: u32,
    ) -> Result<Self, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface,
            })
            .await
            .ok_or("No GPU adapter")?;
        let info = adapter.get_info();
        let name = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);
        println!("GPU: {name}");
        println!(
            "Hardware ray-query flags: {:?}; procedural AABB backend unavailable in wgpu 24",
            adapter.features()
                & (wgpu::Features::EXPERIMENTAL_RAY_QUERY
                    | wgpu::Features::EXPERIMENTAL_RAY_TRACING_ACCELERATION_STRUCTURE)
        );
        let timestamps = adapter.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        );
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("analytic renderer"),
                    required_features: if timestamps {
                        wgpu::Features::TIMESTAMP_QUERY
                            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
                    } else {
                        wgpu::Features::empty()
                    },
                    required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
        let format = if let Some(surface) = surface {
            let caps = surface.get_capabilities(&adapter);
            caps.formats
                .iter()
                .copied()
                .find(|f| {
                    *f == wgpu::TextureFormat::Bgra8Unorm || *f == wgpu::TextureFormat::Rgba8Unorm
                })
                .ok_or("GPU surface needs an unorm presentation format")?
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader_source = include_str!("raycast.wgsl").replacen(
            "const USE_SPLINE:bool=false;",
            &format!("const USE_SPLINE:bool={};", scene.is_spline()),
            1,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("analytic rays"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("analytic intersections"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let blit = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("presentation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("present.wgsl").into()),
        });
        let present = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present image"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &blit,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        if let Some(e) = device.pop_error_scope().await {
            return Err(format!("GPU pipeline: {e}"));
        }
        let packed = scene.packed();
        let bytes = bytemuck::cast_slice(&packed);
        let page = device.limits().max_storage_buffer_binding_size as usize / 48 * 48;
        if bytes.len() > page * 2 {
            return Err("Scene exceeds two GPU coefficient pages".into());
        }
        let split = bytes.len().min(page);
        let make_page = |label, contents| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let coeff = make_page("quadrics page 0", &bytes[..split]);
        let dummy = [0u8; 48];
        let coeff_tail = make_page(
            "quadrics page 1",
            if split < bytes.len() {
                &bytes[split..]
            } else {
                &dummy
            },
        );
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<Frame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let output = Self::texture(&device, w, h);
        let probes = Self::probe_buffer(&device, w, h);
        let temporal = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("temporal camera"),
            size: std::mem::size_of::<Temporal>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let hierarchy = traversal::build(scene);
        let air = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("conservative air hierarchy"),
            contents: bytemuck::cast_slice(&hierarchy),
            usage: wgpu::BufferUsages::STORAGE,
        });
        println!(
            "Air hierarchy: {} levels, {} bytes, {} / {} base patches empty",
            hierarchy[36],
            hierarchy.len() * 4,
            hierarchy[40..40 + scene.patches_per_axis.iter().product::<usize>()]
                .iter()
                .filter(|&&v| v != 0)
                .count(),
            scene.patches_per_axis.iter().product::<usize>()
        );
        let history = [
            Self::history_texture(&device, w, h),
            Self::history_texture(&device, w, h),
        ];
        let bind = std::array::from_fn(|i| {
            Self::bind(
                &device,
                &compute,
                &uniform,
                &coeff,
                &coeff_tail,
                &output,
                &probes,
                &temporal,
                &history[1 - i],
                &history[i],
                &air,
            )
        });
        let screen = Self::screen(&device, &present, &output);
        let queries = timestamps.then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("pass timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 3,
            })
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resolved timestamps"),
            size: 32,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = (0..3)
            .map(|_| Self::slot(&device, probes.size() + 32))
            .collect();
        let (tx, rx) = mpsc::channel();
        let period = queue.get_timestamp_period() as f64;
        println!(
            "Coefficient upload: {} B (f32); timestamps {}; sparse probe buffer {} B",
            bytes.len(),
            timestamps,
            probes.size()
        );
        Ok(Self {
            device,
            queue,
            adapter_name: name,
            format,
            compute,
            present,
            uniform,
            coeff,
            coeff_tail,
            air,
            traversal_mode: 0,
            output,
            bind,
            history,
            temporal,
            previous: None,
            previous_jitter: [0.; 2],
            previous_aa: false,
            previous_strengths: [-1.; 4],
            aa_enabled: true,
            temporal_strength: 0.,
            face_smoothing: 0.,
            spatial_samples: 1,
            screen,
            probes,
            width: w,
            height: h,
            coefficient_bytes: bytes.len() as u64,
            queries,
            resolve,
            slots,
            tx,
            rx,
            period,
            compute_ms: None,
            present_ms: None,
            average_patches: 0.,
            average_quads: 0.,
            frames: 0,
            cpu_encode_ms: 0.,
        })
    }
    fn history_texture(d: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
        d.create_texture(&wgpu::TextureDescriptor {
            label: Some("linear history and depth"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }
    fn texture(d: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
        d.create_texture(&wgpu::TextureDescriptor {
            label: Some("analytic image"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }
    fn probe_buffer(d: &wgpu::Device, w: u32, h: u32) -> wgpu::Buffer {
        d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparse diagnostic rays"),
            size: (w.div_ceil(32) * h.div_ceil(32)) as u64 * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }
    fn slot(d: &wgpu::Device, size: u64) -> Slot {
        Slot {
            buffer: d.create_buffer(&wgpu::BufferDescriptor {
                label: Some("async diagnostics readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            pending: false,
        }
    }
    fn bind(
        d: &wgpu::Device,
        p: &wgpu::ComputePipeline,
        u: &wgpu::Buffer,
        c: &wgpu::Buffer,
        tail: &wgpu::Buffer,
        t: &wgpu::Texture,
        probes: &wgpu::Buffer,
        temporal: &wgpu::Buffer,
        history: &wgpu::Texture,
        next: &wgpu::Texture,
        air: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("analytic resources"),
            layout: &p.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: air.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: temporal.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(
                        &history.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(
                        &next.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: u.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: c.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &t.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: tail.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: probes.as_entire_binding(),
                },
            ],
        })
    }
    fn screen(d: &wgpu::Device, p: &wgpu::RenderPipeline, t: &wgpu::Texture) -> wgpu::BindGroup {
        d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image presentation"),
            layout: &p.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&t.create_view(&Default::default())),
            }],
        })
    }
    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        // Rare resize only: drain pending mappings before replacing diagnostic resources.
        self.device.poll(wgpu::Maintain::Wait);
        self.poll();
        self.width = w;
        self.height = h;
        self.output = Self::texture(&self.device, w, h);
        self.probes = Self::probe_buffer(&self.device, w, h);
        self.history = [
            Self::history_texture(&self.device, w, h),
            Self::history_texture(&self.device, w, h),
        ];
        self.previous = None;
        self.bind = std::array::from_fn(|i| {
            Self::bind(
                &self.device,
                &self.compute,
                &self.uniform,
                &self.coeff,
                &self.coeff_tail,
                &self.output,
                &self.probes,
                &self.temporal,
                &self.history[1 - i],
                &self.history[i],
                &self.air,
            )
        });
        self.screen = Self::screen(&self.device, &self.present, &self.output);
        self.slots = (0..3)
            .map(|_| Self::slot(&self.device, self.probes.size() + 32))
            .collect();
    }
    pub fn poll(&mut self) {
        self.device.poll(wgpu::Maintain::Poll);
        while let Ok((i, ok)) = self.rx.try_recv() {
            let slot = &mut self.slots[i];
            slot.pending = false;
            if !ok {
                continue;
            }
            let data = slot.buffer.slice(..).get_mapped_range();
            if self.queries.is_some() {
                let t: Vec<u64> = data[..24]
                    .chunks_exact(8)
                    .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                self.compute_ms = Some(t[1].saturating_sub(t[0]) as f64 * self.period / 1e6);
                self.present_ms = Some(t[2].saturating_sub(t[1]) as f64 * self.period / 1e6);
            }
            let values: Vec<f32> = data[32..]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            let count = values.len() / 4;
            self.average_patches =
                values.chunks_exact(4).map(|p| p[1] as f64).sum::<f64>() / count as f64;
            self.average_quads =
                values.chunks_exact(4).map(|p| p[2] as f64).sum::<f64>() / count as f64;
            drop(data);
            slot.buffer.unmap();
        }
    }
    pub fn draw(&mut self, frame: &Frame, target: Option<&wgpu::TextureView>) {
        self.poll();
        let start = std::time::Instant::now();
        self.queue
            .write_buffer(&self.uniform, 0, bytemuck::bytes_of(frame));
        let enabled = self.aa_enabled
            && self.temporal_strength > 0.
            && (frame.options[2] == 0 || frame.options[2] == 4);
        let jitter = if enabled {
            jitter_sample(self.frames)
        } else {
            [0.; 2]
        };
        let previous = self.previous.unwrap_or(*frame);
        let distance = (0..3)
            .map(|i| (frame.eye[i] - previous.eye[i]).powi(2))
            .sum::<f32>();
        let valid = enabled
            && self.previous_aa
            && self.previous.is_some()
            && previous.options[2] == frame.options[2]
            && distance < 25.;
        let strengths = [
            self.temporal_strength,
            self.face_smoothing,
            self.spatial_samples as f32,
            self.traversal_mode as f32,
        ];
        // Slider edits immediately discard the old appearance.
        let valid = valid && self.previous_strengths == strengths;
        self.previous_strengths = strengths;
        let temporal = Temporal {
            tuning: [
                self.temporal_strength,
                self.face_smoothing,
                self.spatial_samples as f32,
                self.traversal_mode as f32,
            ],
            previous,
            jitter: [
                jitter[0],
                jitter[1],
                self.previous_jitter[0],
                self.previous_jitter[1],
            ],
            state: [
                if valid { 1. } else { 0. },
                if enabled { 1. } else { 0. },
                if self.aa_enabled { 1. } else { 0. },
                0.,
            ],
        };
        self.queue
            .write_buffer(&self.temporal, 0, bytemuck::bytes_of(&temporal));
        self.previous = Some(*frame);
        self.previous_jitter = jitter;
        self.previous_aa = enabled;
        let slot = if self.frames % 30 == 0 {
            self.slots.iter().position(|s| !s.pending)
        } else {
            None
        };
        let mut e = self.device.create_command_encoder(&Default::default());
        if let (Some(_), Some(q)) = (slot, &self.queries) {
            e.write_timestamp(q, 0);
        }
        {
            let mut pass = e.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("traverse + intersect + shade"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute);
            pass.set_bind_group(0, &self.bind[(self.frames % 2) as usize], &[]);
            pass.dispatch_workgroups(self.width.div_ceil(8), self.height.div_ceil(8), 1);
        }
        if let (Some(_), Some(q)) = (slot, &self.queries) {
            e.write_timestamp(q, 1);
        }
        if let Some(target) = target {
            let mut pass = e.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.present);
            pass.set_bind_group(0, &self.screen, &[]);
            pass.draw(0..3, 0..1);
        }
        if let Some(i) = slot {
            if let Some(q) = &self.queries {
                e.write_timestamp(q, 2);
                e.resolve_query_set(q, 0..3, &self.resolve, 0);
                e.copy_buffer_to_buffer(&self.resolve, 0, &self.slots[i].buffer, 0, 24);
            }
            e.copy_buffer_to_buffer(
                &self.probes,
                0,
                &self.slots[i].buffer,
                32,
                self.probes.size(),
            );
        }
        self.queue.submit(Some(e.finish()));
        if let Some(i) = slot {
            self.slots[i].pending = true;
            let tx = self.tx.clone();
            self.slots[i]
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send((i, r.is_ok()));
                });
        }
        self.frames += 1;
        self.cpu_encode_ms = start.elapsed().as_secs_f64() * 1000.;
    }
    pub fn read_probes(&self) -> Result<Vec<[f32; 4]>, String> {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe verification"),
            size: self.probes.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e = self.device.create_command_encoder(&Default::default());
        e.copy_buffer_to_buffer(&self.probes, 0, &buffer, 0, self.probes.size());
        self.queue.submit(Some(e.finish()));
        let (tx, rx) = mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let data = buffer.slice(..).get_mapped_range();
        Ok(data
            .chunks_exact(16)
            .map(|b| {
                std::array::from_fn(|i| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
            })
            .collect())
    }
    pub fn capture(&self, path: &std::path::Path) -> Result<(), String> {
        let padded = (self.width * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PNG readback"),
            size: padded as u64 * self.height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e = self.device.create_command_encoder(&Default::default());
        e.copy_texture_to_buffer(
            self.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            self.output.size(),
        );
        self.queue.submit(Some(e.finish()));
        let (tx, rx) = mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let data = buffer.slice(..).get_mapped_range();
        let pixels: Vec<u8> = data
            .chunks_exact(padded as usize)
            .flat_map(|r| r[..self.width as usize * 4].iter().copied())
            .collect();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut png = png::Encoder::new(
            std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?),
            self.width,
            self.height,
        );
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()
            .map_err(|e| e.to_string())?
            .write_image_data(&pixels)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// 16-frame low-discrepancy pixel jitter, shared by all rays in a frame.
fn jitter_sample(frame: u64) -> [f32; 2] {
    fn radical(mut n: u32, base: u32) -> f32 {
        let mut r = 0.;
        let mut f = 1. / base as f32;
        while n > 0 {
            r += (n % base) as f32 * f;
            n /= base;
            f /= base as f32;
        }
        r
    }
    let n = (frame % 16) as u32 + 1;
    [radical(n, 2) - 0.5, radical(n, 3) - 0.5]
}

#[cfg(test)]
mod temporal_tests {
    use super::*;
    #[test]
    fn jitter_stays_inside_pixel_and_repeats_deterministically() {
        for i in 0..16 {
            let j = jitter_sample(i);
            assert!(j.iter().all(|v| *v >= -0.5 && *v < 0.5));
            assert_eq!(j, jitter_sample(i + 16));
        }
        assert_ne!(jitter_sample(0), jitter_sample(1));
    }
}
