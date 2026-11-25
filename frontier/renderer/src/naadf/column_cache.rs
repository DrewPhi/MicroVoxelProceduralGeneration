//! Disposable, bounded cache of exact terrain-only 1 m column tiles.
//! Never stores vegetation, edits, occupancy, or rendered LOD. Procedural source
//! identity is part of every key; edited/volumetric columns bypass the cache.
use frontier_world::World;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};
#[derive(Clone)]
pub(super) struct Samples {
    pub heights: [i64; 256],
    pub ground_heights: [i64; 256],
    pub bands: [[u32; 3]; 256],
}
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
// x/z are exact native-cell origins, including non-meter-aligned test regions.
struct Key {
    source: [u64; 9],
    x: i64,
    z: i64,
}
impl Key {
    fn new(world: &World, x: i64, z: i64) -> Option<Self> {
        let t = world.fbm_terrain()?;
        let p = t.params();
        Some(Self {
            source: [
                world.seed(),
                p.recipe as u64,
                p.horizontal_scale.to_bits(),
                p.amplitude.to_bits(),
                u64::from(p.octaves),
                p.roughness.to_bits(),
                t.seed(),
                u64::from(t.source_version()),
                u64::from(t.appearance_version()),
            ],
            x,
            z,
        })
    }
}
fn capacity() -> usize {
    static C: OnceLock<usize> = OnceLock::new();
    *C.get_or_init(|| {
        std::env::var("FRONTIER_NAADF_COLUMN_CACHE_MIB")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| (16..=2048).contains(n))
            .unwrap_or(320)
            * 1048576
            / 7168
    })
}
#[derive(Default)]
struct Cache {
    values: HashMap<Key, Arc<Samples>>,
    fifo: VecDeque<Key>,
    hits: u64,
    misses: u64,
}
fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(Default::default)
}
pub(super) fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| std::env::var("FRONTIER_NAADF_COLUMN_CACHE").as_deref() == Ok("1"))
}
/// Caller must first prove that all native cells in this 16x16 column tile
/// contain no edits and have no volumetric ownership. Revision is deliberately
/// absent from the key: unrelated edits cannot change this immutable product.
pub(super) fn get_unedited(world: &World, x: i64, z: i64) -> Option<Arc<Samples>> {
    let key = Key::new(world, x, z)?;
    let mut c = cache().lock().unwrap();
    let value = c.values.get(&key).cloned();
    if value.is_some() {
        c.hits += 1
    } else {
        c.misses += 1
    }
    value
}
/// Store samples only after the same terrain-only gate as `get_unedited`.
/// The cache itself does not scan edits; the builder already has a local index.
pub(super) fn insert_unedited(world: &World, x: i64, z: i64, s: Samples) {
    let Some(key) = Key::new(world, x, z) else {
        return;
    };
    let mut c = cache().lock().unwrap();
    if c.values.contains_key(&key) {
        return;
    }
    if c.values.len() == capacity() {
        if let Some(old) = c.fifo.pop_front() {
            c.values.remove(&old);
        }
    }
    c.fifo.push_back(key);
    c.values.insert(key, Arc::new(s));
}
pub(super) fn json() -> String {
    // Do not allocate even the cache object when disabled.
    if !enabled() {
        return "{\"enabled\":false}".into();
    }
    let c = cache().lock().unwrap();
    format!("{{\"enabled\":true,\"hits\":{},\"misses\":{},\"tiles\":{},\"capacity_tiles\":{},\"resident_payload_bytes\":{},\"payload_limit_bytes\":{},\"estimated_metadata_bytes\":{}}}",c.hits,c.misses,c.values.len(),capacity(),c.values.len()*7168,capacity()*7168,c.values.capacity()*(std::mem::size_of::<Key>()+std::mem::size_of::<Arc<Samples>>()+8)+c.fifo.capacity()*std::mem::size_of::<Key>())
}
#[cfg(test)]
mod tests {
    use super::*;
    use frontier_world::terrain::{FbmParams, FbmRecipe};
    #[test]
    fn keys_include_every_source_parameter_and_negative_coordinates() {
        let p = FbmParams {
            recipe: FbmRecipe::Alpine,
            ..Default::default()
        };
        let a = World::fbm(7, p).unwrap();
        let b = World::fbm(8, p).unwrap();
        assert!(Key::new(&a, -1, -2) != Key::new(&b, -1, -2));
        assert!(Key::new(&a, -1, -2) != Key::new(&a, 1, -2));
        let b = World::fbm(
            7,
            FbmParams {
                amplitude: p.amplitude + 1.,
                ..p
            },
        )
        .unwrap();
        assert!(Key::new(&a, -1, -2) != Key::new(&b, -1, -2));
    }
}

#[cfg(test)]
#[test]
fn distant_edit_preserves_unedited_cache_identity_and_payload() {
    let mut world = World::fbm(67219831, frontier_world::FbmParams::default()).unwrap();
    let x = -176;
    let z = -304;
    let before = Key::new(&world, x, z);
    insert_unedited(
        &world,
        x,
        z,
        Samples {
            heights: [123; 256],
            ground_heights: [123; 256],
            bands: [[1, 2, 3]; 256],
        },
    );
    let original = get_unedited(&world, x, z).unwrap();
    world.set(
        frontier_world::Cell {
            x: 900000,
            y: 10,
            z: -900000,
        },
        frontier_world::Material::Wood,
    );
    assert!(world.revision() > 0);
    assert!(before == Key::new(&world, x, z));
    let cached = get_unedited(&world, x, z).unwrap();
    assert!(Arc::ptr_eq(&original, &cached));
    assert_eq!(cached.heights, [123; 256]);
    assert_eq!(cached.bands, [[1, 2, 3]; 256]);
    let params = world.fbm_terrain().unwrap().params();
    for changed in [
        frontier_world::FbmParams {
            recipe: frontier_world::FbmRecipe::Alpine,
            ..params
        },
        frontier_world::FbmParams {
            horizontal_scale: params.horizontal_scale + 1.,
            ..params
        },
        frontier_world::FbmParams {
            amplitude: params.amplitude + 1.,
            ..params
        },
        frontier_world::FbmParams {
            octaves: params.octaves + 1,
            ..params
        },
        frontier_world::FbmParams {
            roughness: params.roughness * 0.9,
            ..params
        },
    ] {
        let changed = World::fbm(world.seed(), changed).unwrap();
        assert!(before != Key::new(&changed, x, z));
        assert!(get_unedited(&changed, x, z).is_none());
    }
}
