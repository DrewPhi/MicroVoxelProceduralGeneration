//! World-aligned sampled height and material windows for canonical CPU sources.
use super::{upload, upload_maxima};
use crate::Result;
use frontier_world::{voxel_grid, voxel_size, FbmTerrain, Material};
use glam::{DVec2, IVec2};
use std::{sync::Arc, thread::JoinHandle};

type BuildResult<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Default)]
struct Sample {
    height: f32,
    pigment: [u8; 4],
}

struct Window {
    base: IVec2,
    samples: Vec<Sample>,
}

pub(super) struct SampledLayer {
    terrain: FbmTerrain,
    current: Option<Arc<Window>>,
    pending: Option<JoinHandle<BuildResult<Window>>>,
    side: u32,
    pitch: f64,
}

impl SampledLayer {
    pub(super) fn new(terrain: FbmTerrain, side: u32, pitch: f64) -> Self {
        Self {
            terrain,
            current: None,
            pending: None,
            side,
            pitch,
        }
    }

    pub(super) fn update(
        &mut self,
        queue: &wgpu::Queue,
        heights: &wgpu::Texture,
        maxima: &wgpu::Texture,
        pigments: &wgpu::Texture,
        eye: DVec2,
    ) -> Result<IVec2> {
        const STRIP: i32 = 32;
        let snapped = (eye / (self.pitch * f64::from(STRIP))).floor() * f64::from(STRIP);
        let bound = f64::from(i32::MAX - self.side as i32);
        if snapped.abs().max_element() >= bound {
            return Err("camera is outside the rolling sampled cache address range".into());
        }
        let target = snapped.as_ivec2() - IVec2::splat(self.side as i32 / 2);
        let current = self.current.as_ref().map(|window| window.base);
        let displaced = current.map_or(0., |base| {
            ((eye / self.pitch) - base.as_dvec2() - f64::from(self.side) * 0.5)
                .abs()
                .max_element()
        });
        // The outer level has room for the 6 km frustum during ordinary
        // recentering. For a teleport, publish a fresh window before drawing.
        // The inner level can fade to the outer level while its worker catches up.
        let urgent = current.is_none() || (self.pitch >= 24. && displaced > 96.);
        if urgent {
            let previous = match self.pending.take() {
                Some(job) => Some(Arc::new(join(job)?)),
                None => self.current.clone(),
            };
            let window = if let Some(window) = previous.filter(|window| window.base == target) {
                window
            } else {
                Arc::new(build_window(
                    &self.terrain,
                    self.side,
                    self.pitch,
                    target,
                    self.current.as_deref(),
                )?)
            };
            self.publish(queue, heights, maxima, pigments, window);
        } else if self.pending.as_ref().is_some_and(JoinHandle::is_finished) {
            let window = Arc::new(join(self.pending.take().unwrap())?);
            self.publish(queue, heights, maxima, pigments, window);
        }
        let base = self.current.as_ref().unwrap().base;
        if target != base && self.pending.is_none() {
            let terrain = self.terrain.clone();
            let previous = self.current.clone();
            let side = self.side;
            let pitch = self.pitch;
            self.pending = Some(std::thread::spawn(move || {
                build_window(&terrain, side, pitch, target, previous.as_deref())
            }));
        }
        Ok(base)
    }

    fn publish(
        &mut self,
        queue: &wgpu::Queue,
        heights: &wgpu::Texture,
        maxima: &wgpu::Texture,
        pigments: &wgpu::Texture,
        window: Arc<Window>,
    ) {
        let width = self.side + 1;
        let heights_data = window.samples.iter().map(|s| s.height).collect::<Vec<_>>();
        let colors = window.samples.iter().map(|s| s.pigment).collect::<Vec<_>>();
        let stride = width as usize;
        // Each bilinear patch is bounded by its four corners.
        let side = self.side as usize;
        let bounds = (0..side * side)
            .map(|i| {
                let a = (i / side) * stride + (i % side);
                heights_data[a]
                    .max(heights_data[a + 1])
                    .max(heights_data[a + stride])
                    .max(heights_data[a + stride + 1])
            })
            .collect();
        upload(queue, heights, 0, width, &heights_data);
        upload(queue, pigments, 0, width, &colors);
        upload_maxima(queue, maxima, side, bounds);
        self.current = Some(window);
    }
}

fn join(job: JoinHandle<BuildResult<Window>>) -> BuildResult<Window> {
    job.join().map_err(|_| "rolling sample worker panicked")?
}

fn build_window(
    terrain: &FbmTerrain,
    side: u32,
    pitch: f64,
    base: IVec2,
    previous: Option<&Window>,
) -> BuildResult<Window> {
    let width = side as usize + 1;
    let mut samples = vec![Sample::default(); width * width];
    let rows = width.div_ceil(4);
    std::thread::scope(|jobs| -> BuildResult<()> {
        let workers = samples
            .chunks_mut(rows * width)
            .enumerate()
            .map(|(stripe, strip)| {
                jobs.spawn(move || -> BuildResult<()> {
                    for (i, sample) in strip.iter_mut().enumerate() {
                        let point = base
                            + IVec2::new((i % width) as i32, (stripe * rows + i / width) as i32);
                        if let Some(old) = previous {
                            let d = point.as_i64vec2() - old.base.as_i64vec2();
                            if (0..i64::from(width as u32)).contains(&d.x)
                                && (0..i64::from(width as u32)).contains(&d.y)
                            {
                                *sample = old.samples[d.y as usize * width + d.x as usize];
                                continue;
                            }
                        }
                        let meters = point.as_dvec2() * pitch;
                        let cell = voxel_grid()
                            .checked_cell([meters.x, 0., meters.y])
                            .ok_or("rolling sample is outside the native cell lattice")?;
                        let column = terrain.column_cells(cell.x, cell.z);
                        let [r, g, b] = column
                            .ground_surface
                            .color()
                            .map(|v| (255. * v).round() as u8);
                        *sample = Sample {
                            height: (column.visible_top as f64 * voxel_size()) as f32,
                            pigment: [
                                r,
                                g,
                                b,
                                if column.top_material() == Material::Water {
                                    // Optical depth in 6.25 cm increments.
                                    // Beyond 16 m the bed is already absorbed.
                                    (((column.visible_top - column.ground_top) as f64
                                        * voxel_size()
                                        * 16.)
                                        .ceil()) as u8
                                } else {
                                    0
                                },
                            ],
                        };
                    }
                    Ok(())
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker
                .join()
                .map_err(|_| "rolling sample worker panicked")??;
        }
        Ok(())
    })?;
    Ok(Window { base, samples })
}
