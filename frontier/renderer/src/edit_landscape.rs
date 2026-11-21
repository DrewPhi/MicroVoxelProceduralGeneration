//! Persistent edited columns outside the moving native volume. Work scales with
//! material transitions, so a tall bridge never requires sampling the air below.
use crate::adaptive_geometry::NativeQuad;
use frontier_world::{voxel_size, Cell, Material, World, WorldColumn};
use std::collections::{BTreeMap, BTreeSet};

type Xz = [i64; 2];
const NEIGHBORS: [(usize, i64); 4] = [(0, -1), (0, 1), (1, -1), (1, 1)];

fn neighbor(mut p: Xz, axis: usize, step: i64) -> Xz {
    p[axis] = p[axis].saturating_add(step);
    p
}

struct Column<'a> {
    source: WorldColumn<'a>,
    breaks: Vec<i64>,
}

pub(super) struct Geometry {
    pub origin: Cell,
    pub quads: Vec<NativeQuad>,
    pub columns: BTreeSet<Xz>,
    radius: i64,
}

// Keep rectangle boundaries in integer cells until publication. This also
// preserves adjacency when the selected voxel size has no exact f32 encoding.
#[derive(Clone, Copy)]
struct Face {
    at: [i64; 3],
    size: [i64; 2],
    direction: u32,
    material: Material,
}
impl Face {
    fn new(p: Xz, y: i64, height: i64, axis: usize, negative: bool, material: Material) -> Self {
        let mut at = [p[0], y, p[1]];
        if !negative && axis != 1 {
            at[axis] = at[axis].saturating_add(1);
        }
        let size = match axis {
            0 => [height, 1],
            2 => [1, height],
            _ => [1, 1],
        };
        Self {
            at,
            size,
            direction: axis as u32 | (u32::from(negative) << 2),
            material,
        }
    }
    fn native(self, origin: Cell) -> NativeQuad {
        let cell = voxel_size();
        let base = [origin.x, origin.y, origin.z];
        NativeQuad {
            anchor: std::array::from_fn(|i| {
                ((self.at[i] as i128 - base[i] as i128) as f64 * cell) as f32
            }),
            extent: self.size.map(|s| (s as f64 * cell) as f32),
            axis_sign: self.direction,
            material: self.material as u32,
            profile_index: 0,
        }
    }
}

// Merge coplanar runs first along one tangent and then along the other. Faces
// only join when the plane, orientation, material and entire shared edge agree.
fn merge_faces(faces: &mut Vec<Face>) {
    for along in 0..2 {
        let across = 1 - along;
        let key = |f: &Face| {
            let axis = (f.direction & 3) as usize;
            (
                f.direction,
                f.material as u32,
                f.at[axis],
                f.at[(axis + 1 + across) % 3],
                f.size[across],
            )
        };
        faces
            .sort_unstable_by_key(|f| (key(f), f.at[((f.direction & 3) as usize + 1 + along) % 3]));
        faces.dedup_by(|next, prev| {
            let a = ((prev.direction & 3) as usize + 1 + along) % 3;
            if key(prev) == key(next)
                && prev.at[a].checked_add(prev.size[along]) == Some(next.at[a])
            {
                if let Some(length) = prev.size[along].checked_add(next.size[along]) {
                    prev.size[along] = length;
                    return true;
                }
            }
            false
        });
    }
}

pub(super) fn build(world: &World, origin: Cell, radius: i64) -> Geometry {
    let mut reach = radius;
    loop {
        if let Some(data) = build_within(world, origin, reach) {
            return data;
        }
        if reach == 0 {
            return Geometry {
                origin,
                columns: BTreeSet::new(),
                quads: Vec::new(),
                radius: 0,
            };
        }
        reach /= 2;
    }
}

// Bound background scratch and uploads for very large saves. Shrink the
// neighborhood as a whole so every published column has its complete topology.
fn build_within(world: &World, origin: Cell, radius: i64) -> Option<Geometry> {
    let mut result = Geometry {
        origin,
        columns: BTreeSet::new(),
        quads: Vec::new(),
        radius,
    };
    let min = [origin.x, origin.z].map(|v| v.saturating_sub(radius));
    let max = [origin.x, origin.z].map(|v| v.saturating_add(radius).saturating_add(1));
    // Bound total saved edits and the published mask independently of the
    // temporary column neighborhood used by each extraction batch.
    let mut addresses = world.terrain_edit_surface_candidates(
        min.map(|v| v.saturating_sub(3)),
        max.map(|v| v.saturating_add(3)),
        524288,
    )?;
    addresses.sort_unstable_by_key(|c| (c.x, c.z, c.y));
    for c in &addresses {
        let p = [c.x, c.z];
        if (0..2).all(|a| min[a] <= p[a] && p[a] < max[a]) {
            result.columns.insert(p);
            for (axis, step) in NEIGHBORS {
                result.columns.insert(neighbor(p, axis, step));
            }
        }
    }
    if result.columns.len() > 262144 {
        return None;
    }
    let mut all_faces = Vec::new();
    let mut selected = result.columns.iter().copied();
    loop {
        let batch: Vec<_> = selected.by_ref().take(2048).collect();
        if batch.is_empty() {
            break;
        }
        let mut samples = BTreeMap::new();
        for &p in &batch {
            for q in std::iter::once(p).chain(NEIGHBORS.map(|(a, s)| neighbor(p, a, s))) {
                samples.entry(q).or_insert_with(|| {
                    let source = world.column(q[0], q[1]);
                    let ground = source.ground_height_cells();
                    let start = addresses.partition_point(|c| [c.x, c.z] < q);
                    let mut breaks: Vec<i64> = addresses[start..]
                        .iter()
                        .take_while(|c| [c.x, c.z] == q)
                        .flat_map(|c| [c.y, c.y.saturating_add(1)])
                        .collect();
                    breaks.extend([
                        ground.saturating_sub(12),
                        ground.saturating_sub(1),
                        ground,
                        source.height_cells(),
                    ]);
                    breaks.sort_unstable();
                    breaks.dedup();
                    // Collapse contiguous edits into material runs before comparing
                    // neighbors. A hundred-cell timber post needs just four sides.
                    breaks.retain(|&y| {
                        source.material_without_fluid(y)
                            != source.material_without_fluid(y.saturating_sub(1))
                    });
                    Column { source, breaks }
                });
            }
        }
        let mut faces = Vec::new();
        for p in batch {
            if faces.len() > 524288 {
                return None;
            }
            let col = &samples[&p];
            for &y in &col.breaks {
                let below = col.source.material_without_fluid(y.saturating_sub(1));
                let above = col.source.material_without_fluid(y);
                if below != Material::Air && above == Material::Air {
                    faces.push(Face::new(p, y, 1, 1, false, below));
                } else if above != Material::Air && below == Material::Air {
                    faces.push(Face::new(p, y, 1, 1, true, above));
                }
            }
            for (xz, step) in NEIGHBORS {
                let q = neighbor(p, xz, step);
                let other = &samples[&q];
                let mut breaks = col.breaks.clone();
                breaks.extend_from_slice(&other.breaks);
                breaks.sort_unstable();
                breaks.dedup();
                let axis = xz * 2;
                for interval in breaks.windows(2) {
                    let y = interval[0];
                    let h = interval[1].saturating_sub(y);
                    let mat = col.source.material_without_fluid(y);
                    let adjacent = other.source.material_without_fluid(y);
                    if mat != Material::Air && adjacent == Material::Air {
                        faces.push(Face::new(p, y, h, axis, step < 0, mat));
                    } else if mat == Material::Air
                        && adjacent != Material::Air
                        && !result.columns.contains(&q)
                    {
                        // At the footprint boundary the neighboring solid can own
                        // the inward-facing wall of a cut in an edited column.
                        faces.push(Face::new(q, y, h, axis, step > 0, adjacent));
                    }
                }
            }
        }
        merge_faces(&mut faces);
        if all_faces.len() + faces.len() > 524288 {
            return None;
        }
        all_faces.append(&mut faces);
    }
    merge_faces(&mut all_faces);
    result.quads = all_faces
        .into_iter()
        .map(|face| face.native(origin))
        .collect();
    Some(result)
}

// Eight by eight native columns share one occupancy word pair. The key uses
// low native cell bits, matching each adaptive tile's integer paint origin.
// The snapshot covers a bounded camera neighborhood smaller than one wrap.
fn hash(x: u32, z: u32) -> u32 {
    let v = x.wrapping_mul(0x9e3779b1) ^ z.wrapping_mul(0x85ebca77);
    v ^ (v >> 16)
}

pub(super) fn mask(columns: &BTreeSet<Xz>) -> ([u32; 2], Vec<[u32; 4]>) {
    let mut tiles = BTreeMap::<[u32; 2], [u32; 2]>::new();
    let mut low = [i64::MAX; 2];
    let mut high = [i64::MIN; 2];
    for &[x, z] in columns {
        let [cx, cz] = [x as u32, z as u32];
        let bit = (cx & 7) + (cz & 7) * 8;
        tiles.entry([cx >> 3, cz >> 3]).or_default()[(bit / 32) as usize] |= 1 << (bit & 31);
        for (a, v) in [x, z].into_iter().enumerate() {
            low[a] = low[a].min(v);
            high[a] = high[a].max(v);
        }
    }
    // The final row stores the bounding rectangle. A nearly square hash
    // table handles narrow builds without exceeding texture dimension limits.
    let capacity = (tiles.len() * 2).max(1).next_power_of_two();
    let width = 1usize << (capacity.ilog2() / 2);
    let mut words = vec![[0u32; 4]; capacity + width];
    if !columns.is_empty() {
        words[capacity] = [
            low[0] as u32,
            low[1] as u32,
            (high[0] - low[0]) as u32,
            (high[1] - low[1]) as u32,
        ];
    }
    for ([x, z], bits) in tiles {
        let mut index = hash(x, z) as usize & (capacity - 1);
        while words[index][2] | words[index][3] != 0 {
            index = (index + 1) & (capacity - 1);
        }
        words[index] = [x, z, bits[0], bits[1]];
    }
    ([width as u32, (words.len() / width) as u32], words)
}

use super::{adaptive_shadow::LightFrame, AdaptiveGpu};
use crate::Camera;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use wgpu::util::DeviceExt;

pub(crate) fn mask_texture(device: &wgpu::Device, dims: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("edited terrain column footprint"),
        size: wgpu::Extent3d {
            width: dims[0],
            height: dims[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Uint,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

struct Request {
    world: World,
    origin: Cell,
    radius: i64,
    refresh_distance: u64,
}
impl Request {
    fn covers(&self, world: &World, pos: Cell, view: i64) -> bool {
        self.world.shares_terrain_edits(world)
            && self.radius >= view
            && [self.origin.x.abs_diff(pos.x), self.origin.z.abs_diff(pos.z)]
                .into_iter()
                .all(|d| d <= ((self.radius - view) as u64).min(self.refresh_distance))
    }
}
struct Drawable {
    origin: glam::DVec3,
    cell: Cell,
    faces: wgpu::Buffer,
    mask: wgpu::TextureView,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    count: u32,
    bytes: u64,
    columns: usize,
}
pub(crate) struct HorizonEdits<'a> {
    pub origin: glam::DVec3,
    pub cell: Cell,
    pub faces: &'a wgpu::Buffer,
    pub mask: &'a wgpu::TextureView,
    pub count: u32,
}
#[derive(Default)]
pub(super) struct Surface {
    published: Option<Request>,
    pending: Option<(Request, Receiver<Geometry>)>,
    draw: Option<Drawable>,
    published_radius: i64,
}

impl AdaptiveGpu {
    pub(crate) fn horizon_edits(&self) -> Option<HorizonEdits<'_>> {
        let draw = self.edits.draw.as_ref()?;
        Some(HorizonEdits {
            origin: draw.origin,
            cell: draw.cell,
            faces: &draw.faces,
            mask: &draw.mask,
            count: draw.count,
        })
    }
    pub(crate) fn settle_edit_surface(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        camera: Camera,
        horizon: bool,
    ) -> crate::Result<()> {
        let start = std::time::Instant::now();
        loop {
            self.update_edit_surface(device, queue, world, camera, horizon);
            if !self.edit_surface_pending() {
                return Ok(());
            }
            if start.elapsed().as_secs() >= 60 {
                return Err("terrain edit surface preparation timed out".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    pub(crate) fn edit_surface_pending(&self) -> bool {
        self.edits.pending.is_some()
    }

    pub(crate) fn update_edit_surface(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        camera: Camera,
        horizon: bool,
    ) {
        if world.fbm_terrain().is_none()
            || world.topology_base_cells().is_some()
            || world.terrain_edits().next().is_none()
        {
            self.edits.published = None;
            self.edits.published_radius = 0;
            if self.edits.draw.take().is_some() {
                self.set_edit_mask(
                    device,
                    queue,
                    mask_texture(device, [1, 1]).create_view(&Default::default()),
                );
            }
            // Retain at most one running build when edits are undone mid-job.
            if self
                .edits
                .pending
                .as_ref()
                .is_some_and(|(_, rx)| !matches!(rx.try_recv(), Err(TryRecvError::Empty)))
            {
                self.edits.pending = None;
            }
            return;
        }
        let pos = Cell::from_meters(camera.position.to_array());
        // Perspective frustum corners can lie beyond the forward far distance.
        // Keep sparse ownership local to preserve native precision after travel.
        let reach_m = (camera.far as f64 * 4.).max(if horizon { 35000. } else { 0. });
        let view = (reach_m / voxel_size()).ceil().clamp(1., (1 << 23) as f64) as i64;
        // Avoid periodic-address aliases when teleporting away from an old world.
        if self.edits.published.as_ref().is_some_and(|r| {
            !world.shares_source_lineage(&r.world)
                || r.origin.x.abs_diff(pos.x) > (1 << 25)
                || r.origin.z.abs_diff(pos.z) > (1 << 25)
        }) {
            self.edits.published = None;
            self.edits.published_radius = 0;
            self.edits.draw = None;
            self.set_edit_mask(
                device,
                queue,
                mask_texture(device, [1, 1]).create_view(&Default::default()),
            );
        }
        let ready = self.edits.pending.as_ref().map(|(_, rx)| rx.try_recv());
        match ready {
            Some(Ok(data)) => {
                let (mut request, _) = self.edits.pending.take().unwrap();
                if request.covers(world, pos, view) {
                    request.refresh_distance = ((data.radius / 4).max(1)) as u64;
                    self.edits.published_radius = data.radius;
                    self.publish_edit_surface(device, queue, data);
                    self.edits.published = Some(request);
                }
            }
            Some(Err(TryRecvError::Empty)) => return,
            Some(Err(TryRecvError::Disconnected)) => self.edits.pending = None,
            None => (),
        }
        if self
            .edits
            .published
            .as_ref()
            .is_some_and(|r| r.covers(world, pos, view))
        {
            return;
        }
        let radius = view + (view / 4).max(64);
        let request = Request {
            world: world.clone(),
            origin: pos,
            radius,
            refresh_distance: u64::MAX,
        };
        let snapshot = world.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = tx.send(build(&snapshot, pos, radius));
        });
        self.edits.pending = Some((request, rx));
    }

    fn publish_edit_surface(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: Geometry) {
        if data.columns.is_empty() || data.quads.is_empty() {
            if self.edits.draw.take().is_some() {
                self.set_edit_mask(
                    device,
                    queue,
                    mask_texture(device, [1, 1]).create_view(&Default::default()),
                );
            }
            return;
        }
        let (size, words) = mask(&data.columns);
        let texture = mask_texture(device, size);
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&words),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 16),
                rows_per_image: None,
            },
            texture.size(),
        );
        let faces = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("persistent terrain edit faces"),
            contents: bytemuck::cast_slice(&data.quads),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain edit view"),
            size: 768,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = super::profile_bind(device, &self.layout, &uniform, &self.shadows.zero_table);
        let cell = data.origin;
        let origin =
            glam::DVec3::from_array([cell.x, cell.y, cell.z].map(|v| v as f64 * voxel_size()));
        let bytes = faces.size() + uniform.size() + words.len() as u64 * 16;
        let mask_view = texture.create_view(&Default::default());
        self.edits.draw = Some(Drawable {
            origin,
            cell,
            faces,
            mask: mask_view.clone(),
            uniform,
            bind,
            count: data.quads.len() as u32,
            bytes,
            columns: data.columns.len(),
        });
        self.set_edit_mask(device, queue, mask_view);
    }

    pub(crate) fn prepare_edit_draw(&self, queue: &wgpu::Queue, camera: Camera, aspect: f32) {
        let Some(d) = &self.edits.draw else { return };
        let matrix = camera.matrix(d.origin, aspect);
        let eye = (camera.position - d.origin).as_vec3().extend(1.);
        let flags = self.voxel_filter as u32 | 2 | (u32::from(self.lighting[11] > 0.5) << 2) | 16;
        let paint = [d.cell.x as u32, d.cell.y as u32, d.cell.z as u32, flags];
        let mut lighting = self.lighting;
        lighting[11] = 0.;
        let mut bytes = [0u8; 208];
        bytes[..64].copy_from_slice(bytemuck::cast_slice(&matrix.to_cols_array()));
        bytes[64..80].copy_from_slice(bytemuck::cast_slice(&eye.to_array()));
        bytes[80..96].copy_from_slice(bytemuck::cast_slice(&paint));
        bytes[96..144].copy_from_slice(bytemuck::cast_slice(&lighting));
        bytes[144..].copy_from_slice(bytemuck::cast_slice(
            &LightFrame::new(&camera, self.lighting)
                .matrix(d.origin)
                .to_cols_array(),
        ));
        queue.write_buffer(&d.uniform, 0, &bytes);
        if self.shadows_active() {
            queue.write_buffer(&d.uniform, 256, &bytes[144..208]);
            if self.shadow_cascades_active() {
                let near = super::adaptive_shadow::near_light_matrix(&camera, self.lighting)
                    * glam::Mat4::from_translation((d.origin - camera.position).as_vec3());
                queue.write_buffer(&d.uniform, 512, bytemuck::cast_slice(&near.to_cols_array()));
            }
        }
    }
    pub(super) fn draw_edits<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        let Some(d) = &self.edits.draw else { return };
        pass.set_pipeline(&self.native_pipeline);
        pass.set_bind_group(0, &d.bind, &[0]);
        self.draw_edit_quads(pass, d);
    }
    pub(super) fn draw_edit_shadows<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        pipeline: &'a wgpu::RenderPipeline,
        near: bool,
    ) {
        let Some(d) = &self.edits.draw else { return };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &d.bind, &[if near { 512 } else { 256 }]);
        self.draw_edit_quads(pass, d);
    }
    fn draw_edit_quads<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, d: &'a Drawable) {
        pass.set_vertex_buffer(0, d.faces.slice(..));
        pass.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..6, 0, 0..d.count);
    }
    pub(crate) fn edit_surface_json(&self) -> String {
        let (columns, quads) = self
            .edits
            .draw
            .as_ref()
            .map_or((0, 0), |d| (d.columns, d.count));
        let limited = self
            .edits
            .published
            .as_ref()
            .is_some_and(|r| self.edits.published_radius < r.radius);
        format!(
            r#"{{"columns":{columns},"quads":{quads},"gpu_bytes":{},"pending":{},"budget_limited":{limited},"radius_m":{}}}"#,
            self.edit_bytes(),
            self.edit_surface_pending(),
            self.edits.published_radius as f64 * voxel_size()
        )
    }
    pub(super) fn edit_bytes(&self) -> u64 {
        self.edits.draw.as_ref().map_or(0, |d| d.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::{FbmParams, FbmRecipe};

    #[test]
    fn sparse_faces_match_native_extraction_for_cuts_and_a_floating_bridge() {
        let mut world = World::fbm(
            19,
            FbmParams {
                recipe: FbmRecipe::Regional,
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let origin = Cell { x: -3, y: 12, z: 7 };
        for x in -6..1 {
            world.set_batch(
                (-4..0).flat_map(|y| (2..6).map(move |z| (Cell { x, y, z }, Material::Air))),
            );
            world.set_batch(
                (3..5).flat_map(|y| (1..4).map(move |z| (Cell { x, y, z }, Material::Wood))),
            );
        }
        let mesh = build(&world, origin, 64);
        let mut actual = BTreeSet::new();
        for q in mesh.quads {
            let axis = (q.axis_sign & 3) as usize;
            let neg = q.axis_sign & 4 != 0;
            let mut base = q.anchor.map(|v| (v as f64 / voxel_size()).round() as i64);
            for (i, v) in [origin.x, origin.y, origin.z].into_iter().enumerate() {
                base[i] += v;
            }
            if !neg {
                base[axis] -= 1;
            }
            let extent = q.extent.map(|v| (v as f64 / voxel_size()).round() as i64);
            for j in 0..extent[1] {
                for i in 0..extent[0] {
                    let mut p = base;
                    p[(axis + 1) % 3] += i;
                    p[(axis + 2) % 3] += j;
                    assert!(actual.insert((p, q.axis_sign, q.material)));
                }
            }
        }
        let mut expected = BTreeSet::new();
        for x in -9..4 {
            for z in -1..9 {
                for y in -7..8 {
                    let cell = Cell { x, y, z };
                    let material = world.material_without_fluid(cell);
                    if material == Material::Air {
                        continue;
                    }
                    for axis in 0..3 {
                        for step in [-1, 1] {
                            let mut n = [x, y, z];
                            n[axis] += step;
                            if (mesh.columns.contains(&[x, z])
                                || mesh.columns.contains(&[n[0], n[2]]))
                                && world.material_without_fluid(Cell {
                                    x: n[0],
                                    y: n[1],
                                    z: n[2],
                                }) == Material::Air
                            {
                                expected.insert((
                                    [x, y, z],
                                    axis as u32 | (u32::from(step < 0) << 2),
                                    material as u32,
                                ));
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_runway_beyond_the_native_window_keeps_its_whole_deck_and_soffit() {
        let mut world = World::fbm(
            7,
            FbmParams {
                recipe: FbmRecipe::Regional,
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        // 100 m by 6 m of individual 6.25 cm edits in one suspended course.
        world.set_batch(
            (0..1600).flat_map(|x| (0..96).map(move |z| (Cell { x, y: 47, z }, Material::Wood))),
        );
        let origin = Cell {
            x: -2000,
            y: 500,
            z: 48,
        };
        let geometry = build(&world, origin, 8192);
        assert_eq!(geometry.radius, 8192);
        assert!(geometry.columns.contains(&[1599, 95]) && geometry.columns.contains(&[0, 0]));
        let wood: Vec<_> = geometry
            .quads
            .iter()
            .filter(|q| q.material == Material::Wood as u32)
            .collect();
        assert_eq!(wood.len(), 6);
        for dir in [1, 5] {
            let face = wood.iter().find(|q| q.axis_sign == dir).unwrap();
            assert_eq!(face.extent, [96., 1600.].map(|n| (n * voxel_size()) as f32));
        }
    }

    #[test]
    fn a_dense_suspended_structure_keeps_its_full_requested_radius() {
        let mut world = World::fbm(
            7,
            FbmParams {
                recipe: FbmRecipe::Regional,
                amplitude: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        // More than half a million native cells form one solid with two courses.
        world.restore_terrain_edits((0..384).flat_map(|x| {
            (0..224).flat_map(move |y| {
                (0..8).map(move |z| {
                    (
                        Cell {
                            x: x - 192,
                            y: y + 120,
                            z: z - 32000,
                        },
                        if y < 192 {
                            Material::Wood
                        } else {
                            Material::Snow
                        },
                    )
                })
            })
        }));
        let mesh = build(&world, Cell::default(), 65536);
        assert_eq!(mesh.radius, 65536);
        assert!(mesh.columns.contains(&[-192, -32000]) && mesh.columns.contains(&[191, -31993]));
        let count = |m: Material| mesh.quads.iter().filter(|q| q.material == m as u32).count();
        // Four walls for each material, the wooden underside and the snow roof.
        assert_eq!((count(Material::Wood), count(Material::Snow)), (5, 5));
    }

    #[test]
    fn sparse_footprints_preserve_negative_coordinates_and_empty_buckets() {
        let mut owned = BTreeSet::new();
        for x in -43..23 {
            for z in -18..18 {
                if (x * 7 + z * 11) % 9 == 0 {
                    owned.insert([x, z]);
                }
            }
        }
        let (dims, data) = mask(&owned);
        let capacity = dims[0] * (dims[1] - 1);
        for x in -51i64..51 {
            for z in -23i64..24 {
                let [cx, cz] = [x as u32, z as u32];
                let mut idx = hash(cx >> 3, cz >> 3) & (capacity - 1);
                let found = loop {
                    let word = data[idx as usize];
                    if word[2] | word[3] == 0 {
                        break false;
                    }
                    if word[..2] == [cx >> 3, cz >> 3] {
                        let bit = (cx & 7) + (cz & 7) * 8;
                        break word[2 + (bit / 32) as usize] & (1 << (bit & 31)) != 0;
                    }
                    idx = (idx + 1) & (capacity - 1);
                };
                assert_eq!(found, owned.contains(&[x, z]));
            }
        }
    }
}
