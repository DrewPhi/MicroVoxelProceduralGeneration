//! Camera-independent, source-derived grass/flower appearance pages. Native
//! models remain authoritative. This bounded display estimator keeps opacity
//! separate from face-area lighting: hidden leaf faces cannot add opacity.
use crate::{adaptive_geometry::GeometryTile, spatial_appearance::Profile};
use frontier_world::{
    vegetation::{self, Species},
    Material, World, VOXEL_SIZE,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::Instant,
};
const EDGE: i64 = 16;
const HEIGHT: usize = 16;
const CAPACITY: usize = 262144;
pub(crate) const VERSION: u64 = 4;
fn records() -> usize {
    if directional_enabled() {
        10
    } else {
        2
    }
}
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);
static NANOS: AtomicU64 = AtomicU64::new(0);
pub(crate) fn enabled() -> bool {
    std::env::var("FRONTIER_GROUND_COVER").as_deref() != Ok("off")
}
pub(crate) fn directional_enabled() -> bool {
    enabled() && std::env::var("FRONTIER_GROUND_COVER_DIRECTIONAL").as_deref() == Ok("on")
}
pub(crate) fn shell_enabled() -> bool {
    enabled() && std::env::var("FRONTIER_GROUND_COVER_SHELL").as_deref() == Ok("on")
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Summary {
    pub data: [Profile; 2],
    directional: Option<Box<[Profile; 8]>>,
}
impl Default for Summary {
    fn default() -> Self {
        Self {
            data: [[[0.; 4]; 5]; 2],
            directional: None,
        }
    }
}
impl Summary {
    fn add(&mut self, b: &Self, weight: f32) {
        for i in 0..2 {
            for j in 0..5 {
                for k in 0..4 {
                    self.data[i][j][k] += b.data[i][j][k] * weight;
                }
            }
        }
        if let Some(b) = &b.directional {
            let a = self
                .directional
                .get_or_insert_with(|| Box::new([[[0.; 4]; 5]; 8]));
            for i in 0..8 {
                for j in 0..5 {
                    for k in 0..4 {
                        a[i][j][k] += b[i][j][k] * weight;
                    }
                }
            }
        }
    }
    fn coverage(&self) -> f32 {
        self.data[1][1]
            .iter()
            .chain(self.data[1][2].iter())
            .copied()
            .fold(self.data[1][4][0], f32::max)
    }
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct Key {
    source: [u64; 8],
    x: i64,
    z: i64,
    level: u8,
    edits: u64,
}
#[derive(Default)]
struct Cache {
    map: HashMap<Key, Summary>,
    fifo: VecDeque<Key>,
}
fn cache() -> &'static Mutex<Cache> {
    static VALUE: OnceLock<Mutex<Cache>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(Cache::default()))
}
fn source(w: &World) -> [u64; 8] {
    let p = w.fbm_terrain().map(|t| t.params()).unwrap_or_default();
    [
        w.seed(),
        vegetation::VEGETATION_SOURCE_VERSION as u64 | ((directional_enabled() as u64) << 32),
        p.recipe as u64,
        p.horizontal_scale.to_bits(),
        p.amplitude.to_bits(),
        p.octaves as u64,
        p.roughness.to_bits(),
        VERSION
            | ((w.fbm_terrain().is_some() as u64) << 32)
            | ((w.topology_base_cells().is_some() as u64) << 33),
    ]
}
pub(crate) fn stats_json() -> String {
    let c = cache().lock().unwrap();
    let payload =
        c.map.len() * 160 + c.map.values().filter(|v| v.directional.is_some()).count() * 640;
    format!("{{\"directional\":{},\"enabled\":{},\"version\":{},\"entries\":{},\"hits\":{},\"misses\":{},\"leaf_build_ms\":{},\"payload_bytes\":{},\"capacity_entries\":{},\"shell\":{}}}",directional_enabled(),enabled(),VERSION,c.map.len(),HITS.load(Ordering::Relaxed),MISSES.load(Ordering::Relaxed),NANOS.load(Ordering::Relaxed)as f64/1e6,payload,CAPACITY,shell_enabled())
}
fn node(w: &World, mut key: Key, cancel: &mut impl FnMut() -> bool) -> Result<Summary, String> {
    if cancel() {
        return Err("cancelled ground-cover page".into());
    }
    if w.revision() != 0 {
        let span = EDGE * (1i64 << key.level);
        let min = [key.x * span - 8, key.z * span - 8];
        let max = [(key.x + 1) * span + 8, (key.z + 1) * span + 8];
        key.edits = w
            .edit_cells()
            .filter(|c| c.x >= min[0] && c.x < max[0] && c.z >= min[1] && c.z < max[1])
            .fold(0u64, |h, c| {
                h.wrapping_mul(0x100000001b3)
                    ^ c.x as u64
                    ^ (c.z as u64).rotate_left(19)
                    ^ (c.y as u64).rotate_left(37)
                    ^ (w.material(c) as u64 + 1)
            });
    }
    if let Some(v) = cache().lock().unwrap().map.get(&key).cloned() {
        HITS.fetch_add(1, Ordering::Relaxed);
        return Ok(v);
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    let start = Instant::now();
    let mut value = Summary::default();
    if key.level == 0 {
        value = leaf(
            w,
            [key.x * EDGE, key.z * EDGE],
            cancel,
            directional_enabled(),
        )?;
    } else if key.level <= 2 {
        for z in 0..2 {
            for x in 0..2 {
                let child = node(
                    w,
                    Key {
                        x: key.x * 2 + x,
                        z: key.z * 2 + z,
                        level: key.level - 1,
                        ..key
                    },
                    cancel,
                )?;
                value.add(&child, 0.25);
            }
        }
    } else {
        // Bounded coarse population estimate. Fine render subdivisions share
        // this world-aligned representative; never survey their whole region.
        let side = 1i64 << key.level;
        value = node(
            w,
            Key {
                x: key.x * side + side / 2,
                z: key.z * side + side / 2,
                level: 0,
                ..key
            },
            cancel,
        )?;
    }
    if key.level == 0 {
        NANOS.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    let mut c = cache().lock().unwrap();
    if !c.map.contains_key(&key) {
        while c.map.len() >= CAPACITY {
            if let Some(k) = c.fifo.pop_front() {
                c.map.remove(&k);
            }
        }
        c.map.insert(key, value.clone());
        c.fifo.push_back(key);
    }
    Ok(value)
}
#[derive(Clone, Copy)]
struct Paint {
    rgb: [f32; 3],
}
// Mean projected blossom footprint from procedurally grown models. These
// models are measured once, never placed as canned assets in the world.
fn blossom_area_m2() -> f32 {
    static AREA: OnceLock<f32> = OnceLock::new();
    *AREA.get_or_init(|| {
        let mut area = 0.;
        for seed in 0..24 {
            let p = vegetation::PlantRecord {
                species: Species::Edelweiss,
                seed,
                root: frontier_world::Cell { x: 0, y: 0, z: 0 },
                variant: (seed % 3) as u8,
                rotation: ((seed / 3) % 4) as u8,
            };
            let model = vegetation::model_for(&p);
            let mut tops = HashMap::new();
            for v in &model.voxels {
                let t = tops
                    .entry((v.cell.x, v.cell.z))
                    .or_insert((i64::MIN, false));
                if v.cell.y > t.0 {
                    *t = (v.cell.y, v.material == Material::Flower);
                }
            }
            area += tops.values().filter(|(_, flower)| *flower).count() as f32
                * (VOXEL_SIZE * VOXEL_SIZE) as f32;
        }
        area / 24.
    })
}
fn leaf(
    w: &World,
    min: [i64; 2],
    cancel: &mut impl FnMut() -> bool,
    directional: bool,
) -> Result<Summary, String> {
    let mut out = Summary::default();
    if directional {
        out.directional = Some(Box::new([[[0.; 4]; 5]; 8]));
    }
    let center = [min[0] + EDGE / 2, min[1] + EDGE / 2];
    out.data[1][4][3] = w.column(center[0], center[1]).height_m() as f32;
    let edited: HashSet<_> = w.edit_cells().collect();
    let mut cells = vec![None::<Paint>; (EDGE * EDGE) as usize * HEIGHT];
    let mut heights = HashMap::new();
    for species in [Species::Grass, Species::Edelweiss] {
        let records =
            w.vegetation_ground_records_in(min.map(|v| v - 8), min.map(|v| v + EDGE + 8), species);
        for p in records {
            if cancel() {
                return Err("cancelled ground-cover models".into());
            }
            let model =
                vegetation::model_for_cancellable(&p, || false).ok_or("cancelled plant source")?;
            let stem = model
                .voxels
                .iter()
                .filter(|v| v.material == Material::Leaf)
                .min_by(|a, b| {
                    a.color
                        .iter()
                        .sum::<f32>()
                        .total_cmp(&b.color.iter().sum::<f32>())
                })
                .map(|v| v.color);
            for v in &model.voxels {
                let c = p.world_cell(v.cell);
                let x = c.x - min[0];
                let z = c.z - min[1];
                if !(0..EDGE).contains(&x) || !(0..EDGE).contains(&z) {
                    continue;
                }
                let ground = *heights
                    .entry((c.x, c.z))
                    .or_insert_with(|| (w.column(c.x, c.z).height_m() / VOXEL_SIZE) as i64);
                let y = c.y - ground;
                if !(0..HEIGHT as i64).contains(&y) || edited.contains(&c) {
                    continue;
                }
                let at = ((z * EDGE + x) as usize) * HEIGHT + y as usize;
                cells[at].get_or_insert(Paint {
                    // Keep the green carrier separate from unresolved white
                    // heads. Their expected coverage is reconstructed below.
                    rgb: if p.species == Species::Edelweiss {
                        stem.unwrap_or(v.color)
                    } else {
                        v.color
                    },
                });
            }
        }
    }
    if cells.iter().all(Option::is_none) {
        return Ok(out);
    }
    let index = |x: i64, y: i64, z: i64| ((z * EDGE + x) as usize) * HEIGHT + y as usize;
    let occupied = |x: i64, y: i64, z: i64| {
        y < 0
            || (y < HEIGHT as i64
                && cells[index(x.rem_euclid(EDGE), y, z.rem_euclid(EDGE))].is_some())
    };
    let mut top_rgb = [0f32; 3];
    let mut sum = 0.;
    let mut sum2 = 0.;
    let mut columns = 0.;
    let mut top_heights = Vec::new();
    for z in 0..EDGE {
        for x in 0..EDGE {
            let mut bands = [false; 3];
            let mut top = None;
            for y in 0..HEIGHT as i64 {
                let Some(p) = cells[index(x, y, z)] else {
                    continue;
                };
                bands[(y as usize / 4).min(2)] = true;
                top = Some((y, p));
            }
            for b in 0..3 {
                if bands[b] {
                    out.data[1][3][b] += 1. / 256.;
                }
            }
            if let Some((y, p)) = top {
                for channel in 0..3 {
                    top_rgb[channel] += p.rgb[channel] / 256.;
                }
                let l = p.rgb[0] * 0.2126 + p.rgb[1] * 0.7152 + p.rgb[2] * 0.0722;
                sum += l;
                sum2 += l * l;
                columns += 1.;
                top_heights.push((y + 1) as f32 * VOXEL_SIZE as f32);
            }
        }
    }
    top_heights.sort_by(f32::total_cmp);
    out.data[1][4][0] = columns / 256.;
    out.data[1][4][1] = top_heights[(top_heights.len() * 9 / 10).min(top_heights.len() - 1)];
    out.data[1][4][2] = (sum2 / columns - (sum / columns).powi(2)).max(0.).sqrt();
    out.data[1][3][3] = (vegetation::flower_birth_probability(w.seed(), center[0], center[1])
        as f32
        * 4.
        * blossom_area_m2())
    .min(out.data[1][4][0]);
    // Locally stationary periodic microcell estimator. First-hit opacity is
    // measured; exposed-face area above is used ONLY for conditional lighting.
    // No periodically repeated plant geometry is drawn in the game.
    let mut visible_faces = [[0f32; 4]; 5];
    for (e, elevation) in [0.125f64, 1.].into_iter().enumerate() {
        for a in 0..4 {
            let horizontal = [[1., 0.], [0., 1.], [-1., 0.], [0., -1.]][a];
            let d = [horizontal[0], -elevation, horizontal[1]];
            let mut hits = 0.;
            for z in 0..4 {
                for x in 0..4 {
                    let origin = [
                        (x * 4 + 2) as f64 + 0.5,
                        top_heights.last().copied().unwrap_or(0.) as f64 / VOXEL_SIZE + 0.37,
                        (z * 4 + 2) as f64 + 0.5,
                    ];
                    let mut p = origin.map(|v| v.floor() as i64);
                    let mut face = 0usize;
                    for _ in 0..((HEIGHT + 2) * 12) {
                        if p[1] < 0 {
                            break;
                        }
                        if occupied(p[0], p[1], p[2]) {
                            hits += 1.;
                            if let Some(paint) =
                                cells[index(p[0].rem_euclid(EDGE), p[1], p[2].rem_euclid(EDGE))]
                            {
                                for channel in 0..3 {
                                    visible_faces[face][channel] += paint.rgb[channel] / 128.;
                                    if let Some(lobes) = &mut out.directional {
                                        lobes[e * 4 + a][face][channel] += paint.rgb[channel] / 16.;
                                    }
                                }
                                visible_faces[face][3] += 1. / 128.;
                                if let Some(lobes) = &mut out.directional {
                                    lobes[e * 4 + a][face][3] += 1. / 16.;
                                }
                            }
                            break;
                        }
                        let next = std::array::from_fn::<_, 3, _>(|i| {
                            if d[i] == 0. {
                                f64::INFINITY
                            } else {
                                ((p[i] + i64::from(d[i] > 0.)) as f64 - origin[i]) / d[i]
                            }
                        });
                        let t = next.iter().copied().fold(f64::INFINITY, f64::min);
                        for i in 0..3 {
                            if next[i] == t {
                                p[i] += if d[i] > 0. { 1 } else { -1 };
                                face = match i {
                                    0 => {
                                        if d[i] > 0. {
                                            2
                                        } else {
                                            1
                                        }
                                    }
                                    1 => 0,
                                    _ => {
                                        if d[i] > 0. {
                                            4
                                        } else {
                                            3
                                        }
                                    }
                                };
                            }
                        }
                    }
                }
            }
            out.data[1][1 + e][a] = hits / 16.;
        }
    }
    // Rays are uniform on the horizontal launch plane. A horizontal face
    // projects with factor 1; a cardinal side projects with |dx/dy|, whose
    // average over the eight probe directions is (8+1)/8. Deprojection
    // retains a compact, visibility-conditioned face mixture for new views.
    for face in 0..5 {
        let projection = if face == 0 { 1. } else { 1.125 };
        out.data[0][face] = visible_faces[face].map(|v| v / projection);
    }
    out.data[0][0] = [top_rgb[0], top_rgb[1], top_rgb[2], columns / 256.];
    Ok(out)
}
/// Extend the existing terrain profile table, without changing its colors,
/// normals, topology, native certificates or source/physics identity.
pub(crate) fn decorate(
    w: &World,
    tile: &mut GeometryTile,
    cancel: &mut impl FnMut() -> bool,
) -> Result<(), String> {
    if !enabled()
        || !w.vegetation_enabled()
        || tile.exact
        || tile.key.step_cells < 2
        || tile.gpu_profiles.is_some()
    {
        return Ok(());
    }
    // Coverage populations vary over metres; render subdivision must not
    // trigger a fresh native plant survey per fine mesh tile. Fine radiance
    // variance is reconstructed separately in the shader.
    let mut spacing = EDGE * 8;
    while tile.key.width_cells as i64 / spacing > 4 {
        spacing *= 2;
    }

    let origin = tile
        .key
        .min
        .map(|v| v.div_euclid(spacing) * spacing - spacing);
    let end = tile.key.min.map(|v| v + tile.key.width_cells as i64);
    let dimension = ((end[0] - origin[0]).max(end[1] - origin[1]) + spacing - 1) / spacing + 1;
    let dimension = dimension as usize;
    let source = source(w);
    let level = (spacing / EDGE).ilog2() as u8;
    let mut refs = Vec::new();
    let mut any = false;
    for z in 0..dimension {
        for x in 0..dimension {
            let p = [
                origin[0] + x as i64 * spacing,
                origin[1] + z as i64 * spacing,
            ];
            let value = node(
                w,
                Key {
                    source,
                    x: p[0].div_euclid(spacing),
                    z: p[1].div_euclid(spacing),
                    level,
                    edits: 0,
                },
                cancel,
            )?;
            any |= value.coverage() > 0.;
            let id = tile.face_profiles.len() as u32;
            tile.face_profiles.extend(value.data);
            if let Some(lobes) = value.directional {
                tile.face_profiles.extend(*lobes);
            }
            refs.push(id);
        }
    }

    if !any {
        tile.face_profiles
            .truncate(tile.face_profiles.len() - refs.len() * records());
        return Ok(());
    }
    let at = tile.face_profiles.len() as u32;
    let mut header = crate::spatial_appearance::map_header(
        [
            (origin[0] - tile.key.min[0]) as f32 * VOXEL_SIZE as f32
                + spacing as f32 * VOXEL_SIZE as f32 * 0.5,
            (origin[1] - tile.key.min[1]) as f32 * VOXEL_SIZE as f32
                + spacing as f32 * VOXEL_SIZE as f32 * 0.5,
        ],
        spacing as f32 * VOXEL_SIZE as f32,
        dimension,
        &refs,
        at,
    );
    header[0][0][3] = -7.;
    header[0][1][2] = directional_enabled() as u8 as f32;
    tile.face_profiles.extend(header);
    let mut wrappers = HashMap::new();
    for index in &mut tile.face_profile_indices {
        let old = *index;
        let id = *wrappers.entry(old).or_insert_with(|| {
            let id = tile.face_profiles.len() as u32;
            let mut p = [[0.; 4]; 5];
            p[0] = [
                (old as i64 - id as i64) as f32,
                (at as i64 - id as i64) as f32,
                if shell_enabled() { 1. } else { 0. },
                -6.,
            ];
            tile.face_profiles.push(p);
            id
        });
        *index = id;
    }
    tile.bytes += refs.len() * records() * 80 + (tile.face_profiles.len() - at as usize) * 80;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::Cell;
    fn world() -> World {
        World::fbm(
            7,
            frontier_world::FbmParams {
                recipe: frontier_world::FbmRecipe::Alpine,
                ..Default::default()
            },
        )
        .unwrap()
        .with_vegetation()
    }
    #[test]
    fn directional_lobes_preserve_probe_opacity_without_changing_the_fast_product() {
        let w = world();
        let min = [-58 * EDGE, -48 * EDGE];
        let a = leaf(&w, min, &mut || false, false).unwrap();
        let b = leaf(&w, min, &mut || false, true).unwrap();
        assert!(a.directional.is_none());
        assert_eq!(a.data, b.data);
        let lobes = b.directional.unwrap();
        let mut active = false;
        for e in 0..2 {
            for az in 0..4 {
                let phi: f32 = lobes[e * 4 + az].iter().map(|v| v[3]).sum();
                assert_eq!(phi, b.data[1][1 + e][az]);
                active |= phi > 0.;
            }
        }
        assert!(active);
        assert!(blossom_area_m2() > 0.);
    }
    #[test]
    fn native_summaries_are_finite_bounded_and_restrict_consistently() {
        let w = world();
        let mut active = false;
        for p in [[-58, -48], [-57, -47], [-42, -250], [-43, -249]] {
            let key = Key {
                source: source(&w),
                x: p[0],
                z: p[1],
                level: 0,
                edits: 0,
            };
            let a = node(&w, key, &mut || false).unwrap();
            let b = node(&w, key, &mut || false).unwrap();
            assert_eq!(a, b);
            assert!(a.data.iter().flatten().flatten().all(|v| v.is_finite()));
            assert!(a.data[1][1]
                .iter()
                .chain(a.data[1][2].iter())
                .all(|v| *v >= 0. && *v <= 1.));
            assert!(a.data[1][3][3] <= a.data[1][4][0]);
            active |= a.coverage() > 0.;
        }
        assert!(active);
        let key = Key {
            source: source(&w),
            x: -29,
            z: -24,
            level: 1,
            edits: 0,
        };
        let parent = node(&w, key, &mut || false).unwrap();
        let mut expected = Summary::default();
        for z in 0..2 {
            for x in 0..2 {
                expected.add(
                    &node(
                        &w,
                        Key {
                            x: key.x * 2 + x,
                            z: key.z * 2 + z,
                            level: 0,
                            ..key
                        },
                        &mut || false,
                    )
                    .unwrap(),
                    0.25,
                );
            }
        }
        assert_eq!(parent, expected);
    }
    #[test]
    fn local_edit_changes_only_dependent_cached_field_nodes() {
        let mut w = world();
        let key = Key {
            source: source(&w),
            x: -58,
            z: -48,
            level: 0,
            edits: 0,
        };
        let original = node(&w, key, &mut || false).unwrap();
        w.set(
            Cell {
                x: 90000,
                y: 0,
                z: 90000,
            },
            Material::Stone,
        );
        assert_eq!(node(&w, key, &mut || false).unwrap(), original);
        let min = [key.x * EDGE, key.z * EDGE];
        let records = w.vegetation_records_species(min, min.map(|v| v + EDGE), Species::Grass);
        let mut changed = false;
        for p in records {
            let model = vegetation::model_for(&p);
            for v in &model.voxels {
                let c = p.world_cell(v.cell);
                if c.x >= min[0] && c.x < min[0] + EDGE && c.z >= min[1] && c.z < min[1] + EDGE {
                    w.set(c, Material::Air);
                    changed = true;
                }
            }
        }
        assert!(changed);
        let edited = node(&w, key, &mut || false).unwrap();
        assert_ne!(edited, original);
        assert!(node(&w, key, &mut || true).is_err());
    }
    #[test]
    fn source_identity_separates_seed_params_and_world_kinds() {
        let w = world();
        let different = World::fbm(8, w.fbm_terrain().unwrap().params()).unwrap();
        assert_ne!(source(&w), source(&different));
        assert_ne!(source(&w), source(&World::new(7)));
    }
}
