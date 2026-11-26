//! World-aligned sampled height and material windows for canonical CPU sources.
use super::{reduce_maxima, upload};
use crate::Result;
use bytemuck::Pod;
use frontier_world::{voxel_grid, voxel_size, FbmTerrain, Material};
use glam::{DVec2, IVec2};
use std::{sync::Arc, thread::JoinHandle, time::Instant};

type BuildResult<T> = std::result::Result<T, &'static str>;
const STRIP: i32 = 32;
const TOROIDAL_MIP_MAX: u32 = STRIP.ilog2();

#[derive(Clone, Copy)]
struct Rectangle {
    lo: IVec2,
    hi: IVec2,
}

fn entering_rectangles(side: i32, shift: Option<IVec2>) -> [Rectangle; 4] {
    let max = IVec2::splat(side);
    let start = IVec2::ZERO;
    let lo = shift.unwrap_or(max).clamp(start, max);
    let hi = (shift.unwrap_or(max) + max).clamp(start, max);
    [
        Rectangle {
            lo: start,
            hi: IVec2::new(lo.x, side),
        },
        Rectangle {
            lo: IVec2::new(hi.x, 0),
            hi: max,
        },
        Rectangle {
            lo: IVec2::new(lo.x, 0),
            hi: IVec2::new(hi.x, lo.y),
        },
        Rectangle {
            lo: IVec2::new(lo.x, hi.y),
            hi: IVec2::new(hi.x, side),
        },
    ]
}

// Each axis can cross one wrap and, for heights, one explicit halo texel.
fn axis_uploads(start: i32, end: i32, base: i32, side: i32) -> [(i32, i32, i32); 3] {
    let dst = (start + base.rem_euclid(side)).rem_euclid(side);
    let count = (end.min(side) - start).max(0);
    let first = count.min(side - dst);
    [
        (start, dst, first),
        (start + first, 0, count - first),
        (side, side, (end - side).max(0)),
    ]
}

fn upload_scrolled<T: Pod>(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip: u32,
    side: i32,
    base: IVec2,
    previous: Option<IVec2>,
    halo: bool,
    data: &[T],
) {
    let stride = (side + i32::from(halo)) as usize;
    let shift = previous.map(|old| {
        (old.as_i64vec2() - base.as_i64vec2())
            .clamp(
                glam::I64Vec2::splat(-i64::from(side)),
                glam::I64Vec2::splat(i64::from(side)),
            )
            .as_ivec2()
    });
    let entering = entering_rectangles(side, shift);
    let edges = [
        Rectangle {
            lo: IVec2::new(side, 0),
            hi: IVec2::new(side + 1, side),
        },
        Rectangle {
            lo: IVec2::new(0, side),
            hi: IVec2::splat(side + 1),
        },
    ];
    for rect in entering
        .iter()
        .chain(edges.iter().take(if halo { 2 } else { 0 }))
    {
        for (sx, dx, width) in axis_uploads(rect.lo.x, rect.hi.x, base.x, side) {
            for (sy, dy, height) in axis_uploads(rect.lo.y, rect.hi.y, base.y, side) {
                if width == 0 || height == 0 {
                    continue;
                }
                let offset = sy as usize * stride + sx as usize;
                let len = (height as usize - 1) * stride + width as usize;
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: mip,
                        origin: wgpu::Origin3d {
                            x: dx as u32,
                            y: dy as u32,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&data[offset..offset + len]),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some((stride * size_of::<T>()) as u32),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: width as u32,
                        height: height as u32,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Sample {
    height: f32,
    pigment: [u8; 4],
}

struct Window {
    base: IVec2,
    samples: Vec<Sample>,
    tiles: Option<TileField>,
    procedural_queries: u64,
}

impl Window {
    fn sample(&self, point: IVec2, width: usize) -> Option<Sample> {
        let d = point.as_i64vec2() - self.base.as_i64vec2();
        let range = 0..width as i64;
        (range.contains(&d.x) && range.contains(&d.y))
            .then(|| self.samples[d.y as usize * width + d.x as usize])
    }
}

struct StagedWindow {
    window: Window,
    heights: Vec<f32>,
    colors: Vec<[u8; 4]>,
    bounds: Bounds,
}

enum Bounds {
    Heights(Vec<Vec<f32>>),
    HeightsAndError(Vec<Vec<[f32; 2]>>),
}

impl StagedWindow {
    fn new(window: Window, side: usize, need_errors: bool) -> Self {
        let heights = window.samples.iter().map(|s| s.height).collect::<Vec<_>>();
        let colors = window.samples.iter().map(|s| s.pigment).collect();
        let width = side + 1;
        // Each rendered bilinear patch is bounded by its four corners.
        let first_level = (0..side * side)
            .map(|i| {
                let a = (i / side) * width + (i % side);
                heights[a]
                    .max(heights[a + 1])
                    .max(heights[a + width])
                    .max(heights[a + width + 1])
            })
            .collect::<Vec<_>>();
        let mut maxima = vec![first_level];
        for mip in 0..side.ilog2() {
            maxima.push(reduce_maxima(side >> mip, &maxima[mip as usize]));
        }
        let bounds = if need_errors {
            let errors = bilinear_error_bounds(&heights, side);
            Bounds::HeightsAndError(
                maxima
                    .iter()
                    .enumerate()
                    .map(|(mip, data)| {
                        let errors_at_mip = mip.checked_sub(1).map(|i| &errors[i]);
                        data.iter()
                            .enumerate()
                            .map(|(i, &top)| [top, errors_at_mip.map_or(0., |r| r[i])])
                            .collect()
                    })
                    .collect(),
            )
        } else {
            Bounds::Heights(maxima)
        };
        Self {
            window,
            heights,
            colors,
            bounds,
        }
    }
}

fn bilinear_error_bounds(heights: &[f32], side: usize) -> Vec<Vec<f32>> {
    let width = side + 1;
    let mut errors: Vec<Vec<f32>> = Vec::new();
    for mip in 1..=side.ilog2() {
        let step = 1 << mip;
        let half = step / 2;
        let n = side / step;
        let previous = errors.last();
        let layer = (0..n * n)
            .map(|i| {
                let x = i % n;
                let z = i / n;
                let start = (z * width + x) * step;
                let h = |dx, dz| f64::from(heights[start + dx + dz * width]);
                let [a, b, c, d] = [h(0, 0), h(step, 0), h(0, step), h(step, step)];
                let local = [
                    h(half, 0) - (a + b) * 0.5,
                    h(half, step) - (c + d) * 0.5,
                    h(0, half) - (a + c) * 0.5,
                    h(step, half) - (b + d) * 0.5,
                    h(half, half) - (a + b + c + d) * 0.25,
                ]
                .into_iter()
                .map(f64::abs)
                .fold(0., f64::max);
                let child = previous.map_or(0., |data| {
                    let stride = 2 * n;
                    let i = 2 * z * stride + 2 * x;
                    data[i]
                        .max(data[i + 1])
                        .max(data[i + stride])
                        .max(data[i + stride + 1])
                });
                // The difference of two bilinear patches is bilinear on
                // each child. Its extremum is attained at a child corner.
                ((local + f64::from(child)) as f32).next_up()
            })
            .collect();
        errors.push(layer);
    }
    errors
}

pub(super) struct SampledLayer {
    pub(super) strip_uploads: bool,
    pub(super) error_bounds: bool,
    pub(super) tile_error_m: f32,
    pub(super) outer_coverage: bool,
    pub(super) defer_initial: bool,
    terrain: FbmTerrain,
    current: Option<Arc<Window>>,
    pending: Option<JoinHandle<BuildResult<StagedWindow>>>,
    side: u32,
    pitch: f64,
    first_ready_ms: f64,
    publications: u64,
    max_moving_upload_ms: f64,
    first_queries: u64,
    total_queries: u64,
}

impl SampledLayer {
    pub(super) fn new(terrain: FbmTerrain, side: u32, pitch: f64) -> Self {
        Self {
            strip_uploads: false,
            error_bounds: false,
            tile_error_m: 0.,
            outer_coverage: false,
            defer_initial: false,
            terrain,
            current: None,
            pending: None,
            side,
            pitch,
            first_ready_ms: 0.,
            publications: 0,
            max_moving_upload_ms: 0.,
            first_queries: 0,
            total_queries: 0,
        }
    }

    pub(super) fn ready(&self) -> bool {
        self.current.is_some()
    }

    pub(super) fn update(
        &mut self,
        queue: &wgpu::Queue,
        heights: &wgpu::Texture,
        maxima: &wgpu::Texture,
        pigments: &wgpu::Texture,
        eye: DVec2,
    ) -> Result<IVec2> {
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
        // The outer level has room for the 9 km visibility radius during ordinary
        // recentering. For a teleport, publish a fresh window before drawing.
        // The inner level can fade to the outer level while its worker catches up.
        let urgent = (current.is_none() && !self.defer_initial)
            || (self.outer_coverage && displaced > f64::from(self.side) * (96. / 1024.));
        if urgent {
            let start = Instant::now();
            let pending = self.pending.take().map(join).transpose()?;
            let window = match pending {
                Some(window) if window.window.base == target => window,
                previous => {
                    let prior = previous
                        .as_ref()
                        .map(|w| &w.window)
                        .or(self.current.as_deref());
                    build_staged_window(
                        &self.terrain,
                        self.side,
                        self.pitch,
                        target,
                        prior,
                        self.error_bounds,
                        self.tile_error_m,
                    )?
                }
            };
            self.publish(queue, heights, maxima, pigments, window);
            if current.is_none() {
                self.first_ready_ms = start.elapsed().as_secs_f64() * 1000.;
            }
        } else if self.pending.as_ref().is_some_and(JoinHandle::is_finished) {
            let window = join(self.pending.take().unwrap())?;
            self.publish(queue, heights, maxima, pigments, window);
        }
        let base = self.current.as_ref().map(|window| window.base);
        if Some(target) != base && self.pending.is_none() {
            let terrain = self.terrain.clone();
            let previous = self.current.clone();
            let side = self.side;
            let pitch = self.pitch;
            let need_errors = self.error_bounds;
            let tile_error_m = self.tile_error_m;
            self.pending = Some(std::thread::spawn(move || {
                build_staged_window(
                    &terrain,
                    side,
                    pitch,
                    target,
                    previous.as_deref(),
                    need_errors,
                    tile_error_m,
                )
            }));
        }
        Ok(base.unwrap_or(target))
    }

    pub(super) fn publish_report(&self) -> String {
        format!(
            r#"{{"first_ready_ms":{},"publications":{},"max_moving_upload_ms":{},"first_procedural_queries":{},"total_procedural_queries":{}}}"#,
            self.first_ready_ms,
            self.publications,
            self.max_moving_upload_ms,
            self.first_queries,
            self.total_queries,
        )
    }

    fn publish(
        &mut self,
        queue: &wgpu::Queue,
        heights: &wgpu::Texture,
        maxima: &wgpu::Texture,
        pigments: &wgpu::Texture,
        window: StagedWindow,
    ) {
        let start = Instant::now();
        let width = self.side + 1;
        let base = window.window.base;
        let previous = self.current.as_ref().map(|old| old.base);
        if self.strip_uploads {
            upload_scrolled(
                queue,
                heights,
                0,
                self.side as i32,
                base,
                previous,
                true,
                &window.heights,
            );
            upload_scrolled(
                queue,
                pigments,
                0,
                self.side as i32,
                base,
                previous,
                true,
                &window.colors,
            );
        } else {
            upload(queue, heights, 0, width, &window.heights);
            upload(queue, pigments, 0, width, &window.colors);
        }
        match &window.bounds {
            Bounds::Heights(mips) => self.publish_bounds(queue, maxima, base, previous, mips),
            Bounds::HeightsAndError(mips) => {
                self.publish_bounds(queue, maxima, base, previous, mips)
            }
        }
        if self.current.is_some() {
            self.max_moving_upload_ms = self
                .max_moving_upload_ms
                .max(start.elapsed().as_secs_f64() * 1000.);
        } else {
            self.first_queries = window.window.procedural_queries;
        }
        self.total_queries += window.window.procedural_queries;
        self.publications += 1;
        self.current = Some(Arc::new(window.window));
    }

    fn publish_bounds<T: Pod>(
        &self,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        base: IVec2,
        previous: Option<IVec2>,
        mips: &[Vec<T>],
    ) {
        for (mip, data) in mips.iter().enumerate() {
            let side = self.side >> mip;
            if self.strip_uploads && mip as u32 <= TOROIDAL_MIP_MAX {
                let shift = |p: IVec2| IVec2::new(p.x >> mip, p.y >> mip);
                upload_scrolled(
                    queue,
                    texture,
                    mip as u32,
                    side as i32,
                    shift(base),
                    previous.map(shift),
                    false,
                    data,
                );
            } else {
                upload(queue, texture, mip as u32, side, data);
            }
        }
    }
}

fn join(job: JoinHandle<BuildResult<StagedWindow>>) -> BuildResult<StagedWindow> {
    job.join().map_err(|_| "rolling sample worker panicked")?
}

fn build_staged_window(
    terrain: &FbmTerrain,
    side: u32,
    pitch: f64,
    base: IVec2,
    previous: Option<&Window>,
    need_errors: bool,
    tile_error_m: f32,
) -> BuildResult<StagedWindow> {
    let window = build_window(terrain, side, pitch, base, previous, tile_error_m)?;
    Ok(StagedWindow::new(window, side as usize, need_errors))
}

fn build_window(
    terrain: &FbmTerrain,
    side: u32,
    pitch: f64,
    base: IVec2,
    previous: Option<&Window>,
    tile_error_m: f32,
) -> BuildResult<Window> {
    let width = side as usize + 1;
    let mut samples = vec![Sample::default(); width * width];
    let source = ColumnSource {
        terrain,
        pitch,
        landscapes: terrain.params().recipe.has_landscape_relief(),
    };
    let Some(error) = (tile_error_m > 0.).then_some(tile_error_m) else {
        // A cold Eroded window has enough independent work to use six cores.
        // Recentered windows keep four workers and reuse their interior while
        // the GPU is rendering.
        let workers = if side >= 1024
            && previous.is_none()
            && terrain.params().recipe == frontier_world::FbmRecipe::Eroded
        {
            std::thread::available_parallelism().map_or(4, |n| n.get().min(6))
        } else {
            4
        };
        let procedural_queries = parallel_fill(&mut samples, width, workers, |x, z, sample| {
            let point = base + IVec2::new(x as i32, z as i32);
            if let Some(old) = previous.and_then(|w| w.sample(point, width)) {
                *sample = old;
                return Ok(0);
            }
            *sample = source.sample(point)?;
            Ok(1)
        })?;
        return Ok(Window {
            base,
            samples,
            tiles: None,
            procedural_queries,
        });
    };

    let coarse_width = side as usize / 2 + 1;
    let mut coarse = vec![Sample::default(); coarse_width * coarse_width];
    let coarse_queries = parallel_fill(&mut coarse, coarse_width, 4, |x, z, sample| {
        let point = base + IVec2::new(x as i32 * 2, z as i32 * 2);
        if let Some(old) = previous.and_then(|w| w.sample(point, width)) {
            *sample = old;
            return Ok(0);
        }
        *sample = source.sample(point)?;
        Ok(1)
    })?;
    let (tiles, probe_queries) = TileField::build(
        &source,
        side,
        base,
        previous.and_then(|w| w.tiles.as_ref()),
        error,
    )?;
    let fine_queries = parallel_fill(&mut samples, width, 4, |x, z, sample| {
        let point = base + IVec2::new(x as i32, z as i32);
        if let Some(old) = previous.and_then(|w| w.sample(point, width)) {
            *sample = old;
            return Ok(0);
        }
        let at = |x: usize, z: usize| coarse[z * coarse_width + x];
        let [x0, x1, z0, z1] = [x / 2, x.div_ceil(2), z / 2, z.div_ceil(2)];
        *sample = at(x0, z0);
        if x0 == x1 && z0 == z1 {
            return Ok(0);
        }
        // Adjacent tiles share the same coarser lattice. Only the height
        // residual is refined, so paint stays comparable across tolerances.
        sample.height =
            0.25 * (at(x0, z0).height + at(x1, z0).height + at(x0, z1).height + at(x1, z1).height);
        let w = tiles.weight(point);
        if w == 0. {
            return Ok(0);
        }
        let h = source.top(point)?.0;
        sample.height += w * (h - sample.height);
        Ok(1)
    })?;
    Ok(Window {
        base,
        samples,
        tiles: Some(tiles),
        procedural_queries: coarse_queries + fine_queries + probe_queries,
    })
}

struct ColumnSource<'a> {
    terrain: &'a FbmTerrain,
    pitch: f64,
    landscapes: bool,
}

impl ColumnSource<'_> {
    fn native_cell(&self, point: IVec2) -> BuildResult<frontier_world::Cell> {
        let meters = point.as_dvec2() * self.pitch;
        voxel_grid()
            .checked_cell([meters.x, 0., meters.y])
            .ok_or("rolling sample is outside the native cell lattice")
    }

    fn top(&self, point: IVec2) -> BuildResult<(f32, bool)> {
        let cell = self.native_cell(point)?;
        let column = self.terrain.column_cells(cell.x, cell.z);
        Ok((
            (column.visible_top as f64 * voxel_size()) as f32,
            column.top_material() == Material::Water,
        ))
    }

    fn sample(&self, point: IVec2) -> BuildResult<Sample> {
        let cell = self.native_cell(point)?;
        let (column, cover) = self.terrain.column_with_tree_cover(cell.x, cell.z);
        let [r, g, b] = column
            .ground_surface
            .color()
            .map(|v| (255. * v).round() as u8);
        let alpha = if column.top_material() == Material::Water {
            let depth = (column.visible_top - column.ground_top) as f64 * voxel_size();
            if self.landscapes {
                // Upper half stores the wet bit and 0.125 m depth increments.
                128 + (depth * 8.).ceil().min(127.) as u8
            } else {
                (depth * 16.).ceil() as u8
            }
        } else {
            (cover * 127.).round() as u8
        };
        Ok(Sample {
            height: (column.visible_top as f64 * voxel_size()) as f32,
            pigment: [r, g, b, alpha],
        })
    }
}

struct TileField {
    origin: IVec2,
    side: usize,
    strengths: Vec<f32>,
}

impl TileField {
    fn build(
        source: &ColumnSource,
        side: u32,
        base: IVec2,
        old: Option<&Self>,
        error: f32,
    ) -> BuildResult<(Self, u64)> {
        let origin = base / STRIP - IVec2::ONE;
        let side = (side as i32 / STRIP + 3) as usize;
        let mut strengths = vec![0.; side * side];
        let queries = parallel_fill(&mut strengths, side, 4, |x, z, output| {
            let tile = origin + IVec2::new(x as i32, z as i32);
            if let Some(previous) = old.and_then(|cache| cache.strength(tile)) {
                *output = previous;
                return Ok(0);
            }
            let mut residual = 0.0f32;
            for dz in [4, 14, 24] {
                for dx in [4, 14, 24] {
                    let p = tile * STRIP + IVec2::new(dx, dz);
                    let a = source.top(p)?;
                    let b = source.top(p + IVec2::new(2, 0))?;
                    let c = source.top(p + IVec2::new(0, 2))?;
                    let d = source.top(p + IVec2::splat(2))?;
                    let mid = source.top(p + IVec2::ONE)?;
                    let e = (mid.0 - 0.25 * (a.0 + b.0 + c.0 + d.0)).abs();
                    residual = residual.max(e);
                    if [a, b, c, d].iter().any(|&(_, wet)| wet != mid.1) {
                        residual = residual.max(2. * error);
                    }
                }
            }
            *output = smooth_unit(residual / error - 1.);
            Ok(45)
        })?;
        Ok((
            Self {
                origin,
                side,
                strengths,
            },
            queries,
        ))
    }

    fn strength(&self, tile: IVec2) -> Option<f32> {
        let p = tile - self.origin;
        let range = 0..self.side as i32;
        (range.contains(&p.x) && range.contains(&p.y))
            .then(|| self.strengths[p.y as usize * self.side + p.x as usize])
    }

    fn weight(&self, point: IVec2) -> f32 {
        let tile = IVec2::new(point.x.div_euclid(STRIP), point.y.div_euclid(STRIP));
        let centre = self.strength(tile).unwrap();
        if centre == 0. {
            return 0.;
        }
        let offset = point - tile * STRIP;
        let neighbor = |local: i32| if local < STRIP / 2 { -1 } else { 1 };
        let distance = |local: i32| local.min(STRIP - local) as f32;
        let nx = IVec2::new(neighbor(offset.x), 0);
        let nz = IVec2::new(0, neighbor(offset.y));
        let x = centre.min(self.strength(tile + nx).unwrap());
        let z = centre.min(self.strength(tile + nz).unwrap());
        let both = x.min(z).min(self.strength(tile + nx + nz).unwrap());
        let wx = smooth_unit(distance(offset.x) / 8.);
        let wz = smooth_unit(distance(offset.y) / 8.);
        let edge = both + wx * (z - both);
        let inside = x + wx * (centre - x);
        edge + wz * (inside - edge)
    }
}

fn smooth_unit(value: f32) -> f32 {
    let t = value.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn parallel_fill<T: Send>(
    samples: &mut [T],
    width: usize,
    workers: usize,
    fill: impl Fn(usize, usize, &mut T) -> BuildResult<u64> + Sync,
) -> BuildResult<u64> {
    let rows = width.div_ceil(workers);
    let fill = &fill;
    std::thread::scope(|jobs| {
        let workers = samples
            .chunks_mut(rows * width)
            .enumerate()
            .map(|(stripe, strip)| {
                jobs.spawn(move || -> BuildResult<u64> {
                    let mut queries = 0;
                    for (row, row_samples) in strip.chunks_mut(width).enumerate() {
                        for (x, sample) in row_samples.iter_mut().enumerate() {
                            queries += fill(x, stripe * rows + row, sample)?;
                        }
                    }
                    Ok(queries)
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|job| job.join().map_err(|_| "rolling sample worker panicked")?)
            .sum()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolled_rectangles_cover_entering_cells_and_halo() {
        let side = 8;
        let base = IVec2::new(-13, 15);
        for shift in [
            None,
            Some(IVec2::new(-3, 2)),
            Some(IVec2::new(4, -1)),
            Some(IVec2::ZERO),
        ] {
            let mut written = [[false; 9]; 9];
            let halo = [
                Rectangle {
                    lo: IVec2::new(side, 0),
                    hi: IVec2::new(side + 1, side),
                },
                Rectangle {
                    lo: IVec2::new(0, side),
                    hi: IVec2::splat(side + 1),
                },
            ];
            for rect in entering_rectangles(side, shift).into_iter().chain(halo) {
                for (sx, dx, width) in axis_uploads(rect.lo.x, rect.hi.x, base.x, side) {
                    for (sy, dy, height) in axis_uploads(rect.lo.y, rect.hi.y, base.y, side) {
                        for y in 0..height {
                            for x in 0..width {
                                let physical = |v: i32, b: i32| {
                                    if v == side {
                                        side
                                    } else {
                                        (v + b).rem_euclid(side)
                                    }
                                };
                                assert_eq!(
                                    [dx + x, dy + y],
                                    [physical(sx + x, base.x), physical(sy + y, base.y)]
                                );
                                let was_written =
                                    &mut written[(sy + y) as usize][(sx + x) as usize];
                                assert!(!*was_written);
                                *was_written = true;
                            }
                        }
                    }
                }
            }
            for (y, row) in written.iter().enumerate() {
                for (x, &value) in row.iter().enumerate() {
                    let reused = shift.is_some_and(|old| {
                        let p = IVec2::new(x as i32, y as i32) - old;
                        x < side as usize
                            && y < side as usize
                            && (0..side).contains(&p.x)
                            && (0..side).contains(&p.y)
                    });
                    assert_eq!(value, !reused);
                }
            }
        }
    }

    #[test]
    fn residual_mips_bound_every_descendant_vertex() {
        let side = 16;
        let width = side + 1;
        let heights = (0..width * width)
            .map(|i| ((i * 127 + i * i * 11) % 213) as f32 * 0.03125 - 20.)
            .collect::<Vec<_>>();
        for (level, errors) in bilinear_error_bounds(&heights, side).iter().enumerate() {
            let step = 2usize << level;
            for (i, &error) in errors.iter().enumerate() {
                let nx = side / step;
                let start = (i / nx * width + i % nx) * step;
                let h = |x, z| f64::from(heights[start + x + z * width]);
                for iz in 0..=step {
                    for ix in 0..=step {
                        let x = ix as f64 / step as f64;
                        let z = iz as f64 / step as f64;
                        let estimate = (1. - z) * ((1. - x) * h(0, 0) + x * h(step, 0))
                            + z * ((1. - x) * h(0, step) + x * h(step, step));
                        assert!((estimate - h(ix, iz)).abs() <= f64::from(error));
                    }
                }
            }
        }
    }

    #[test]
    fn graded_tiles_keep_the_same_samples_when_the_window_scrolls() {
        let terrain = FbmTerrain::new(
            19,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::Landforms,
                ..Default::default()
            },
        )
        .unwrap();
        let base = IVec2::new(1344, 1024);
        let old = build_window(&terrain, 64, 0.75, base, None, 0.1).unwrap();
        let next = base + IVec2::new(32, -32);
        let reused = build_window(&terrain, 64, 0.75, next, Some(&old), 0.1).unwrap();
        let fresh = build_window(&terrain, 64, 0.75, next, None, 0.1).unwrap();
        for (a, b) in reused.samples.iter().zip(&fresh.samples) {
            assert_eq!(a.height.to_bits(), b.height.to_bits());
            assert_eq!(a.pigment, b.pigment);
        }
        assert!(reused.procedural_queries < fresh.procedural_queries);
    }
}
