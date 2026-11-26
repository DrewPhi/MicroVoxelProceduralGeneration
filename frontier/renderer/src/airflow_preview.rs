//! Aircraft debug tracers. The sampled forces are from the flight model.
//! Passive tracers use local relative wind and an approximate obstacle/wake field.
use crate::aircraft::{airframe, Airflow, WING_SECTIONS};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use std::time::Instant;

const TRACERS: usize = 256;
const COUNT: usize = TRACERS * 2 + 6 * 32;
const HALF_CELL: f32 = 0.03125;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Marker {
    center: [f32; 4],
    axis: [f32; 4],
    color: [f32; 4],
}

#[derive(Clone, Copy, Default)]
struct Tracer {
    point: Vec3,
    previous: Vec3,
    age: f32,
}

pub(super) struct Preview {
    buffer: wgpu::Buffer,
    instances: Vec<Marker>,
    particles: [Tracer; TRACERS],
    random: u32,
    last: Option<Instant>,
    time: f32,
}

impl Preview {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("airflow markers"),
                size: (COUNT * std::mem::size_of::<Marker>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            instances: Vec::with_capacity(COUNT),
            particles: [Tracer::default(); TRACERS],
            random: 1,
            last: None,
            time: 0.,
        }
    }

    pub fn clear(&mut self) {
        self.last = None;
        self.instances.clear();
    }

    pub fn update(&mut self, queue: &wgpu::Queue, flow: Airflow<'_>) {
        let now = Instant::now();
        let first = self.last.is_none();
        let dt = self
            .last
            .replace(now)
            .map_or(0., |t| now.duration_since(t).as_secs_f32().min(0.05));
        self.time = (self.time + dt) % 1000.;
        self.instances.clear();
        let speed = flow.relative_air.length();
        let direction = flow.relative_air.normalize_or(-Vec3::Z);
        let lateral = direction.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = lateral.cross(direction);
        if speed > 1. || flow.propwash > 1. {
            let steps = (dt * (speed + flow.propwash + 8.) / 0.30).ceil().max(1.) as u32;
            let step = dt / steps as f32;
            let random = &mut self.random;
            for trace in &mut self.particles {
                if first
                    || trace.age > 2.5
                    || trace.point.abs().cmpgt(Vec3::new(10., 8., 16.)).any()
                {
                    let depth = if speed <= 1. {
                        2.3 - 2. * next(random)
                    } else if first {
                        5.5 - 14. * next(random)
                    } else {
                        6.2
                    };
                    let x = if speed > 1. { 6.5 } else { 1.3 };
                    let y = if speed > 1. { 2.1 } else { 1.2 };
                    let point = lateral * (x * (2. * next(random) - 1.))
                        + up * (0.50 + y * (2. * next(random) - 1.))
                        - direction * depth;
                    *trace = Tracer {
                        point,
                        previous: point,
                        age: 0.,
                    };
                }
                trace.age += dt;
                trace.previous = trace.point;
                // Short steps resolve the turn around the thin wing even when
                // the airplane crosses several native cells per rendered frame.
                for _ in 0..steps {
                    let point =
                        trace.point + velocity(trace.point, &flow, direction, self.time) * step;
                    if !crate::aircraft::parked_solid(point) {
                        trace.point = point;
                    }
                }
                if crate::aircraft::parked_solid(trace.point) {
                    trace.age = 3.;
                    continue;
                }
                let color = Vec3::new(0.42, 0.88, 1.0);
                self.instances.push(Marker {
                    center: trace.point.extend(HALF_CELL).to_array(),
                    axis: Vec3::Y.extend(HALF_CELL).to_array(),
                    color: color.extend(1.).to_array(),
                });
                let tail = (trace.point - trace.previous).clamp_length_max(0.30);
                segment(
                    &mut self.instances,
                    trace.point - tail,
                    trace.point,
                    0.012,
                    color * 0.70,
                );
            }
        }
        for section in flow.sections {
            let base = section.origin + Vec3::Y * 0.11;
            let color =
                Vec3::new(0.16, 0.95, 0.46).lerp(Vec3::new(1.0, 0.28, 0.15), section.separation);
            arrow(&mut self.instances, base, section.lift, color);
            arrow(
                &mut self.instances,
                base,
                section.drag,
                Vec3::new(1.0, 0.61, 0.13),
            );
        }
        if !self.instances.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.instances));
        }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, pipeline: &'a wgpu::RenderPipeline) {
        if !self.instances.is_empty() {
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, self.buffer.slice(..));
            pass.draw(0..36, 0..self.instances.len() as u32);
        }
    }
}

fn next(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / 16_777_216.
}

// Remove motion into the obstacle in a band outside its effective skin.
// Tangential motion continues around the obstacle like a smoke tracer.
fn deflect(v: Vec3, point: Vec3, radii: Vec3) -> Vec3 {
    let local = point / radii;
    let distance = local.length();
    if distance > 1.55 {
        return v;
    }
    let outward = (local / radii).normalize_or(Vec3::Y);
    let blend = ((1.55 - distance) * 2.2).clamp(0., 1.);
    let through = v.dot(outward).min(0.);
    v + outward * (-through * blend + v.length() * (1.16 - distance).max(0.) * 1.7)
}

fn velocity(p: Vec3, flow: &Airflow<'_>, direction: Vec3, time: f32) -> Vec3 {
    let mut v = flow.relative_air - flow.angular_velocity.cross(p - flow.center_of_gravity);
    if p.z < airframe::PROP_Z {
        let slip_radius = 0.92 + 0.035 * (airframe::PROP_Z - p.z);
        let radial = (p.x * p.x + (p.y + 0.06).powi(2)).sqrt() / slip_radius;
        v.z -= flow.propwash * (1. - radial).clamp(0., 1.);
    }
    let u = (p.x / airframe::WING.span).abs();
    if u < 1. {
        let (chord, center, height) = airframe::WING.section(u);
        let around = p - Vec3::new(p.x, height, center);
        v = deflect(v, around, Vec3::new(1., 0.16, chord * 0.57));
        let per_side = WING_SECTIONS / 2;
        let index = (u * per_side as f32) as usize + if p.x < 0. { per_side } else { 0 };
        let sample = flow.sections[index];
        let offset = p - sample.origin;
        let behind = offset.dot(direction);
        if behind > 0. {
            let off_axis = offset - direction * behind;
            let strength = 1. / (1. + 2.2 * off_axis.length_squared() + 0.15 * behind);
            v -= sample.lift.clamp_length_max(1600.) * (0.004 * strength);
            let eddy = (time * 13. + p.x * 3.5 + behind * 2.1).sin();
            v.y += eddy * 4. * sample.separation * strength;
        }
    }
    let tail_x = (p.x / airframe::TAIL.span).abs();
    if tail_x < 1. {
        let (chord, center, height) = airframe::TAIL.section(tail_x);
        v = deflect(
            v,
            Vec3::new(0., p.y - height, p.z - center),
            Vec3::new(1., 0.13, chord * 0.57),
        );
    }
    v = deflect(
        v,
        p - Vec3::new(0., 0.12, -0.15),
        Vec3::new(0.73, 0.78, 2.80),
    );
    deflect(
        v,
        p - Vec3::new(0., 0.05, -3.45),
        Vec3::new(0.44, 0.41, 2.17),
    )
}

fn segment(markers: &mut Vec<Marker>, start: Vec3, end: Vec3, radius: f32, color: Vec3) {
    let delta = end - start;
    let length = delta.length();
    if length > 0.005 {
        markers.push(Marker {
            center: (start + delta * 0.5).extend(radius).to_array(),
            axis: (delta / length).extend(length * 0.5).to_array(),
            color: color.extend(1.).to_array(),
        });
    }
}

fn arrow(markers: &mut Vec<Marker>, base: Vec3, force: Vec3, color: Vec3) {
    let length = (force.length() / 550.).min(2.8);
    if length < 0.10 {
        return;
    }
    let dir = force.normalize();
    let tip = base + dir * length;
    let side = dir.cross(Vec3::X).normalize_or(Vec3::Y);
    let head = (length * 0.27).min(0.23);
    segment(markers, base, tip, 0.024, color);
    for sign in [-1., 1.] {
        segment(
            markers,
            tip,
            tip - dir * head + side * (sign * head * 0.52),
            0.024,
            color,
        );
    }
}
