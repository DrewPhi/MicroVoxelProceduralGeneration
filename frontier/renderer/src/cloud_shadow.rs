use glam::{Vec2, Vec3, Vec3Swizzles};
use std::cell::Cell;

const SIZE: u32 = 128;
const CELL_M: f32 = 96.;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(super) const BYTES: u64 = SIZE as u64 * SIZE as u64 * 4;

#[derive(Clone, Copy, PartialEq)]
struct Footprint {
    center: Vec2,
    sun: Vec3,
    coverage: f32,
}

impl Footprint {
    fn new(eye: Vec3, sun: Vec3, weather: Vec2, coverage: f32) -> Option<Self> {
        if sun.y <= 0.08 || coverage <= 0.001 {
            return None;
        }
        // Coordinates are on the cloud base in the advecting weather field.
        // Snap by whole texels so overlapping samples survive map recentering.
        let base = eye.xz() + sun.xz() * ((1720. - eye.y) / sun.y) + weather;
        let snap = CELL_M * 8.;
        Some(Self {
            center: (base / snap).round() * snap,
            sun,
            coverage,
        })
    }
}

pub(super) struct CloudShadow {
    pub view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    requested: Cell<Option<Footprint>>,
    drawn: Cell<Option<Footprint>>,
}

impl CloudShadow {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        Self {
            view: super::texture(device, "sunlight through cloud deck", FORMAT, SIZE, SIZE, 1),
            pipeline: super::make_pipeline_groups(
                device,
                "cloud sunlight",
                &[layout],
                super::SKY_SHADER,
                &[FORMAT],
                "fs_cloud_sun",
            ),
            requested: Cell::new(None),
            drawn: Cell::new(None),
        }
    }
    pub fn prepare(&self, eye: Vec3, sun: Vec3, weather: Vec2, coverage: f32) -> [f32; 4] {
        let footprint = Footprint::new(eye, sun, weather, coverage);
        self.requested.set(footprint);
        footprint.map_or([0.; 4], |p| {
            [p.center.x, p.center.y, CELL_M, 1. / (CELL_M * SIZE as f32)]
        })
    }
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, sky_bind: &wgpu::BindGroup) {
        let Some(p) = self.requested.get() else {
            return;
        };
        if self.drawn.replace(Some(p)) == Some(p) {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cloud sunlight"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, sky_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sunlight_grid_is_stable_while_flying_along_a_sun_ray() {
        let sun = Vec3::new(0.4, 0.8, 0.2).normalize();
        let eye = Vec3::new(-861., 437., 2130.);
        let weather = Vec2::new(-510., 109.);
        let a = Footprint::new(eye, sun, weather, 0.6).unwrap();
        let b = Footprint::new(eye + sun * 2900., sun, weather, 0.6).unwrap();
        assert!(a == b);
        let moved = Footprint::new(eye + Vec3::X * 3500., sun, weather, 0.6).unwrap();
        assert_eq!(((moved.center - a.center) / CELL_M).fract(), Vec2::ZERO);
        assert!(Footprint::new(eye, sun, weather, 0.).is_none());
        assert!(Footprint::new(eye, -Vec3::Y, weather, 0.6).is_none());
    }
}
