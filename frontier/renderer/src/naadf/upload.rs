//! Upload unreachable arena slots and a temporary root strip, then publish
//! the new root mapping in one queue-ordered texture copy transaction.
use super::{
    scatter,
    stream::{Change, Roots, Write},
    Gpu, Volume,
};

const MIB: usize = 1024 * 1024;

#[derive(Clone, Copy)]
struct UploadConfig {
    batch: bool,
    scatter: bool,
    bytes: usize,
    calls: usize,
}
impl UploadConfig {
    fn active() -> Self {
        static CONFIG: std::sync::OnceLock<UploadConfig> = std::sync::OnceLock::new();
        *CONFIG.get_or_init(|| {
            let limit = |name, default, max| {
                std::env::var(name)
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|&n| (1..=max).contains(&n))
                    .unwrap_or(default)
            };
            Self {
                batch: std::env::var_os("FRONTIER_NAADF_BATCH_UPLOAD").is_some_and(|s| s == "1"),
                scatter: std::env::var_os("FRONTIER_NAADF_SCATTER_UPLOAD")
                    .is_some_and(|s| s == "1"),
                // 256 m radius means at most 512 × 512 words in a root plane.
                bytes: limit("FRONTIER_NAADF_UPLOAD_MIB", 8, 64) * MIB,
                calls: limit("FRONTIER_NAADF_UPLOAD_CALLS", 64, 512),
            }
        })
    }
}

pub(super) fn scatter_allowance() -> Option<usize> {
    let config = UploadConfig::active();
    config.scatter.then_some(config.bytes)
}

#[derive(Default)]
pub(super) struct UploadFrame {
    root_bytes: usize,
    arena_bytes: usize,
    root_calls: usize,
    arena_calls: usize,
    arena_slices: usize,
    byte_limited: bool,
    call_limited: bool,
    root_phase: bool,
    descriptor_bytes: usize,
    scatter_dispatches: usize,
    descriptor_limit_hits: usize,
    scatter_pack_ms: f64,
    scatter_gpu_bytes: usize,
    scatter_cpu_bytes: usize,
}
impl UploadFrame {
    fn bytes(&self) -> usize {
        self.root_bytes + self.arena_bytes + self.descriptor_bytes
    }
    fn calls(&self) -> usize {
        self.root_calls + self.arena_calls
    }
}

/// Process totals, including writes for shifts that were later canceled.
/// Bytes are payload bytes; queue internals may pad texture row storage.
/// No budget is recorded on frames that only wait for raster coverage.
#[derive(Default)]
pub(super) struct UploadStats {
    frames: u64,
    root_bytes: u64,
    arena_bytes: u64,
    root_calls: u64,
    arena_calls: u64,
    arena_slices: u64,
    byte_limited: u64,
    call_limited: u64,
    root_phase: u64,
    unused_bytes: u64,
    unused_calls: u64,
    peak_bytes: usize,
    peak_calls: usize,
    pub(super) publish_copies: u64,
    descriptor_bytes: u64,
    scatter_dispatches: u64,
    descriptor_limit_hits: u64,
    scatter_pack_ms: f64,
    scatter_gpu_bytes_peak: usize,
    scatter_cpu_bytes_peak: usize,
}
impl UploadStats {
    pub(super) fn record(&mut self, frame: UploadFrame) {
        if frame.calls() == 0 {
            return;
        }
        let config = UploadConfig::active();
        self.frames += 1;
        self.root_bytes += frame.root_bytes as u64;
        self.arena_bytes += frame.arena_bytes as u64;
        self.root_calls += frame.root_calls as u64;
        self.arena_calls += frame.arena_calls as u64;
        self.arena_slices += frame.arena_slices as u64;
        self.byte_limited += u64::from(frame.byte_limited);
        self.call_limited += u64::from(frame.call_limited);
        self.root_phase += u64::from(frame.root_phase);
        self.unused_bytes += config.bytes.saturating_sub(frame.bytes()) as u64;
        self.unused_calls += config.calls.saturating_sub(frame.calls()) as u64;
        self.peak_bytes = self.peak_bytes.max(frame.bytes());
        self.peak_calls = self.peak_calls.max(frame.calls());
        self.descriptor_bytes += frame.descriptor_bytes as u64;
        self.scatter_dispatches += frame.scatter_dispatches as u64;
        self.descriptor_limit_hits += frame.descriptor_limit_hits as u64;
        self.scatter_pack_ms += frame.scatter_pack_ms;
        self.scatter_gpu_bytes_peak = self.scatter_gpu_bytes_peak.max(frame.scatter_gpu_bytes);
        self.scatter_cpu_bytes_peak = self.scatter_cpu_bytes_peak.max(frame.scatter_cpu_bytes);
    }

    pub(super) fn json(&self) -> String {
        let config = UploadConfig::active();
        let mut json = format!(
            concat!(
                r#"{{"batching":{},"bytes_per_frame_limit":{},"calls_per_frame_limit":{},"#,
                r#""active_frames":{},"root_bytes":{},"arena_bytes":{},"#,
                r#""root_calls":{},"arena_calls":{},"arena_source_slices":{},"#,
                r#""byte_limited_frames":{},"call_limited_frames":{},"legacy_root_phase_frames":{},"#,
                r#""unused_bytes":{},"unused_calls":{},"peak_bytes":{},"peak_calls":{},"#,
                r#""publication_texture_copies":{}}}"#
            ),
            config.batch,
            config.bytes,
            config.calls,
            self.frames,
            self.root_bytes,
            self.arena_bytes,
            self.root_calls,
            self.arena_calls,
            self.arena_slices,
            self.byte_limited,
            self.call_limited,
            self.root_phase,
            self.unused_bytes,
            self.unused_calls,
            self.peak_bytes,
            self.peak_calls,
            self.publish_copies
        );
        json.pop();
        use std::fmt::Write;
        write!(
            json,
            concat!(
                r#","scatter":{},"descriptor_bytes":{},"scatter_descriptors":{},"scatter_dispatches":{},"#,
                r#""descriptor_limit_hits":{},"scatter_pack_cpu_ms":{},"#,
                r#""scatter_gpu_staging_bytes_peak":{},"scatter_cpu_pack_bytes_peak":{},"#,
                r#""transfer_bytes":{}}}"#
            ),
            config.scatter,
            self.descriptor_bytes,
            self.descriptor_bytes / scatter::DESCRIPTOR_BYTES as u64,
            self.scatter_dispatches,
            self.descriptor_limit_hits,
            self.scatter_pack_ms,
            self.scatter_gpu_bytes_peak,
            self.scatter_cpu_bytes_peak,
            self.root_bytes + self.arena_bytes + self.descriptor_bytes
        )
        .unwrap();
        json
    }
}

#[derive(Default)]
struct Cursor {
    root: usize,
    layer: usize,
    write: usize,
    offset: usize,
}

enum Upload {
    Scatter(scatter::Packet),
    Root {
        index: usize,
        first_layer: usize,
        layers: usize,
    },
    Arena {
        first: usize,
        last: usize,
        start: usize,
        bytes: usize,
    },
}

fn write_bytes(w: &Write) -> usize {
    w.data.len() * 4
}

impl Cursor {
    fn advance(
        &mut self,
        roots: &[Roots],
        writes: &[Write],
        config: UploadConfig,
        scatter_limits: scatter::Limits,
        mut submit: impl FnMut(Upload),
    ) -> (bool, UploadFrame) {
        let mut frame = UploadFrame::default();
        while frame.calls() < config.calls && frame.bytes() < config.bytes {
            let bytes_left = config.bytes - frame.bytes();
            if let Some(root) = roots.get(self.root) {
                let [x, y, z] = root.region.dims();
                let plane = x * y * 4;
                debug_assert!(plane <= config.bytes);
                let layers = (bytes_left / plane).min(z - self.layer);
                if layers != 0 {
                    submit(Upload::Root {
                        index: self.root,
                        first_layer: self.layer,
                        layers,
                    });
                    frame.root_calls += 1;
                    frame.root_bytes += layers * plane;
                    self.layer += layers;
                    if self.layer == z {
                        self.layer = 0;
                        self.root += 1;
                    } else {
                        // The remaining frame budget cannot fit another
                        // whole plane, even if some arena writes can fit.
                        frame.byte_limited = true;
                    }
                    if !config.batch && !config.scatter {
                        // Preserve the baseline's one-root-write phase, even
                        // when this was the final work in the transaction.
                        frame.root_phase = true;
                        break;
                    }
                    continue;
                }
                frame.byte_limited = true;
            }

            let Some(write) = writes.get(self.write) else {
                break;
            };
            if config.scatter {
                let Some(packet) = scatter::Packet::plan(
                    writes,
                    self.write,
                    self.offset,
                    bytes_left,
                    scatter_limits,
                ) else {
                    frame.byte_limited = true;
                    break;
                };
                let descriptors = packet.descriptors();
                frame.arena_bytes += packet.bytes;
                frame.descriptor_bytes += packet.transfer_bytes() - packet.bytes;
                frame.arena_slices += descriptors;
                frame.arena_calls += 1;
                frame.scatter_dispatches += 1;
                (self.write, self.offset) = if packet.end == write_bytes(&writes[packet.last]) {
                    (packet.last + 1, 0)
                } else {
                    (packet.last, packet.end)
                };
                if self.write != writes.len() && descriptors == scatter_limits.descriptors {
                    frame.descriptor_limit_hits += 1;
                }
                submit(Upload::Scatter(packet));
                continue;
            }
            let first = self.write;
            let start = self.offset;
            let mut last = first;
            let mut bytes = bytes_left.min(write_bytes(write) - start);
            if config.batch {
                while bytes < bytes_left {
                    let Some(next) = writes.get(last + 1) else {
                        break;
                    };
                    let prior = &writes[last];
                    if prior.kind != next.kind || prior.word + prior.data.len() != next.word {
                        break;
                    }
                    last += 1;
                    bytes += (bytes_left - bytes).min(write_bytes(next));
                }
            }
            submit(Upload::Arena {
                first,
                last,
                start,
                bytes,
            });
            frame.arena_calls += 1;
            frame.arena_slices += last - first + 1;
            frame.arena_bytes += bytes;
            let last_byte = write.word * 4 + start + bytes - writes[last].word * 4;
            (self.write, self.offset) = if last_byte == write_bytes(&writes[last]) {
                (last + 1, 0)
            } else {
                (last, last_byte)
            };
        }
        let work_left = self.root != roots.len() || self.write != writes.len();
        if work_left {
            frame.byte_limited |= frame.bytes() == config.bytes;
            frame.call_limited = !frame.root_phase && frame.calls() == config.calls;
        }
        (!work_left && !frame.root_phase, frame)
    }

    fn complete(&self, change: &Change) -> bool {
        self.root == change.roots.len() && self.write == change.writes.len()
    }
}

pub struct PendingShift {
    pub change: Change,
    textures: Vec<wgpu::Texture>,
    cursor: Cursor,
}
fn extent([x, y, z]: [usize; 3]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: y as u32,
        height: x as u32,
        depth_or_array_layers: z as u32,
    }
}
fn origin([x, y, z]: [usize; 3]) -> wgpu::Origin3d {
    wgpu::Origin3d {
        x: y as u32,
        y: x as u32,
        z: z as u32,
    }
}
impl PendingShift {
    pub fn new(gpu: &Gpu, mut change: Change) -> Self {
        if UploadConfig::active().batch {
            // Every write targets a newly reserved range. Ordering disjoint
            // writes by destination exposes adjacent runs from separate strips.
            change.writes.sort_unstable_by_key(|w| (w.kind, w.word));
            debug_assert!(change
                .writes
                .windows(2)
                .all(|w| w[0].kind != w[1].kind || w[0].word + w[0].data.len() <= w[1].word));
        }
        let textures = change
            .roots
            .iter()
            .map(|r| {
                gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("NAADF staged root strip"),
                    size: extent(r.region.dims()),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::R32Uint,
                    usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            })
            .collect();
        Self {
            change,
            textures,
            cursor: Cursor::default(),
        }
    }
    pub(super) fn advance(&mut self, gpu: &Gpu, volume: &Volume) -> (bool, UploadFrame) {
        let roots = &self.change.roots;
        let writes = &self.change.writes;
        let textures = &self.textures;
        let mut scatter_pack_ms = 0.;
        let (done, mut frame) = self.cursor.advance(
            roots,
            writes,
            UploadConfig::active(),
            volume
                .scatter
                .as_ref()
                .map_or_else(scatter::Limits::default, |s| s.limits),
            |upload| match upload {
                Upload::Scatter(packet) => {
                    scatter_pack_ms += volume
                        .scatter
                        .as_ref()
                        .expect("scatter upload resources are allocated for this volume")
                        .submit(&gpu.device, &gpu.queue, writes, &packet);
                }
                Upload::Root {
                    index,
                    first_layer,
                    layers,
                } => {
                    let r = &roots[index];
                    let [nx, ny, _] = r.region.dims();
                    let plane = nx * ny * 4;
                    let bytes = bytemuck::cast_slice::<u32, u8>(&r.nodes);
                    gpu.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &textures[index],
                            mip_level: 0,
                            origin: origin([0, 0, first_layer]),
                            aspect: wgpu::TextureAspect::All,
                        },
                        &bytes[first_layer * plane..(first_layer + layers) * plane],
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some((ny * 4) as u32),
                            rows_per_image: Some(nx as u32),
                        },
                        extent([nx, ny, layers]),
                    );
                }
                Upload::Arena {
                    first,
                    last,
                    start,
                    bytes,
                } => {
                    let w = &writes[first];
                    let buf = match w.kind {
                        0 => &volume.blocks,
                        1 => &volume.voxels,
                        _ => &volume.palette,
                    };
                    let dst = (w.word * 4 + start) as u64;
                    if first == last {
                        gpu.queue.write_buffer(
                            buf,
                            dst,
                            &bytemuck::cast_slice::<u32, u8>(&w.data)[start..start + bytes],
                        );
                    } else {
                        // Populate the queue staging allocation directly; do not
                        // create an intermediate copy of a merged upload.
                        let mut staging = gpu
                            .queue
                            .write_buffer_with(
                                buf,
                                dst,
                                wgpu::BufferSize::new(bytes as u64).unwrap(),
                            )
                            .expect("valid NAADF arena upload staging");
                        let mut copied = 0;
                        for (i, source) in writes[first..=last].iter().enumerate() {
                            let source = bytemuck::cast_slice::<u32, u8>(&source.data);
                            let start = if i == 0 { start } else { 0 };
                            let count = (bytes - copied).min(source.len() - start);
                            staging[copied..copied + count]
                                .copy_from_slice(&source[start..start + count]);
                            copied += count;
                        }
                        debug_assert_eq!(copied, bytes);
                    }
                }
            },
        );
        frame.scatter_pack_ms = scatter_pack_ms;
        if let Some(scatter) = &volume.scatter {
            frame.scatter_gpu_bytes = scatter.gpu_bytes();
            frame.scatter_cpu_bytes = scatter.cpu_bytes();
        }
        (done, frame)
    }
    pub fn publish(self, gpu: &Gpu, v: &mut Volume) -> (Change, u64) {
        assert!(
            self.cursor.complete(&self.change),
            "NAADF upload must finish before publication"
        );
        let mut copies = 0;
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("NAADF root strip publication"),
            });
        let n = v.n as usize;
        for (r, t) in self.change.roots.iter().zip(&self.textures) {
            let dims = r.region.dims();
            let parts: [Vec<(usize, usize, usize)>; 3] = std::array::from_fn(|a| {
                let dst = (r.region.lo[a] - v.bounds[0][a] as i64 + v.ring[a] as i64)
                    .rem_euclid(n as i64) as usize;
                split(dst, dims[a], n)
            });
            for &(sx, dx, nx) in &parts[0] {
                for &(sy, dy, ny) in &parts[1] {
                    for &(sz, dz, nz) in &parts[2] {
                        copies += 1;
                        encoder.copy_texture_to_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: t,
                                mip_level: 0,
                                origin: origin([sx, sy, sz]),
                                aspect: wgpu::TextureAspect::All,
                            },
                            wgpu::TexelCopyTextureInfo {
                                texture: &v._chunks,
                                mip_level: 0,
                                origin: origin([dx, dy, dz]),
                                aspect: wgpu::TextureAspect::All,
                            },
                            extent([nx, ny, nz]),
                        );
                    }
                }
            }
        }
        gpu.queue.submit(Some(encoder.finish()));
        v.bounds = self.change.region.bounds();
        v.ring = self.change.ring;
        if let Some(cache) = &mut v.root_distance {
            cache.retarget(self.change.region);
        }
        v.ownership = self.change.owner.clone();
        (self.change, copies)
    }
}
fn split(dst: usize, len: usize, n: usize) -> Vec<(usize, usize, usize)> {
    let first = len.min(n - dst);
    let mut out = vec![(0, dst, first)];
    if first < len {
        out.push((first, 0, len - first));
    }
    out
}
#[cfg(test)]
#[test]
fn wrapped_texture_copy_covers_each_slot_once() {
    assert_eq!(split(15, 4, 16), vec![(0, 15, 1), (1, 0, 3)]);
    assert_eq!(split(3, 6, 16), vec![(0, 3, 6)]);
}

#[cfg(test)]
mod schedule_tests {
    use super::*;
    use crate::naadf::stream::Region;

    fn roots(dims: [i64; 3]) -> Roots {
        Roots {
            region: Region {
                lo: [0; 3],
                hi: dims,
            },
            nodes: vec![0; dims.iter().product::<i64>() as usize],
        }
    }

    #[test]
    fn batching_and_baseline_transfer_identical_words_under_the_same_limits() {
        let roots = [roots([1, 3, 3]), roots([2, 2, 1])];
        let mut writes = [
            Write {
                kind: 1,
                word: 9,
                data: vec![90, 91],
            },
            Write {
                kind: 0,
                word: 8,
                data: vec![8, 9, 10, 11],
            },
            Write {
                kind: 0,
                word: 12,
                data: vec![12, 13, 14, 15],
            },
            Write {
                kind: 0,
                word: 16,
                data: vec![16],
            },
            Write {
                kind: 0,
                word: 17,
                data: vec![17],
            },
            Write {
                kind: 0,
                word: 18,
                data: vec![18],
            },
            Write {
                kind: 0,
                word: 19,
                data: vec![19],
            },
            // Real arena holes and switches to a different buffer must
            // stay separate even when there is byte and call headroom.
            Write {
                kind: 0,
                word: 21,
                data: vec![21],
            },
            Write {
                kind: 1,
                word: 22,
                data: vec![220],
            },
        ];
        let mut results = vec![];
        for batch in [false, true] {
            if batch {
                writes.sort_unstable_by_key(|w| (w.kind, w.word));
            }
            let expected_words: Vec<_> = writes
                .iter()
                .flat_map(|w| {
                    w.data
                        .iter()
                        .enumerate()
                        .map(|(i, &v)| (w.kind, w.word + i, v))
                })
                .collect();
            let mut cursor = Cursor::default();
            let mut root_layers = vec![];
            let mut arena_words = vec![];
            let mut frames = 0;
            let mut total_arena_calls = 0;
            let mut mixed_frame = false;
            let mut partial_merge = false;
            loop {
                let (done, frame) = cursor.advance(
                    &roots,
                    &writes,
                    UploadConfig {
                        batch,
                        scatter: false,
                        bytes: 24,
                        calls: 2,
                    },
                    scatter::Limits::default(),
                    |op| match op {
                        Upload::Scatter(_) => panic!("unexpected scatter in contiguous A/B"),
                        Upload::Root {
                            index,
                            first_layer,
                            layers,
                        } => {
                            root_layers.extend(
                                (first_layer..first_layer + layers).map(|layer| (index, layer)),
                            );
                        }
                        Upload::Arena {
                            first,
                            last,
                            start,
                            bytes,
                        } => {
                            let mut word = writes[first].word + start / 4;
                            let end = word + bytes / 4;
                            for w in &writes[first..=last] {
                                assert_eq!(w.kind, writes[first].kind);
                                assert!(w.word <= word && word < w.word + w.data.len());
                                for i in word..end.min(w.word + w.data.len()) {
                                    arena_words.push((w.kind, i, w.data[i - w.word]));
                                    word += 1;
                                }
                            }
                            assert_eq!(word, end);
                            partial_merge |=
                                first != last && end < writes[last].word + writes[last].data.len();
                        }
                    },
                );
                frames += 1;
                assert!(frames <= 10);
                assert!(frame.bytes() <= 24 && frame.calls() <= 2);
                total_arena_calls += frame.arena_calls;
                mixed_frame |= frame.root_calls != 0 && frame.arena_calls != 0;
                if done {
                    break;
                }
            }
            assert_eq!(root_layers, [(0, 0), (0, 1), (0, 2), (1, 0)]);
            let mut expected_words = expected_words;
            expected_words.sort_unstable();
            arena_words.sort_unstable();
            assert_eq!(arena_words, expected_words);
            assert_eq!(mixed_frame, batch);
            assert_eq!(partial_merge, batch);
            results.push((frames, total_arena_calls));
        }
        assert!(results[1].0 < results[0].0);
        assert!(results[1].1 < results[0].1);
    }

    #[test]
    fn legacy_root_phase_and_cap_deferrals_are_distinguished() {
        let roots = [roots([1, 2, 1])];
        let writes = [
            Write {
                kind: 0,
                word: 0,
                data: vec![1, 2],
            },
            Write {
                kind: 0,
                word: 3,
                data: vec![3, 4, 5],
            },
        ];
        let mut cursor = Cursor::default();
        let config = UploadConfig {
            batch: false,
            scatter: false,
            bytes: 16,
            calls: 1,
        };
        let (done, root) =
            cursor.advance(&roots, &writes, config, scatter::Limits::default(), |_| {});
        assert!(!done && root.root_phase && !root.call_limited && !root.byte_limited);
        assert_eq!(root.bytes(), 8);
        let (done, arena) =
            cursor.advance(&roots, &writes, config, scatter::Limits::default(), |_| {});
        assert!(!done && arena.call_limited && !arena.byte_limited && !arena.root_phase);
        assert_eq!(arena.bytes(), 8);
        let (done, arena) =
            cursor.advance(&roots, &writes, config, scatter::Limits::default(), |_| {});
        assert!(done && !arena.call_limited && !arena.byte_limited);
        let (done, idle) =
            cursor.advance(&roots, &writes, config, scatter::Limits::default(), |_| {
                panic!("finished upload ran again")
            });
        assert!(done && idle.calls() == 0);
    }
    #[test]
    fn scatter_scheduler_counts_queue_calls_instead_of_disjoint_ranges() {
        let roots = [roots([1, 2, 2])];
        let writes: Vec<_> = (0..200)
            .map(|i| Write {
                kind: (i % 3) as u8,
                word: i * 3,
                data: vec![i as u32],
            })
            .collect();
        let mut cursor = Cursor::default();
        let config = UploadConfig {
            batch: false,
            scatter: true,
            bytes: 4096,
            calls: 2,
        };
        let mut copied = Vec::new();
        let mut root_layers = Vec::new();
        let mut pack = Vec::new();
        let (done, frame) = cursor.advance(
            &roots,
            &writes,
            config,
            scatter::Limits::default(),
            |op| match op {
                Upload::Root {
                    index,
                    first_layer,
                    layers,
                } => {
                    root_layers.extend((first_layer..first_layer + layers).map(|z| (index, z)));
                }
                Upload::Scatter(packet) => {
                    packet.pack(&writes, &mut pack);
                    for d in pack[..packet.descriptors() * 4].chunks_exact(4) {
                        copied.push((d[0] as u8, d[1] as usize, pack[d[2] as usize]));
                    }
                }
                Upload::Arena { .. } => panic!("scatter must replace individual arena writes"),
            },
        );
        assert!(done);
        assert_eq!(root_layers, [(0, 0), (0, 1)]);
        assert_eq!(copied.len(), 200);
        assert_eq!(
            copied,
            writes
                .iter()
                .map(|w| (w.kind, w.word, w.data[0]))
                .collect::<Vec<_>>()
        );
        assert_eq!(frame.root_calls, 1);
        assert_eq!(frame.arena_calls, 1);
        assert_eq!(frame.scatter_dispatches, 1);
        assert_eq!(frame.arena_slices, 200);
        assert_eq!(frame.descriptor_bytes, 200 * scatter::DESCRIPTOR_BYTES);
        assert_eq!(frame.bytes(), 16 + 200 * 20);
        assert!(frame.bytes() <= config.bytes);
    }

    #[test]
    fn scatter_scheduler_obeys_total_transfer_and_descriptor_caps_across_frames() {
        let writes: Vec<_> = (0..12)
            .map(|i| Write {
                kind: (i % 3) as u8,
                word: i * 9,
                data: vec![i as u32; 5],
            })
            .collect();
        let mut cursor = Cursor::default();
        let config = UploadConfig {
            batch: false,
            scatter: true,
            bytes: 92,
            calls: 2,
        };
        let limits = scatter::Limits {
            packet_bytes: 64,
            descriptors: 2,
        };
        let mut expected = Vec::new();
        for w in &writes {
            for (j, &value) in w.data.iter().enumerate() {
                expected.push((w.kind, w.word + j, value));
            }
        }
        let mut copied = Vec::new();
        let mut frames = 0;
        let mut pack = Vec::new();
        loop {
            let (done, frame) = cursor.advance(&[], &writes, config, limits, |op| {
                let Upload::Scatter(packet) = op else {
                    panic!("unexpected direct upload")
                };
                packet.pack(&writes, &mut pack);
                assert!(packet.transfer_bytes() <= limits.packet_bytes);
                assert!(packet.descriptors() <= limits.descriptors);
                for d in pack[..packet.descriptors() * 4].chunks_exact(4) {
                    for j in 0..d[3] {
                        copied.push((d[0] as u8, (d[1] + j) as usize, pack[(d[2] + j) as usize]));
                    }
                }
            });
            frames += 1;
            assert!(frames <= 20);
            assert!(frame.bytes() <= config.bytes && frame.calls() <= config.calls);
            assert_eq!(frame.calls(), frame.scatter_dispatches);
            if done {
                break;
            }
        }
        assert_eq!(copied, expected);
        let (done, idle) =
            cursor.advance(&[], &writes, config, limits, |_| panic!("repeated work"));
        assert!(done && idle.calls() == 0);
    }
}
