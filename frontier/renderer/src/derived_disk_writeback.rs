//! Optional bounded writeback. One I/O thread owns the disk cache; geometry jobs
//! only wait for positively indexed reads. Absent keys and write admission never
//! wait for encoding, fsync, or directory mutation.
use crate::{
    adaptive_geometry::{GeometryTile, TileKey},
    derived_disk_cache::{DiskCache, DiskCacheKeyer, DiskCacheStats},
};
use frontier_world::World;
use std::{
    collections::{HashSet, VecDeque},
    io,
    path::PathBuf,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};
const WRITE_BYTES: usize = 64 * 1024 * 1024;
const WRITE_COUNT: usize = 64;
// Stale positive index entries are safe (a read can miss). Periodic replacement
// bounds their metadata without scanning the cache on every generated tile.
const INDEX_COUNT: usize = 32768;
struct Read {
    key: TileKey,
    reply: mpsc::SyncSender<Option<GeometryTile>>,
}
struct State {
    index: HashSet<String>,
    reads: VecDeque<Read>,
    writes: VecDeque<Arc<GeometryTile>>,
    write_keys: HashSet<TileKey>,
    pending_bytes: usize,
    peak_bytes: usize,
    peak_count: usize,
    skipped: u64,
    fast_misses: u64,
    active_buffer: usize,
    peak_buffer: usize,
    disk: DiskCacheStats,
    closing: bool,
    failed: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    #[cfg(test)]
    test_gate: Mutex<Option<Arc<TestGate>>>,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum TestJob {
    Read(TileKey),
    Write(TileKey),
}
#[cfg(test)]
struct TestGate {
    selected: mpsc::Sender<TestJob>,
    first_release: Mutex<Option<mpsc::Receiver<()>>>,
}
#[cfg(test)]
impl TestGate {
    fn selected(&self, job: TestJob) {
        let _ = self.selected.send(job);
        let release = self.first_release.lock().unwrap().take();
        if let Some(release) = release {
            let _ = release.recv();
        }
    }
}
pub(crate) struct AsyncDiskCache {
    shared: Arc<Shared>,
    keyer: DiskCacheKeyer,
    max_entry: usize,
    worker: Option<JoinHandle<()>>,
}
impl AsyncDiskCache {
    pub(crate) fn new(
        path: PathBuf,
        budget: u64,
        max_entry: usize,
        world: &World,
    ) -> io::Result<Self> {
        let max_entry = max_entry.min(WRITE_BYTES);
        let disk = DiskCache::new(path, budget, max_entry, world)?;
        let keyer = disk.keyer();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                index: disk.index_names(),
                reads: VecDeque::new(),
                writes: VecDeque::new(),
                write_keys: HashSet::new(),
                pending_bytes: 0,
                peak_bytes: 0,
                peak_count: 0,
                skipped: 0,
                fast_misses: 0,
                active_buffer: 0,
                peak_buffer: 0,
                disk: disk.stats(),
                closing: false,
                failed: false,
            }),
            wake: Condvar::new(),
            #[cfg(test)]
            test_gate: Mutex::new(None),
        });
        let io_shared = shared.clone();
        let io_keyer = keyer.clone();
        let worker = thread::Builder::new()
            .name("adaptive-disk-writeback".into())
            .spawn(move || {
                let recovery = io_shared.clone();
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(disk, io_shared, io_keyer, max_entry)
                }))
                .is_err()
                {
                    let mut s = recovery.state.lock().unwrap_or_else(|p| p.into_inner());
                    s.failed = true;
                    s.disk.errors += 1;
                    s.index.clear();
                    s.writes.clear();
                    s.write_keys.clear();
                    s.pending_bytes = 0;
                    s.active_buffer = 0;
                    while let Some(read) = s.reads.pop_front() {
                        let _ = read.reply.send(None);
                    }
                    recovery.wake.notify_all();
                }
            })?;
        Ok(Self {
            shared,
            keyer,
            max_entry,
            worker: Some(worker),
        })
    }
    pub(crate) fn load(&mut self, key: TileKey) -> Option<GeometryTile> {
        let name = self.keyer.name(key);
        let (send, receive) = mpsc::sync_channel(1);
        {
            let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            if s.failed || s.closing || !s.index.contains(&name) {
                s.fast_misses += 1;
                return None;
            }
            s.reads.push_back(Read { key, reply: send });
            self.shared.wake.notify_one();
        }
        receive.recv().ok().flatten()
    }
    pub(crate) fn queue_store(&mut self, tile: Arc<GeometryTile>) -> bool {
        let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if s.failed
            || s.closing
            || s.write_keys.contains(&tile.key)
            || s.write_keys.len() >= WRITE_COUNT
            || tile.bytes > WRITE_BYTES.saturating_sub(s.pending_bytes)
        {
            s.skipped += 1;
            return false;
        }
        s.pending_bytes += tile.bytes;
        s.write_keys.insert(tile.key);
        s.writes.push_back(tile);
        s.peak_bytes = s.peak_bytes.max(s.pending_bytes);
        s.peak_count = s.peak_count.max(s.write_keys.len());
        self.shared.wake.notify_one();
        true
    }
    pub(crate) fn stats(&self) -> DiskCacheStats {
        let s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        DiskCacheStats {
            misses: s.disk.misses + s.fast_misses,
            writeback_queued_entries: s.write_keys.len(),
            writeback_peak_queued_entries: s.peak_count,
            writeback_queued_payload_bytes: s.pending_bytes,
            writeback_peak_queued_payload_bytes: s.peak_bytes,
            writeback_skipped_writes: s.skipped,
            writeback_encode_buffer_cap_bytes: self.max_entry,
            writeback_encode_buffer_reserved_bytes: s.active_buffer,
            writeback_peak_encode_buffer_reserved_bytes: s.peak_buffer,
            ..s.disk
        }
    }
    /// Explicit durability barrier for tests/tools. Interactive preparation does
    /// not invoke it. Shutdown also drains the bounded admitted write queue.
    #[allow(dead_code)]
    pub(crate) fn flush(&mut self) {
        let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        while !s.write_keys.is_empty() && !s.failed {
            s = self.shared.wake.wait(s).unwrap_or_else(|p| p.into_inner());
        }
    }
}
impl Drop for AsyncDiskCache {
    fn drop(&mut self) {
        {
            let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            s.closing = true;
            self.shared.wake.notify_one();
        }
        // GeometryCache is dropped on the preparation worker at application
        // shutdown. At most 64 entries / 64 MiB payload remain to encode/write;
        // filesystem calls themselves cannot promise a wall-clock deadline.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn run(mut disk: DiskCache, shared: Arc<Shared>, keyer: DiskCacheKeyer, max_entry: usize) {
    enum Job {
        Read(Read),
        Write(Arc<GeometryTile>),
    }
    loop {
        let job = {
            let mut s = shared.state.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if let Some(read) = s.reads.pop_front() {
                    break Job::Read(read);
                }
                if let Some(tile) = s.writes.pop_front() {
                    s.active_buffer = max_entry;
                    s.peak_buffer = s.peak_buffer.max(max_entry);
                    break Job::Write(tile);
                }
                if s.closing {
                    return;
                }
                s = shared.wake.wait(s).unwrap_or_else(|p| p.into_inner());
            }
        };
        // Test-only blocking at an actual IO boundary lets the regression
        // control backlog deterministically without sleeps or filesystem timing.
        #[cfg(test)]
        {
            let gate = shared.test_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.selected(match &job {
                    Job::Read(r) => TestJob::Read(r.key),
                    Job::Write(t) => TestJob::Write(t.key),
                });
            }
        }
        match job {
            Job::Read(read) => {
                let tile = disk.load(read.key);
                let removed = disk.take_removed_names();
                let mut s = shared.state.lock().unwrap_or_else(|p| p.into_inner());
                for name in removed {
                    s.index.remove(&name);
                }
                if tile.is_none() {
                    s.index.remove(&keyer.name(read.key));
                }
                s.disk = disk.stats();
                drop(s);
                let _ = read.reply.send(tile);
            }
            Job::Write(tile) => {
                let bytes = tile.bytes;
                let key = tile.key;
                let stored = disk.store(&tile);
                let removed = disk.take_removed_names();
                drop(tile);
                let mut s = shared.state.lock().unwrap_or_else(|p| p.into_inner());
                for name in removed {
                    s.index.remove(&name);
                }
                if stored {
                    if s.index.len() >= INDEX_COUNT {
                        s.index = disk.index_names();
                    }
                    s.index.insert(keyer.name(key));
                }
                s.pending_bytes -= bytes;
                s.write_keys.remove(&key);
                s.active_buffer = 0;
                s.disk = disk.stats();
                shared.wake.notify_all();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    fn fixture() -> (World, Arc<GeometryTile>, PathBuf) {
        let world = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let key = TileKey {
            min: [0, -512],
            width_cells: 16,
            step_cells: 4,
            edge_steps: [4; 4],
            source_revision: 0,
        };
        let tile = Arc::new(crate::adaptive_geometry::build(&world, key, || false).unwrap());
        let path = std::env::temp_dir().join(format!(
            "frontier-writeback-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        (world, tile, path)
    }
    #[test]
    fn indexed_read_preempts_write_backlog_and_inflight_write_counts_toward_limit() {
        use std::time::Duration;
        struct Release(Option<mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }
        let (world, warm, path) = fixture();
        let warm_key = warm.key;
        {
            let mut disk =
                DiskCache::new(path.clone(), 8 * 1024 * 1024, 1024 * 1024, &world).unwrap();
            assert!(disk.store(&warm));
        }
        let mut cache =
            AsyncDiskCache::new(path.clone(), 8 * 1024 * 1024, 1024 * 1024, &world).unwrap();
        let (selected_tx, selected) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Release(Some(release_tx));
        *cache.shared.test_gate.lock().unwrap() = Some(Arc::new(TestGate {
            selected: selected_tx,
            first_release: Mutex::new(Some(release_rx)),
        }));
        let mut keys = vec![];
        for i in 0..=WRITE_COUNT {
            let key = TileKey {
                min: [(i as i64 + 1) * 16, -512],
                ..warm_key
            };
            let tile = Arc::new(crate::adaptive_geometry::build(&world, key, || false).unwrap());
            let accepted = cache.queue_store(tile);
            assert_eq!(accepted, i < WRITE_COUNT);
            if i < WRITE_COUNT {
                keys.push(key);
            }
            if i == 0 {
                assert_eq!(
                    selected.recv_timeout(Duration::from_secs(5)).unwrap(),
                    TestJob::Write(key)
                );
            }
        }
        // The first write has left the deque but remains in-flight. Admitting
        // 64 further writes here would violate the externally stated limit.
        assert_eq!(cache.stats().writeback_queued_entries, WRITE_COUNT);
        assert_eq!(cache.stats().writeback_skipped_writes, 1);
        assert!(cache.stats().writeback_peak_queued_payload_bytes <= WRITE_BYTES);
        let shared = cache.shared.clone();
        let reader = thread::spawn(move || {
            let loaded = cache.load(warm_key);
            (cache, loaded)
        });
        {
            let mut state = shared.state.lock().unwrap();
            while state.reads.is_empty() {
                let (next, timeout) = shared
                    .wake
                    .wait_timeout(state, Duration::from_secs(5))
                    .unwrap();
                state = next;
                assert!(!timeout.timed_out(), "indexed read was not queued");
            }
        }
        drop(release);
        assert_eq!(
            selected.recv_timeout(Duration::from_secs(5)).unwrap(),
            TestJob::Read(warm_key)
        );
        assert_eq!(
            selected.recv_timeout(Duration::from_secs(5)).unwrap(),
            TestJob::Write(keys[1])
        );
        let (mut cache, loaded) = reader.join().unwrap();
        assert!(loaded.is_some());
        cache.flush();
        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.writes, WRITE_COUNT as u64);
        assert_eq!(stats.writeback_peak_queued_entries, WRITE_COUNT);
        assert_eq!(stats.writeback_queued_entries, 0);
        assert_eq!(stats.writeback_queued_payload_bytes, 0);
        drop(cache);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn absent_index_is_fast_and_shutdown_drains_admitted_writeback() {
        let (world, tile, path) = fixture();
        let key = tile.key;
        let mut cache =
            AsyncDiskCache::new(path.clone(), 4 * 1024 * 1024, 1024 * 1024, &world).unwrap();
        assert!(cache.load(key).is_none());
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(
            cache.stats().read_ms,
            0.,
            "negative local index must not schedule a disk read"
        );
        assert!(cache.queue_store(tile.clone()));
        drop(cache);
        let mut warm =
            AsyncDiskCache::new(path.clone(), 4 * 1024 * 1024, 1024 * 1024, &world).unwrap();
        let loaded = warm
            .load(key)
            .expect("shutdown must finish admitted cache products");
        assert_eq!(loaded.mesh.indices, tile.mesh.indices);
        assert_eq!(loaded.face_profiles, tile.face_profiles);
        assert_eq!(loaded.certified_error_m, tile.certified_error_m);
        assert_eq!(warm.stats().hits, 1);
        assert_eq!(warm.stats().writes, 0);
        drop(warm);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn flush_releases_queued_arc_and_write_errors_do_not_poison_worker() {
        let (world, tile, path) = fixture();
        let mut cache =
            AsyncDiskCache::new(path.clone(), 4 * 1024 * 1024, 1024 * 1024, &world).unwrap();
        let weak = Arc::downgrade(&tile);
        assert!(cache.queue_store(tile));
        cache.flush();
        assert!(
            weak.upgrade().is_none(),
            "flush must release even the in-flight Arc"
        );
        assert_eq!(cache.stats().writeback_queued_payload_bytes, 0);
        assert_eq!(cache.stats().writeback_queued_entries, 0);
        assert_eq!(cache.stats().writeback_encode_buffer_reserved_bytes, 0);
        // A key revision incompatible with the immutable snapshot must fail as
        // optional cache work; subsequent valid reads must still succeed.
        let key = TileKey {
            min: [64, -512],
            width_cells: 16,
            step_cells: 4,
            edge_steps: [4; 4],
            source_revision: 0,
        };
        let mut invalid = crate::adaptive_geometry::build(&world, key, || false).unwrap();
        invalid.key.source_revision = 1;
        assert!(cache.queue_store(Arc::new(invalid)));
        cache.flush();
        assert!(cache.stats().errors >= 1);
        assert!(cache
            .load(TileKey {
                min: [0, -512],
                ..key
            })
            .is_some());
        drop(cache);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn optional_oversized_write_is_skipped_without_retaining_payload() {
        let (world, tile, path) = fixture();
        let mut cache =
            AsyncDiskCache::new(path.clone(), 4 * 1024 * 1024, 1024 * 1024, &world).unwrap();
        let mut tile = Arc::try_unwrap(tile).ok().unwrap();
        tile.bytes = WRITE_BYTES + 1;
        let tile = Arc::new(tile);
        let weak = Arc::downgrade(&tile);
        assert!(!cache.queue_store(tile));
        assert!(weak.upgrade().is_none());
        assert_eq!(cache.stats().writeback_skipped_writes, 1);
        assert_eq!(cache.stats().writeback_queued_payload_bytes, 0);
        drop(cache);
        std::fs::remove_dir_all(path).unwrap();
    }
}
