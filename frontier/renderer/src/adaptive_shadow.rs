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
    pub near: NearShadowResources,
    pipelines: [wgpu::RenderPipeline; 6],
    uniform: wgpu::Buffer,
    zero_table: wgpu::Buffer,
    binds: Vec<wgpu::BindGroup>,
    empty_bind: Option<wgpu::BindGroup>,
    keys: Vec<TileKey>,
    near_keys: Vec<TileKey>,
    near_terrain_uniform: wgpu::Buffer,
    near_binds: Vec<wgpu::BindGroup>,
    near_empty_bind: Option<wgpu::BindGroup>,
    far_depth_uniform: wgpu::Buffer,
    far_depth_bind: wgpu::BindGroup,
    cached: Option<ShadowCacheKey>,
    redraw: bool,
    cascades: bool,
    cache_enabled: bool,
}
#[derive(PartialEq, Eq)]
struct ShadowCacheKey {
    revision: u64,
    center: [u64; 3],
    sun: [u32; 3],
    keys: Vec<TileKey>,
}
/// Exact draw inputs consumed by the plant depth shader. Texture and uniform
/// resource identity are scoped by the owning NearShadowResources instance;
/// replacing that resource necessarily creates an empty cache.
#[derive(PartialEq, Eq)]
struct NearShadowCacheKey {
    projection: [u32; 20],
    eye: [u32; 3],
    paint_origin: [u32; 3],
    casters: Vec<(crate::vegetation::MeshKey, Vec<u8>)>,
}
fn reuse_near_depth(cache: &mut Option<NearShadowCacheKey>, next: NearShadowCacheKey) -> bool {
    if cache.as_ref() == Some(&next) {
        return true;
    }
    *cache = Some(next);
    false
}
pub(crate) fn shadow_cascades_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FRONTIER_SHADOW_CASCADES").as_deref() == Ok("1"))
}
pub(crate) fn sampling_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
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
    pub(super) fn allocated_bytes(&self) -> u64 {
        SIZE as u64 * SIZE as u64 * 4
            + CAMERA_UNIFORM_BYTES
            + near_size() as u64 * near_size() as u64 * 4
            + if self.cascades {
                CAMERA_UNIFORM_BYTES
            } else {
                UNIFORM_STRIDE
            }
            + 240 // near/far sampling transforms and empty profile table
    }
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
        let near_terrain_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("near terrain shadow transforms"),
            size: if shadow_cascades_enabled() {
                CAMERA_UNIFORM_BYTES
            } else {
                UNIFORM_STRIDE
            },
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let far_depth_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("far vegetation shadow transform"),
            contents: &[0u8; 80],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let far_depth_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("far vegetation shadow depth"),
            layout: &near_depth_layout(device),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: far_depth_uniform.as_entire_binding(),
            }],
        });
        Self {
            near: NearShadowResources::new(device),
            zero_table,
            _texture: texture,
            view,
            sampling_bind,
            pipelines,
            uniform,
            binds: vec![],
            empty_bind: None,
            keys: vec![],
            near_keys: vec![],
            near_terrain_uniform,
            near_binds: vec![],
            near_empty_bind: None,
            far_depth_uniform,
            far_depth_bind,
            cached: None,
            redraw: true,
            cascades: shadow_cascades_enabled(),
            cache_enabled: std::env::var("FRONTIER_SHADOW_CACHE").as_deref() != Ok("off"),
        }
    }
}
// Camera-relative matrix keeps double-precision world coordinates out of WGSL.
// Snap in light-space texels; translations smaller than a map texel remain stable.
pub(super) struct LightFrame {
    sun: Vec3,
    up: Vec3,
    center: DVec3,
    projection: Mat4,
}
impl LightFrame {
    pub(super) fn new(camera: &Camera, lighting: [f32; 12]) -> Self {
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
        if shadow_cascades_enabled() {
            center = camera.position;
        }
        let texel = (HALF_EXTENT * 2. / SIZE as f32) as f64;
        for axis in [right.as_dvec3(), light_up.as_dvec3()] {
            let distance = center.dot(axis);
            center += axis * ((distance / texel).round() * texel - distance);
        }
        Self {
            sun,
            up,
            center,
            projection: Mat4::orthographic_rh(
                -HALF_EXTENT,
                HALF_EXTENT,
                -HALF_EXTENT,
                HALF_EXTENT,
                1.,
                12000.,
            ),
        }
    }
    pub(super) fn matrix(&self, origin: DVec3) -> Mat4 {
        let center = (self.center - origin).as_vec3();
        self.projection * Mat4::look_at_rh(center + self.sun * 6000., center, self.up)
    }
}
pub(super) fn light_matrix(camera: &Camera, lighting: [f32; 12], origin: DVec3) -> Mat4 {
    LightFrame::new(camera, lighting).matrix(origin)
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
    pub(crate) fn set_naadf_mask(
        &self,
        queue: &wgpu::Queue,
        bounds: Option<[[f64; 3]; 2]>,
        eye: [f64; 3],
    ) {
        let mut values = [0f32; 8];
        if let Some(b) = bounds {
            for axis in 0..3 {
                values[axis] = (b[0][axis] - eye[axis]) as f32;
                values[4 + axis] = (b[1][axis] - eye[axis]) as f32;
            }
            values[7] = 1.;
        }
        queue.write_buffer(
            &self.shadows.near.uniform,
            80,
            bytemuck::cast_slice(&values),
        );
    }
    pub(super) fn prepare_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &Camera,
        owners: &HashSet<TileKey>,
    ) {
        self.shadows.keys.clear();
        let light_frame = LightFrame::new(camera, self.lighting);
        let matrix = near_light_matrix(camera, self.lighting);
        let mut near_uniform = [0f32; 20];
        near_uniform[..16].copy_from_slice(&matrix.to_cols_array());
        near_uniform[16] = if self.near_shadows_active() { 1. } else { 0. };
        self.shadows.near.uniform_key = near_uniform.map(f32::to_bits);
        queue.write_buffer(
            &self.shadows.near.uniform,
            0,
            bytemuck::cast_slice(&near_uniform),
        );
        if self.lighting[5] < 0.5 {
            self.shadows.cached = None;
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
                        intersects_light_bounds(light_frame.matrix(c.origin), c.bounds)
                    })
                })
                .copied(),
        );
        self.shadows.keys.sort();
        let cache_key = ShadowCacheKey {
            revision: self.residency_revision,
            center: light_frame.center.to_array().map(f64::to_bits),
            sun: light_frame.sun.to_array().map(f32::to_bits),
            keys: self.shadows.keys.clone(),
        };
        // Cascades receive changing plant depths after terrain: rebuild instead
        // of retaining those depths after a caster moves or is replaced.
        self.shadows.redraw = !self.shadows.cache_enabled
            || self.shadows.cascades
            || self.shadows.cached.as_ref() != Some(&cache_key);
        self.shadows.cached = Some(cache_key);
        if self.shadows.cascades {
            let mut far_uniform = [0f32; 20];
            far_uniform[..16].copy_from_slice(&light_frame.matrix(camera.position).to_cols_array());
            far_uniform[16] = 1.;
            queue.write_buffer(
                &self.shadows.far_depth_uniform,
                0,
                bytemuck::cast_slice(&far_uniform),
            );
            self.shadows.near_keys = owners
                .iter()
                .copied()
                .filter(|key| {
                    self.clusters.get(key).is_some_and(|c| {
                        intersects_light_bounds(
                            matrix * Mat4::from_translation((c.origin - camera.position).as_vec3()),
                            c.bounds,
                        )
                    })
                })
                .collect();
            self.shadows.near_keys.sort();
            while self.shadows.near_binds.len() < self.profile_tables.len() {
                let i = self.shadows.near_binds.len();
                self.shadows.near_binds.push(profile_bind(
                    device,
                    &self.layout,
                    &self.shadows.near_terrain_uniform,
                    &self.profile_tables[i].buffer,
                ));
            }
            if self.shadows.near_empty_bind.is_none() {
                self.shadows.near_empty_bind = Some(profile_bind(
                    device,
                    &self.layout,
                    &self.shadows.near_terrain_uniform,
                    &self.shadows.zero_table,
                ));
            }
            let mut near_bytes = vec![0u8; self.shadows.near_keys.len() * UNIFORM_STRIDE as usize];
            for (i, key) in self.shadows.near_keys.iter().enumerate() {
                let c = &self.clusters[key];
                let transform =
                    matrix * Mat4::from_translation((c.origin - camera.position).as_vec3());
                near_bytes[i * UNIFORM_STRIDE as usize..i * UNIFORM_STRIDE as usize + 64]
                    .copy_from_slice(bytemuck::cast_slice(&transform.to_cols_array()));
            }
            if !near_bytes.is_empty() {
                queue.write_buffer(&self.shadows.near_terrain_uniform, 0, &near_bytes);
            }
        }
        if !self.shadows.redraw {
            return;
        }
        let mut bytes = vec![0u8; self.shadows.keys.len() * UNIFORM_STRIDE as usize];
        for (i, key) in self.shadows.keys.iter().enumerate() {
            let c = &self.clusters[key];
            let matrix = light_frame.matrix(c.origin).to_cols_array();
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
        if self.shadows.redraw {
            self.draw_shadow_map(encoder, false);
        }
        if self.shadows.cascades && self.near_shadows_active() {
            self.draw_shadow_map(encoder, true);
        }
    }
    fn draw_shadow_map(&self, encoder: &mut wgpu::CommandEncoder, near: bool) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bounded Alpine terrain shadow depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: if near {
                    &self.shadows.near.view
                } else {
                    &self.shadows.view
                },
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let keys = if near {
            &self.shadows.near_keys
        } else {
            &self.shadows.keys
        };
        for (i, key) in keys.iter().enumerate() {
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
                || {
                    if near {
                        self.shadows.near_empty_bind.as_ref().unwrap()
                    } else {
                        self.shadows.empty_bind.as_ref().unwrap()
                    }
                },
                |p| {
                    if near {
                        &self.shadows.near_binds[p.table.page]
                    } else {
                        &self.shadows.binds[p.table.page]
                    }
                },
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

// Independent small map: 9.375cm texels retain tree branch/contact shadows that
// the 1m far terrain map cannot represent. Only already streamed plants cast.
fn near_size() -> u32 {
    static SIZE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        if std::env::var("FRONTIER_BEAUTY_REFERENCE").is_ok_and(|v| v == "1") {
            2048
        } else {
            1024
        }
    })
}
pub(crate) fn near_sampling_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("close plant shadow sampling"),
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
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(112),
                },
                count: None,
            },
        ],
    })
}
pub(super) struct NearShadowResources {
    _texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind: wgpu::BindGroup,
    pub depth_bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    uniform_key: [u32; 20],
    cached: std::cell::RefCell<Option<NearShadowCacheKey>>,
    reused: std::cell::Cell<bool>,
}
impl NearShadowResources {
    fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("close plant shadows (4 MiB; reference 16 MiB)"),
            size: wgpu::Extent3d {
                width: near_size(),
                height: near_size(),
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
            label: Some("close plant shadow PCF"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("eye-relative close plant shadow transform"),
            contents: &[0u8; 112],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("close plant shadows"),
            layout: &near_sampling_layout(device),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let depth_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("plant shadow depth uniform"),
            layout: &near_depth_layout(device),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        Self {
            _texture: texture,
            view,
            bind,
            depth_bind,
            uniform,
            uniform_key: [0; 20],
            cached: std::cell::RefCell::new(None),
            reused: std::cell::Cell::new(false),
        }
    }
}
// Construct orientation before subtracting the floating origin. Snapping absolute
// double-precision light coordinates avoids visible crawling during origin shifts.
fn near_light_matrix(camera: &Camera, lighting: [f32; 12]) -> Mat4 {
    let sun = Vec3::new(lighting[0], lighting[1], lighting[2]).normalize_or_zero();
    let sun = if sun.length_squared() > 0.5 {
        sun
    } else {
        Vec3::Y
    };
    let up = if sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let right = up.cross(sun).normalize();
    let light_up = sun.cross(right).normalize();
    let mut center = camera.position;
    let texel = 96. / near_size() as f64;
    for axis in [right.as_dvec3(), light_up.as_dvec3()] {
        let distance = center.dot(axis);
        center += axis * ((distance / texel).round() * texel - distance);
    }
    let center = (center - camera.position).as_vec3();
    Mat4::orthographic_rh(-48., 48., -48., 48., 1., 256.)
        * Mat4::look_at_rh(center + sun * 128., center, up)
}
impl AdaptiveGpu {
    /// Published terrain depth uses the same bounded owner set as surface shadows.
    pub(crate) fn far_shadow_bind(&self) -> &wgpu::BindGroup {
        &self.shadows.sampling_bind
    }
    pub(crate) fn far_shadow_view(&self) -> &wgpu::TextureView {
        &self.shadows.view
    }
    pub(crate) fn far_shadow_depth_bind(&self) -> &wgpu::BindGroup {
        &self.shadows.far_depth_bind
    }
    pub(crate) fn shadow_cascades_active(&self) -> bool {
        self.shadows.cascades && self.near_shadows_active()
    }
    pub(crate) fn shadow_cache_reused(&self) -> bool {
        self.shadows_active() && !self.shadows.redraw
    }
    /// Called immediately before the near vegetation pass. An unchanged map
    /// retains its existing depth attachment; cascades always rebuild because
    /// terrain has already cleared and populated the same attachment this frame.
    pub(crate) fn reuse_near_plant_shadow(
        &self,
        camera: &Camera,
        origin: DVec3,
        casters: Vec<(crate::vegetation::MeshKey, Vec<u8>)>,
    ) -> bool {
        if self.shadows.cascades || !self.shadows.cache_enabled {
            self.shadows.near.cached.borrow_mut().take();
            self.shadows.near.reused.set(false);
            return false;
        }
        let next = NearShadowCacheKey {
            projection: self.shadows.near.uniform_key,
            eye: (camera.position - origin)
                .as_vec3()
                .to_array()
                .map(f32::to_bits),
            paint_origin: origin.to_array().map(|v| (v * 16.) as i64 as u32),
            casters,
        };
        let reused = reuse_near_depth(&mut self.shadows.near.cached.borrow_mut(), next);
        self.shadows.near.reused.set(reused);
        reused
    }
    pub(crate) fn near_shadow_cache_reused(&self) -> bool {
        self.near_shadows_active() && self.shadows.near.reused.get()
    }
    /// Eye-relative input for the atmosphere pass; terrain's draw uniforms instead
    /// use each cluster origin. Both map to the identical world-snapped light map.
    pub(crate) fn far_shadow_projection(&self, camera: &Camera) -> Mat4 {
        light_matrix(camera, self.lighting, camera.position)
    }
    pub(crate) fn near_shadow_bind(&self) -> &wgpu::BindGroup {
        &self.shadows.near.bind
    }
    pub(crate) fn near_shadow_view(&self) -> &wgpu::TextureView {
        &self.shadows.near.view
    }
    pub(crate) fn near_shadows_active(&self) -> bool {
        self.near_beauty && self.shadows_active()
    }
}

pub(crate) fn near_depth_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("plant shadow depth uniform"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(80),
            },
            count: None,
        }],
    })
}
impl AdaptiveGpu {
    pub(crate) fn near_shadow_depth_bind(&self) -> &wgpu::BindGroup {
        &self.shadows.near.depth_bind
    }
}

#[cfg(test)]
mod near_tests {
    use super::*;
    #[test]
    fn near_depth_reuse_requires_exact_shader_inputs_and_caster_bytes() {
        let key = || NearShadowCacheKey {
            projection: [0; 20],
            eye: [0; 3],
            paint_origin: [0; 3],
            casters: vec![(
                crate::vegetation::MeshKey {
                    species: 2,
                    variant: 0,
                    seed: 9,
                    tile: [0; 2],
                    lod: 4,
                    edit_id: 0,
                },
                vec![0; 32],
            )],
        };
        let mut cache = None;
        assert!(!reuse_near_depth(&mut cache, key()));
        assert!(reuse_near_depth(&mut cache, key()));
        // Eye affects ground distance fades independently of the snapped map.
        let mut eye = key();
        eye.eye[0] = 1;
        assert!(!reuse_near_depth(&mut cache, eye));
        let mut projection = key();
        projection.projection[5] = 1;
        assert!(!reuse_near_depth(&mut cache, projection));
        let mut origin = key();
        origin.paint_origin[1] = 1;
        assert!(!reuse_near_depth(&mut cache, origin));
        let mut phase = key();
        phase.casters[0].1[31] = 1;
        assert!(!reuse_near_depth(&mut cache, phase));
        let mut lod = key();
        lod.casters[0].0.lod = 2;
        assert!(!reuse_near_depth(&mut cache, lod));
        let mut edited = key();
        edited.casters[0].0.edit_id = 1;
        assert!(!reuse_near_depth(&mut cache, edited));
        let mut removed = key();
        removed.casters.clear();
        assert!(!reuse_near_depth(&mut cache, removed));
        // Replacing the owning texture/map starts with an empty cache.
        cache = None;
        assert!(!reuse_near_depth(&mut cache, key()));
    }
    #[test]
    fn cached_depth_requires_same_world_light_and_ownership_revisions() {
        let key = || ShadowCacheKey {
            revision: 7,
            center: [0; 3],
            sun: [0, 1, 0],
            keys: vec![TileKey {
                min: [0; 2],
                width_cells: 512,
                step_cells: 8,
                edge_steps: [8; 4],
                source_revision: 2,
            }],
        };
        let original = key();
        assert!(original == key());
        let mut edited = key();
        edited.revision += 1;
        assert!(original != edited);
        let mut source = key();
        source.keys[0].source_revision += 1;
        assert!(original != source);
        let mut light = key();
        light.sun[0] = 1;
        assert!(original != light);
        let mut camera = key();
        camera.center[0] = 1;
        assert!(original != camera);
        let mut ownership = key();
        ownership.keys.clear();
        assert!(original != ownership);
    }
    #[test]
    fn atmosphere_and_cluster_shadow_coordinates_agree() {
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(1e8, 42., -1e8);
        let origin = camera.position + DVec3::new(32., -8., 64.);
        let world_point = camera.position + DVec3::new(7., 3., 22.);
        let lighting = [0.4, 0.8, 0.3, 1., 0., 1., 0., 0., 0., 0., 0., 1.];
        let eye_relative = light_matrix(&camera, lighting, camera.position)
            .transform_point3((world_point - camera.position).as_vec3());
        let cluster_relative = light_matrix(&camera, lighting, origin)
            .transform_point3((world_point - origin).as_vec3());
        assert!((eye_relative - cluster_relative).abs().max_element() < 1e-5);
    }
    #[test]
    fn near_shadow_depth_decreases_toward_the_sun() {
        let camera = Camera::fixture();
        let sun = Vec3::new(0.4, 0.8, 0.3).normalize();
        let mut lighting = [0.; 12];
        lighting[..3].copy_from_slice(&sun.to_array());
        let matrix = near_light_matrix(&camera, lighting);
        let receiver = matrix.transform_point3(Vec3::ZERO);
        let caster = matrix.transform_point3(sun * 4.);
        assert!(caster.z < receiver.z);
        assert!((caster.x - receiver.x).abs() < 1e-6);
        assert!((caster.y - receiver.y).abs() < 1e-6);
    }
    #[test]
    fn near_light_projection_is_stable_in_world_texels() {
        let mut camera = Camera::fixture();
        camera.position = DVec3::ZERO;
        let mut lighting = [0.; 12];
        lighting[0] = 0.4;
        lighting[1] = 0.8;
        lighting[2] = 0.3;
        let point = DVec3::new(3., 2., 5.);
        let a = near_light_matrix(&camera, lighting).transform_point3(point.as_vec3());
        camera.position += DVec3::new(0.001, 0.001, 0.001);
        let b = near_light_matrix(&camera, lighting)
            .transform_point3((point - camera.position).as_vec3());
        assert!((a.x - b.x).abs() < 0.000001);
        assert!((a.y - b.y).abs() < 0.000001);
    }
    #[test]
    fn near_light_projection_keeps_large_world_origins_out_of_gpu_math() {
        let mut camera = Camera::fixture();
        camera.position = DVec3::new(1e10, -1e10, 1e10);
        let mut lighting = [0.; 12];
        lighting[1] = 1.;
        let matrix = near_light_matrix(&camera, lighting);
        assert!(matrix.to_cols_array().into_iter().all(f32::is_finite));
        let p = matrix.transform_point3(Vec3::ZERO);
        assert!(p.x.abs() < 0.002 && p.y.abs() < 0.002);
        assert!(p.z > 0. && p.z < 1.);
    }
}
