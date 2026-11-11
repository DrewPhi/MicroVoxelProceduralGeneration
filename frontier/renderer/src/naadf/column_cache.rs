//! Disposable, bounded cache of exact terrain-only 1 m column tiles.
//! Never stores vegetation, edits, occupancy, or rendered LOD. Full source
//! identity is part of every key; edited/volumetric columns bypass the cache.
use frontier_world::World;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};
#[derive(Clone)]
pub(super) struct Samples {
    pub heights: Vec<i64>,
    pub bands: Vec<[u32; 3]>,
}
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
// x/z are exact native-cell origins, including non-meter-aligned test regions.
struct Key {
    source: [u64; 8],
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
                world.revision(),
                p.recipe as u64,
                p.horizontal_scale.to_bits(),
                p.amplitude.to_bits(),
                u64::from(p.octaves),
                p.roughness.to_bits(),
                t.seed(),
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
            / 5120
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
pub(super) fn get(world: &World, x: i64, z: i64) -> Option<Arc<Samples>> {
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
pub(super) fn insert(world: &World, x: i64, z: i64, s: Samples) {
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
    format!("{{\"enabled\":true,\"hits\":{},\"misses\":{},\"tiles\":{},\"capacity_tiles\":{},\"resident_payload_bytes\":{},\"payload_limit_bytes\":{}}}",c.hits,c.misses,c.values.len(),capacity(),c.values.len()*5120,capacity()*5120)
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
