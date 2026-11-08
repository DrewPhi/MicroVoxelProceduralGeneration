//! Bounded Alpine directional shadows over the current published terrain owners.
//! Independent of eye visibility, native facing bins and occlusion rejection.
//! No extra terrain is generated: unavailable/off-map casters and fixed volume
//! owners are outside this presentation experiment's shadow coverage.
use super::*;
use glam::Mat4;
const SIZE: u32 = 2048;
const HALF_EXTENT: f32 = 1024.;
pub(super) struct ShadowResources {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    pub sampling_bind: wgpu::BindGroup,
    pipelines: [wgpu::RenderPipeline; 6],
    uniform: wgpu::Buffer,
    zero_table: wgpu::Buffer,
    binds: Vec<wgpu::BindGroup>,
    empty_bind: Option<wgpu::BindGroup>,
    keys: Vec<TileKey>,
}
pub(super) fn sampling_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bounded Alpine shadow sampling"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    })
}
impl ShadowResources {
    pub(super) fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        pipelines: [wgpu::RenderPipeline; 6],
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bounded Alpine shadow map (16 MiB)"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Alpine shadow PCF"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sampling_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Alpine shadow map"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Alpine shadow caster transforms"),
            size: CAMERA_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let zero_table = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("empty shadow profile"),
            contents: &[0u8; 80],
            usage: wgpu::BufferUsages::STORAGE,
        });
        Self {
            zero_table,
            _texture: texture,
            view,
            sampling_bind,
            pipelines,
            uniform,
            binds: vec![],
            empty_bind: None,
            keys: vec![],
        }
    }
}
// Camera-relative matrix keeps double-precision world coordinates out of WGSL.
// Snap in light-space texels; translations smaller than a map texel remain stable.
pub(super) fn light_matrix(camera: &Camera, lighting: [f32; 12], origin: DVec3) -> Mat4 {
    let sun = Vec3::new(lighting[0], lighting[1], lighting[2]).normalize_or_zero();
    let sun = if sun.length_squared() > 0.5 {
        sun
    } else {
        Vec3::Y
    };
    let up = if sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let right = up.cross(sun).normalize();
    let light_up = sun.cross(right).normalize();
    let forward = camera.direction().as_dvec3();
    let mut center =
        camera.position + DVec3::new(forward.x, 0., forward.z).normalize_or_zero() * 300.;
    let texel = (HALF_EXTENT * 2. / SIZE as f32) as f64;
    for axis in [right.as_dvec3(), light_up.as_dvec3()] {
        let distance = center.dot(axis);
        center += axis * ((distance / texel).round() * texel - distance);
    }
    let center = (center - origin).as_vec3();
    let view = Mat4::look_at_rh(center + sun * 6000., center, up);
    Mat4::orthographic_rh(
        -HALF_EXTENT,
        HALF_EXTENT,
        -HALF_EXTENT,
        HALF_EXTENT,
        1.,
        12000.,
    ) * view
}
fn intersects_light_bounds(matrix: Mat4, bounds: [Vec3; 2]) -> bool {
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for corner in 0..8 {
        let p = Vec3::new(
            bounds[(corner & 1) as usize].x,
            bounds[((corner >> 1) & 1) as usize].y,
            bounds[((corner >> 2) & 1) as usize].z,
        );
        let q = matrix.transform_point3(p);
        low = low.min(q);
        high = high.max(q);
    }
    low.x <= 1. && high.x >= -1. && low.y <= 1. && high.y >= -1. && low.z <= 1. && high.z >= 0.
}
impl AdaptiveGpu {
    pub(super) fn prepare_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &Camera,
        owners: &HashSet<TileKey>,
    ) {
        self.shadows.keys.clear();
        if self.lighting[5] < 0.5 {
            return;
        }
        while self.shadows.binds.len() < self.profile_tables.len() {
            let i = self.shadows.binds.len();
            self.shadows.binds.push(profile_bind(
                device,
                &self.layout,
                &self.shadows.uniform,
                &self.profile_tables[i].buffer,
            ));
        }
        if self.shadows.empty_bind.is_none() {
            // Bindings 1/2 are statically unused by generic mesh depth vertices.
            self.shadows.empty_bind = Some(profile_bind(
                device,
                &self.layout,
                &self.shadows.uniform,
                &self.shadows.zero_table,
            ));
        }
        self.shadows.keys.extend(
            owners
                .iter()
                .filter(|k| {
                    self.clusters.get(k).is_some_and(|c| {
                        intersects_light_bounds(
                            light_matrix(camera, self.lighting, c.origin),
                            c.bounds,
                        )
                    })
                })
                .copied(),
        );
        self.shadows.keys.sort();
        let mut bytes = vec![0u8; self.shadows.keys.len() * UNIFORM_STRIDE as usize];
        for (i, key) in self.shadows.keys.iter().enumerate() {
            let c = &self.clusters[key];
            let matrix = light_matrix(camera, self.lighting, c.origin).to_cols_array();
            let start = i * UNIFORM_STRIDE as usize;
            bytes[start..start + 64].copy_from_slice(bytemuck::cast_slice(&matrix));
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.shadows.uniform, 0, &bytes);
        }
    }
    pub(crate) fn draw_shadows(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.lighting[5] < 0.5 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bounded Alpine terrain shadow depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadows.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        for (i, key) in self.shadows.keys.iter().enumerate() {
            let c = &self.clusters[key];
            let kind = if c.native_quads {
                2
            } else if c.derived_proxy {
                5
            } else if c.indexed_proxy {
                4
            } else if c.proxy_triangles {
                3
            } else if c.profile.is_some() {
                1
            } else {
                0
            };
            pass.set_pipeline(&self.shadows.pipelines[kind]);
            let bind = c.profile.as_ref().map_or_else(
                || self.shadows.empty_bind.as_ref().unwrap(),
                |p| &self.shadows.binds[p.table.page],
            );
            pass.set_bind_group(0, bind, &[(i as u64 * UNIFORM_STRIDE) as u32]);
            pass.set_vertex_buffer(
                0,
                self.vertices[c.vertices.page]
                    .buffer
                    .slice(c.vertices.range.clone()),
            );
            if c.native_quads {
                pass.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..6, 0, 0..c.count);
            } else if c.proxy_triangles {
                pass.draw(0..3, 0..c.count);
            } else {
                if let Some(p) = &c.profile {
                    let indices = p.indices.as_ref().unwrap();
                    pass.set_vertex_buffer(
                        1,
                        self.profiles[indices.page]
                            .buffer
                            .slice(indices.range.clone()),
                    );
                } else {
                    pass.set_vertex_buffer(1, self.zero_profile.slice(..));
                }
                let indices = c.indices.as_ref().unwrap();
                pass.set_index_buffer(
                    self.indices[indices.page]
                        .buffer
                        .slice(indices.range.clone()),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..c.count, 0, 0..1);
            }
        }
    }
}
