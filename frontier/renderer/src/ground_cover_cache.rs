//! Byte-bounded, sharded immutable summary cache. Producers never hold a cache
//! lock while computing children. Abandoned producers wake all waiters.
use super::{Key, Summary};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
const SHARDS: usize = 16;
const MAX_FLIGHTS_PER_SHARD: usize = 64;
// Includes both retained key copies, Arc allocation and conservative hash-table
// slot/control overhead, in addition to the Summary and optional direction data.
const ENTRY_OVERHEAD: usize = 2 * std::mem::size_of::<Key>() + 96;
struct Cached {
    summary: Summary,
    bytes: usize,
    live: Arc<AtomicUsize>,
}
impl Drop for Cached {
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
#[derive(Default)]
struct Shard {
    values: HashMap<Key, Arc<Cached>>,
    fifo: VecDeque<Key>,
    flights: HashMap<Key, Arc<Flight>>,
    bytes: usize,
}
enum State {
    Pending,
    Ready(Arc<Cached>),
    Abandoned,
}
pub(super) struct Flight {
    state: Mutex<State>,
    ready: Condvar,
}
impl Flight {
    pub(super) fn wait(
        &self,
        cancel: &mut impl FnMut() -> bool,
    ) -> Result<Option<Summary>, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if cancel() {
                return Err("cancelled ground-cover page".into());
            }
            match &*state {
                State::Ready(v) => return Ok(Some(v.summary.clone())),
                State::Abandoned => return Ok(None),
                State::Pending => {
                    state = self
                        .ready
                        .wait_timeout(state, Duration::from_millis(10))
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            }
        }
    }
}
pub(super) enum Request<'a> {
    Hit(Summary),
    Wait(Arc<Flight>),
    Build(Producer<'a>),
    // Bounded in-flight metadata: saturated shards compute without registration.
    Bypass,
}
pub(super) struct Producer<'a> {
    cache: &'a Cache,
    key: Key,
    shard: usize,
    flight: Arc<Flight>,
    done: bool,
}
impl Producer<'_> {
    pub(super) fn complete(mut self, summary: &Summary) {
        let bytes = ENTRY_OVERHEAD
            + std::mem::size_of::<Summary>()
            + if summary.directional.is_some() {
                640
            } else {
                0
            };
        self.cache.live.fetch_add(bytes, Ordering::Relaxed);
        let value = Arc::new(Cached {
            summary: summary.clone(),
            bytes,
            live: self.cache.live.clone(),
        });
        {
            let mut shard = self.cache.lock(self.shard);
            shard.flights.remove(&self.key);
            if bytes <= self.cache.per_shard_budget {
                while shard.bytes + bytes > self.cache.per_shard_budget {
                    let Some(key) = shard.fifo.pop_front() else {
                        break;
                    };
                    if let Some(old) = shard.values.remove(&key) {
                        shard.bytes -= old.bytes;
                        self.cache.evictions.fetch_add(1, Ordering::Relaxed);
                    }
                }
                shard.bytes += bytes;
                shard.values.insert(self.key, value.clone());
                shard.fifo.push_back(self.key);
            }
        }
        *self.flight.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Ready(value);
        self.done = true;
        self.flight.ready.notify_all();
    }
}
impl Drop for Producer<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        self.cache.lock(self.shard).flights.remove(&self.key);
        *self.flight.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Abandoned;
        self.cache.abandoned.fetch_add(1, Ordering::Relaxed);
        self.flight.ready.notify_all();
    }
}
pub(super) struct Cache {
    shards: [Mutex<Shard>; SHARDS],
    per_shard_budget: usize,
    live: Arc<AtomicUsize>,
    hits: AtomicU64,
    misses: AtomicU64,
    coalesced: AtomicU64,
    bypassed: AtomicU64,
    abandoned: AtomicU64,
    evictions: AtomicU64,
    lock_nanos: AtomicU64,
}
impl Cache {
    pub(super) fn new(budget: usize) -> Self {
        Self {
            shards: std::array::from_fn(|_| Mutex::new(Shard::default())),
            per_shard_budget: budget / SHARDS,
            live: Arc::default(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
            bypassed: AtomicU64::new(0),
            abandoned: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            lock_nanos: AtomicU64::new(0),
        }
    }
    fn lock(&self, index: usize) -> std::sync::MutexGuard<'_, Shard> {
        let start = Instant::now();
        let guard = self.shards[index].lock().unwrap_or_else(|e| e.into_inner());
        self.lock_nanos
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        guard
    }
    fn shard(key: Key) -> usize {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hash);
        hash.finish() as usize % SHARDS
    }
    pub(super) fn request(&self, key: Key) -> Request<'_> {
        let index = Self::shard(key);
        let mut shard = self.lock(index);
        if let Some(v) = shard.values.get(&key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Request::Hit(v.summary.clone());
        }
        if let Some(flight) = shard.flights.get(&key) {
            self.coalesced.fetch_add(1, Ordering::Relaxed);
            return Request::Wait(flight.clone());
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        if shard.flights.len() >= MAX_FLIGHTS_PER_SHARD {
            self.bypassed.fetch_add(1, Ordering::Relaxed);
            return Request::Bypass;
        }
        let flight = Arc::new(Flight {
            state: Mutex::new(State::Pending),
            ready: Condvar::new(),
        });
        shard.flights.insert(key, flight.clone());
        Request::Build(Producer {
            cache: self,
            key,
            shard: index,
            flight,
            done: false,
        })
    }
    pub(super) fn stats_json_fields(&self) -> String {
        let (mut entries, mut bytes, mut flights, mut payload) = (0, 0, 0, 0);
        for index in 0..SHARDS {
            let shard = self.lock(index);
            entries += shard.values.len();
            bytes += shard.bytes;
            flights += shard.flights.len();
            payload += shard
                .values
                .values()
                .map(|v| {
                    160 + if v.summary.directional.is_some() {
                        640
                    } else {
                        0
                    }
                })
                .sum::<usize>();
        }
        format!("\"entries\":{entries},\"payload_bytes\":{payload},\"retained_estimated_bytes\":{bytes},\"byte_budget\":{},\"evicted_but_live_estimated_bytes\":{},\"in_flight\":{flights},\"in_flight_limit\":{},\"hits\":{},\"misses\":{},\"coalesced_requests\":{},\"bypassed_requests\":{},\"abandoned_producers\":{},\"evictions\":{},\"lock_wait_ms\":{}",
            self.per_shard_budget*SHARDS,self.live.load(Ordering::Relaxed).saturating_sub(bytes),
            SHARDS*MAX_FLIGHTS_PER_SHARD,self.hits.load(Ordering::Relaxed),self.misses.load(Ordering::Relaxed),self.coalesced.load(Ordering::Relaxed),self.bypassed.load(Ordering::Relaxed),self.abandoned.load(Ordering::Relaxed),self.evictions.load(Ordering::Relaxed),self.lock_nanos.load(Ordering::Relaxed)as f64/1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Barrier};
    fn key(x: i64) -> Key {
        Key {
            source: [7; 8],
            x,
            z: -11,
            level: 0,
            edits: 0,
        }
    }
    fn summary(x: f32) -> Summary {
        let mut s = Summary::default();
        s.data[0][0][0] = x;
        s
    }
    #[test]
    fn same_key_has_one_producer_and_exact_results() {
        let cache = Cache::new(1048576);
        let barrier = Barrier::new(8);
        let builds = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    let actual = match cache.request(key(-1)) {
                        Request::Hit(v) => v,
                        Request::Wait(f) => f.wait(&mut || false).unwrap().unwrap(),
                        Request::Build(p) => {
                            builds.fetch_add(1, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(20));
                            let v = summary(f32::from_bits(0x3f012345));
                            p.complete(&v);
                            v
                        }
                        Request::Bypass => panic!("unexpected saturation"),
                    };
                    assert_eq!(actual, summary(f32::from_bits(0x3f012345)));
                });
            }
        });
        assert_eq!(builds.load(Ordering::Relaxed), 1);
        assert_eq!(cache.misses.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn independent_keys_build_concurrently() {
        let cache = Cache::new(1048576);
        let (sender, receiver) = mpsc::channel();
        let release = Barrier::new(3);
        std::thread::scope(|scope| {
            for x in [1, 2] {
                let sender = sender.clone();
                let release = &release;
                let cache = &cache;
                scope.spawn(move || {
                    let Request::Build(p) = cache.request(key(x)) else {
                        panic!("missing producer")
                    };
                    sender.send(x).unwrap();
                    release.wait();
                    p.complete(&summary(x as f32));
                });
            }
            let first = receiver.recv_timeout(Duration::from_secs(1));
            let second = receiver.recv_timeout(Duration::from_secs(1));
            release.wait();
            assert!(
                first.is_ok() && second.is_ok(),
                "independent computation serialized"
            );
        });
    }
    #[test]
    fn cancellation_abandonment_and_panics_allow_retry() {
        let cache = Cache::new(1048576);
        let Request::Build(producer) = cache.request(key(1)) else {
            panic!()
        };
        let Request::Wait(flight) = cache.request(key(1)) else {
            panic!()
        };
        assert!(flight.wait(&mut || true).is_err());
        drop(producer);
        assert!(flight.wait(&mut || false).unwrap().is_none());
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let Request::Build(_producer) = cache.request(key(1)) else {
                panic!()
            };
            panic!("simulated builder panic");
        }));
        assert!(panic.is_err());
        let Request::Build(producer) = cache.request(key(1)) else {
            panic!()
        };
        producer.complete(&summary(3.));
        let Request::Hit(result) = cache.request(key(1)) else {
            panic!()
        };
        assert_eq!(result, summary(3.));
        assert_eq!(cache.abandoned.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn directional_payload_and_evicted_waiter_values_are_accounted() {
        let bytes = ENTRY_OVERHEAD + std::mem::size_of::<Summary>() + 640;
        let cache = Cache::new(SHARDS * bytes);
        let first = key(0);
        let second = (1..1000)
            .map(key)
            .find(|k| Cache::shard(*k) == Cache::shard(first))
            .unwrap();
        let Request::Build(producer) = cache.request(first) else {
            panic!()
        };
        let Request::Wait(waiter) = cache.request(first) else {
            panic!()
        };
        let mut value = summary(2.);
        value.directional = Some(Box::new([[[0.; 4]; 5]; 8]));
        producer.complete(&value);
        let Request::Build(producer) = cache.request(second) else {
            panic!()
        };
        producer.complete(&value);
        assert_eq!(cache.lock(Cache::shard(first)).bytes, bytes);
        assert_eq!(cache.live.load(Ordering::Relaxed), 2 * bytes);
        assert_eq!(waiter.wait(&mut || false).unwrap().unwrap(), value);
        drop(waiter);
        assert_eq!(cache.live.load(Ordering::Relaxed), bytes);
    }
    #[test]
    fn retained_bytes_and_inflight_metadata_are_bounded() {
        let entry_bytes = ENTRY_OVERHEAD + std::mem::size_of::<Summary>();
        let cache = Cache::new(SHARDS * entry_bytes * 2);
        let shard = Cache::shard(key(0));
        let keys: Vec<_> = (0..100000)
            .map(key)
            .filter(|k| Cache::shard(*k) == shard)
            .take(100)
            .collect();
        for (i, k) in keys.iter().take(10).enumerate() {
            let Request::Build(p) = cache.request(*k) else {
                panic!()
            };
            p.complete(&summary(i as f32));
        }
        assert_eq!(cache.lock(shard).values.len(), 2);
        assert_eq!(cache.lock(shard).bytes, 2 * entry_bytes);
        let mut producers = Vec::new();
        for k in keys.iter().skip(10).take(MAX_FLIGHTS_PER_SHARD) {
            let Request::Build(p) = cache.request(*k) else {
                panic!()
            };
            producers.push(p);
        }
        assert!(matches!(cache.request(keys[99]), Request::Bypass));
        drop(producers);
        assert!(cache.lock(shard).flights.is_empty());
    }
}
