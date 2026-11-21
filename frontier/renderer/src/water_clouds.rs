//! Directional cloud light for water, sampled from the same volume as the sky.
use super::{cloud_shader_source, make_pipeline_groups, CloudView};
use glam::Vec3;

pub(crate) const WIDTH: u32 = 64;
pub(crate) const HEIGHT: u32 = 32;
pub(crate) const BYTES: u64 = (WIDTH * HEIGHT * 4) as u64;

#[derive(PartialEq)]
struct Weather {
    cell: [i64; 3],
    wind: i64,
    origin: [f32; 2],
    sun: Vec3,
    overcast: f32,
    coverage: f32,
}

impl From<CloudView> for Weather {
    fn from(view: CloudView) -> Self {
        Self {
            cell: view.world_eye.to_array().map(|v| (v / 8.).floor() as i64),
            wind: (view.wind * 1000. / 4.).floor() as i64,
            origin: view.weather_origin,
            sun: view.sun,
            overcast: view.overcast,
            coverage: view.coverage,
        }
    }
}

pub(super) struct WaterClouds {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    weather: Option<Weather>,
}

impl WaterClouds {
    pub(super) fn new(device: &wgpu::Device, sky_layout: &wgpu::BindGroupLayout) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud light reflected in water"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let pipeline = make_pipeline_groups(
            device,
            "water cloud panorama",
            &[sky_layout],
            &cloud_shader_source(),
            &[wgpu::TextureFormat::Rgba8Unorm],
            "fs_water_clouds",
        );
        Self {
            texture,
            view,
            pipeline,
            weather: None,
        }
    }

    pub(super) fn draw<'a>(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        sky: &wgpu::BindGroup,
        view: CloudView,
        receivers: impl Iterator<Item = (&'a wgpu::Buffer, u64)>,
    ) -> bool {
        let weather = Weather::from(view);
        let changed = self.weather.as_ref() != Some(&weather);
        if changed {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sample reflected cloud hemisphere"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, sky, &[]);
            pass.draw(0..3, 0..1);
            self.weather = Some(weather);
        }
        // A complete angular row is exactly one aligned texture-copy row.
        // Newly installed native buffers receive the current panorama too.
        for (buffer, offset) in receivers {
            encoder.copy_texture_to_buffer(
                self.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset,
                        bytes_per_row: Some(WIDTH * 4),
                        rows_per_image: Some(HEIGHT),
                    },
                },
                self.texture.size(),
            );
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panorama_fits_the_uniform_and_tracks_weather_independently_of_view_direction() {
        assert_eq!(WIDTH * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        assert_eq!(crate::terrain_light::CLOUD_OFFSET % 16, 0);
        assert!(crate::terrain_light::CLOUD_OFFSET + BYTES <= 16384);
        let view = CloudView {
            inverse: glam::Mat4::IDENTITY.to_cols_array_2d(),
            relative_eye: Vec3::ZERO,
            world_eye: Vec3::new(-9., 90., 18.),
            sun: Vec3::Y,
            coverage: 0.51,
            overcast: 0.12,
            wind: 0.002,
            weather_origin: [0., 20.],
        };
        let key = Weather::from(view);
        let turn = CloudView {
            inverse: glam::Mat4::from_rotation_x(0.55).to_cols_array_2d(),
            ..view
        };
        assert!(key == Weather::from(turn));
        for changed in [
            CloudView {
                world_eye: view.world_eye + Vec3::X * 12.,
                ..view
            },
            CloudView {
                wind: 0.009,
                ..view
            },
            CloudView {
                weather_origin: [2., 5.],
                ..view
            },
            CloudView {
                coverage: 0.8,
                ..view
            },
        ] {
            assert!(key != Weather::from(changed));
        }
    }
}
