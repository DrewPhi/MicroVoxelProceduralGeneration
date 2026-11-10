//! Upload unreachable arena slots and a temporary root strip, then publish
//! the new root mapping in one queue-ordered texture copy transaction.
use super::{stream::Change, Gpu, Volume};
pub struct PendingShift {
    pub change: Change,
    textures: Vec<wgpu::Texture>,
    root: usize,
    write: usize,
    offset: usize,
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
    pub fn new(gpu: &Gpu, change: Change) -> Self {
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
            root: 0,
            write: 0,
            offset: 0,
        }
    }
    pub fn advance(&mut self, gpu: &Gpu, volume: &Volume) -> bool {
        let allowance = 8 * 1048576;
        if let Some(r) = self.change.roots.get(self.root) {
            let [nx, ny, nz] = r.region.dims();
            let plane = nx * ny * 4;
            let layers = (allowance / plane).max(1).min(nz - self.offset);
            let bytes = bytemuck::cast_slice::<u32, u8>(&r.nodes);
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[self.root],
                    mip_level: 0,
                    origin: origin([0, 0, self.offset]),
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes[self.offset * plane..(self.offset + layers) * plane],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some((ny * 4) as u32),
                    rows_per_image: Some(nx as u32),
                },
                extent([nx, ny, layers]),
            );
            self.offset += layers;
            if self.offset == nz {
                self.offset = 0;
                self.root += 1;
            }
            return false;
        }
        let mut left = allowance;
        for _ in 0..64 {
            let Some(w) = self.change.writes.get(self.write) else {
                return true;
            };
            let buf = match w.kind {
                0 => &volume.blocks,
                1 => &volume.voxels,
                _ => &volume.palette,
            };
            let bytes = bytemuck::cast_slice::<u32, u8>(&w.data);
            let count = left.min(bytes.len() - self.offset);
            gpu.queue.write_buffer(
                buf,
                (w.word * 4 + self.offset) as u64,
                &bytes[self.offset..self.offset + count],
            );
            self.offset += count;
            left -= count;
            if self.offset == bytes.len() {
                self.offset = 0;
                self.write += 1;
            }
            if left == 0 {
                break;
            }
        }
        self.write == self.change.writes.len()
    }
    pub fn publish(self, gpu: &Gpu, v: &mut Volume) -> Change {
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
        v.ownership = self.change.owner.clone();
        self.change
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
