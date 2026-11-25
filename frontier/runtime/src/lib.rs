//! Small, deterministic policy contracts for the Frontier streaming runtime.
//!
//! This crate is a tested policy core, not a background streaming runtime: it
//! does not create threads, generate terrain, upload GPU resources, or publish
//! renderer state. Callers drive its explicit state transitions.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub mod benchmark;
pub mod occlusion;
pub mod representation;
pub mod routes;
pub mod travel;

/// Identifies generated content. Camera state is intentionally absent.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ContentKey {
    /// Identity of the owning world instance; equal seeds do not imply equal edits.
    pub world_instance: u64,
    pub source_version: u64,
    pub seed: u64,
    pub region: [i64; 3],
    pub revision: u64,
    pub representation: Representation,
}

/// The reusable representation requested for a region.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Representation {
    HeightPage,
    VolumeBrick,
    Mesh,
    Custom(u16),
}

/// Identity for a user-visible request, independent of content identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DemandId(pub u64);

/// Identity for one view selection. It never contributes to a content key.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewEpoch(pub u64);

/// A bounded LRU cache whose budget is expressed in caller-provided bytes.
///
/// Values with a zero byte cost are allowed. Inserting an oversized value
/// returns `false` and leaves the cache unchanged.
pub struct PageCache<K, T> {
    budget_bytes: usize,
    entry_limit: usize,
    used_bytes: usize,
    values: HashMap<K, CacheEntry<T>>,
    lru: VecDeque<K>,
}

struct CacheEntry<T> {
    value: T,
    bytes: usize,
}

impl<K: Clone + Eq + Hash, T> PageCache<K, T> {
    pub fn new(budget_bytes: usize, entry_limit: usize) -> Self {
        Self {
            budget_bytes,
            entry_limit,
            used_bytes: 0,
            values: HashMap::new(),
            lru: VecDeque::new(),
        }
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }
    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&mut self, key: &K) -> Option<&T> {
        if !self.values.contains_key(key) {
            return None;
        }
        self.touch(key);
        self.values.get(key).map(|entry| &entry.value)
    }

    /// Insert or replace an entry, evicting least-recently-used entries until
    /// both its byte cost and entry count fit. Returns false when the entry is
    /// larger than the byte budget or the cache has no entry slots.
    pub fn insert(&mut self, key: K, value: T, bytes: usize) -> bool {
        if bytes > self.budget_bytes || self.entry_limit == 0 {
            return false;
        }

        if let Some(old) = self.values.remove(&key) {
            self.used_bytes -= old.bytes;
            self.remove_from_lru(&key);
        }
        while self.used_bytes > self.budget_bytes - bytes || self.values.len() >= self.entry_limit {
            let Some(evicted) = self.lru.pop_front() else {
                break;
            };
            if let Some(entry) = self.values.remove(&evicted) {
                self.used_bytes -= entry.bytes;
            }
        }
        self.used_bytes += bytes;
        self.values.insert(key.clone(), CacheEntry { value, bytes });
        self.lru.push_back(key);
        true
    }

    pub fn remove(&mut self, key: &K) -> Option<T> {
        self.remove_from_lru(key);
        self.values.remove(key).map(|entry| {
            self.used_bytes -= entry.bytes;
            entry.value
        })
    }

    pub fn clear(&mut self) {
        self.values.clear();
        self.lru.clear();
        self.used_bytes = 0;
    }

    fn touch(&mut self, key: &K) {
        self.remove_from_lru(key);
        self.lru.push_back(key.clone());
    }

    fn remove_from_lru(&mut self, key: &K) {
        if let Some(index) = self.lru.iter().position(|candidate| candidate == key) {
            self.lru.remove(index);
        }
    }
}

/// One schedulable request. Larger priorities win; earlier deadlines break
/// ties. Deadlines are opaque caller ticks, typically a monotonic clock unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub key: ContentKey,
    pub demand: DemandId,
    pub view_epoch: ViewEpoch,
    pub priority: i32,
    pub deadline: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestStatus {
    Queued,
    Deduplicated,
    AttachedToRunning,
    IgnoredStaleEpoch,
    CapacityReached,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct JobToken {
    id: u64,
    pub key: ContentKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadyContent {
    pub key: ContentKey,
    pub demand: Option<DemandId>,
    pub view_epoch: Option<ViewEpoch>,
    /// True only if the result still belongs to active demand at completion.
    pub may_publish: bool,
    /// False when an edit invalidated this content while its job was running.
    pub may_cache: bool,
}

#[derive(Clone, Copy)]
struct Work {
    request: Request,
    order: u64,
    publishable_demand: bool,
    cacheable: bool,
}

/// Explicit queue/in-flight policy. Completed content remains reusable even
/// when its request became stale; stale work is simply not eligible to publish.
pub struct Scheduler {
    active_epoch: ViewEpoch,
    queued: HashMap<ContentKey, Work>,
    running: HashMap<u64, Work>,
    running_by_key: HashMap<ContentKey, u64>,
    next_job_id: u64,
    next_order: u64,
    queued_limit: usize,
    running_limit: usize,
}

impl Scheduler {
    pub fn new(initial_epoch: ViewEpoch, queued_limit: usize, running_limit: usize) -> Self {
        Self {
            active_epoch: initial_epoch,
            queued: HashMap::new(),
            running: HashMap::new(),
            running_by_key: HashMap::new(),
            next_job_id: 1,
            next_order: 0,
            queued_limit,
            running_limit,
        }
    }

    pub fn active_epoch(&self) -> ViewEpoch {
        self.active_epoch
    }
    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }
    pub fn running_len(&self) -> usize {
        self.running.len()
    }

    /// Switch visible demand. Queued work for the prior epoch is canceled;
    /// running work is detached but may still complete and populate a cache.
    pub fn set_view_epoch(&mut self, epoch: ViewEpoch) {
        self.active_epoch = epoch;
        self.queued
            .retain(|_, work| work.request.view_epoch == epoch);
        for work in self.running.values_mut() {
            if work.request.view_epoch != epoch {
                work.publishable_demand = false;
            }
        }
    }

    pub fn request(&mut self, request: Request) -> RequestStatus {
        if request.view_epoch != self.active_epoch {
            return RequestStatus::IgnoredStaleEpoch;
        }
        if let Some(job_id) = self.running_by_key.get(&request.key).copied() {
            if let Some(work) = self.running.get_mut(&job_id).filter(|work| work.cacheable) {
                work.request = request;
                work.publishable_demand = true;
                return RequestStatus::AttachedToRunning;
            }
        }
        if let Some(work) = self.queued.get_mut(&request.key) {
            // Keep one queue slot and make it represent the newest demand.
            work.request = request;
            return RequestStatus::Deduplicated;
        }
        if self.queued.len() >= self.queued_limit {
            return RequestStatus::CapacityReached;
        }
        let order = self.take_order();
        self.queued.insert(
            request.key,
            Work {
                request,
                order,
                publishable_demand: true,
                cacheable: true,
            },
        );
        RequestStatus::Queued
    }

    /// Invalidate one content revision after an edit. Queued work is dropped;
    /// running work may finish, but its result cannot enter a cache or publish.
    pub fn invalidate_content(&mut self, key: ContentKey) {
        self.queued.remove(&key);
        if let Some(job_id) = self.running_by_key.get(&key).copied() {
            if let Some(work) = self.running.get_mut(&job_id) {
                work.cacheable = false;
                work.publishable_demand = false;
            }
            // Future demand for this key must schedule fresh work instead of
            // attaching to a computation that began before invalidation.
            self.running_by_key.remove(&key);
        }
    }

    /// Take the highest priority request, then earliest deadline, then FIFO.
    pub fn take_next(&mut self) -> Option<JobToken> {
        if self.running.len() >= self.running_limit {
            return None;
        }
        let key = self
            .queued
            .iter()
            .max_by(|(_, a), (_, b)| {
                a.request
                    .priority
                    .cmp(&b.request.priority)
                    .then_with(|| b.request.deadline.cmp(&a.request.deadline))
                    .then_with(|| b.order.cmp(&a.order))
            })
            .map(|(key, _)| *key)?;
        let id = self.next_job_id;
        let next_job_id = self.next_job_id.checked_add(1)?;
        let work = self.queued.remove(&key)?;
        self.next_job_id = next_job_id;
        self.running.insert(id, work);
        self.running_by_key.insert(key, id);
        Some(JobToken { id, key })
    }

    /// Mark generated content ready. Callers must honor `may_cache` before
    /// caching and `may_publish` before publishing. Recheck that the key's
    /// world revision is current at publication time; invalidation does not
    /// permanently blacklist a key.
    pub fn complete(&mut self, token: JobToken) -> Option<ReadyContent> {
        if self.running.get(&token.id)?.request.key != token.key {
            return None;
        }
        let work = self.running.remove(&token.id)?;
        if self.running_by_key.get(&token.key) == Some(&token.id) {
            self.running_by_key.remove(&token.key);
        }
        let attached = work.cacheable
            && work.publishable_demand
            && work.request.view_epoch == self.active_epoch;
        Some(ReadyContent {
            key: token.key,
            demand: attached.then_some(work.request.demand),
            view_epoch: attached.then_some(work.request.view_epoch),
            may_publish: attached,
            may_cache: work.cacheable,
        })
    }

    fn take_order(&mut self) -> u64 {
        let order = self.next_order;
        self.next_order = self.next_order.checked_add(1).unwrap_or(u64::MAX);
        order
    }
}

/// Bounded rolling frame-time metrics. Invalid (negative or non-finite)
/// durations are rejected and never enter percentile calculations.
pub struct FrameStats {
    capacity: usize,
    samples: VecDeque<f64>,
    accepted_frames: u64,
    missed_budget_frames: u64,
    rejected_inputs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSummary {
    pub sample_count: usize,
    pub accepted_frames: u64,
    pub missed_budget_frames: u64,
    pub rejected_inputs: u64,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub p99_ms: Option<f64>,
}

impl FrameStats {
    pub const FRAME_BUDGET_MS: f64 = 16.6667;

    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            samples: VecDeque::with_capacity(capacity),
            accepted_frames: 0,
            missed_budget_frames: 0,
            rejected_inputs: 0,
        }
    }

    /// Returns true when the sample was accepted. A zero-capacity recorder
    /// counts frame totals but keeps no percentile sample.
    pub fn record(&mut self, frame_ms: f64) -> bool {
        if !frame_ms.is_finite() || frame_ms < 0.0 {
            self.rejected_inputs = self.rejected_inputs.saturating_add(1);
            return false;
        }
        self.accepted_frames = self.accepted_frames.saturating_add(1);
        if frame_ms > Self::FRAME_BUDGET_MS {
            self.missed_budget_frames = self.missed_budget_frames.saturating_add(1);
        }
        if self.capacity != 0 {
            if self.samples.len() == self.capacity {
                self.samples.pop_front();
            }
            self.samples.push_back(frame_ms);
        }
        true
    }

    pub fn summary(&self) -> FrameSummary {
        let mut sorted: Vec<f64> = self.samples.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        FrameSummary {
            sample_count: sorted.len(),
            accepted_frames: self.accepted_frames,
            missed_budget_frames: self.missed_budget_frames,
            rejected_inputs: self.rejected_inputs,
            p50_ms: percentile(&sorted, 0.50),
            p95_ms: percentile(&sorted, 0.95),
            p99_ms: percentile(&sorted, 0.99),
        }
    }
}

fn percentile(sorted: &[f64], percentile: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (percentile * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.saturating_sub(1).min(sorted.len() - 1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(region_x: i64, revision: u64) -> ContentKey {
        ContentKey {
            world_instance: 1,
            source_version: 3,
            seed: 42,
            region: [region_x, 0, 0],
            revision,
            representation: Representation::VolumeBrick,
        }
    }

    fn request(key: ContentKey, demand: u64, epoch: u64, priority: i32, deadline: u64) -> Request {
        Request {
            key,
            demand: DemandId(demand),
            view_epoch: ViewEpoch(epoch),
            priority,
            deadline,
        }
    }

    #[test]
    fn cache_reuses_content_across_view_epochs_and_evicts_lru() {
        let stable_content = key(0, 1);
        let mut cache = PageCache::new(8, 4);
        assert!(cache.insert(stable_content, "page", 4));
        let _ = ViewEpoch(1);
        let _ = ViewEpoch(2);
        assert_eq!(cache.get(&stable_content), Some(&"page"));
        assert!(cache.insert(key(1, 1), "second", 4));
        assert_eq!(cache.get(&stable_content), Some(&"page")); // touched, now MRU
        assert!(cache.insert(key(2, 1), "third", 4));
        assert!(cache.get(&key(1, 1)).is_none());
        assert_eq!(cache.used_bytes(), 8);
    }

    #[test]
    fn cache_rejects_oversized_and_handles_usize_limits() {
        let mut cache = PageCache::new(usize::MAX, 2);
        assert!(cache.insert(1, "kept", usize::MAX - 1));
        assert!(cache.insert(2, "fills-budget", usize::MAX));
        assert!(cache.get(&1).is_none());
        assert_eq!(cache.get(&2), Some(&"fills-budget"));
        assert_eq!(cache.used_bytes(), usize::MAX);
        let mut no_slots = PageCache::new(8, 0);
        assert!(!no_slots.insert(1, "rejected", 0));
    }

    #[test]
    fn stop_resume_and_turn_away_keep_cached_content_and_cancel_old_queue() {
        let mut scheduler = Scheduler::new(ViewEpoch(1), 16, 4);
        let content = key(0, 1);
        assert_eq!(
            scheduler.request(request(content, 10, 1, 5, 20)),
            RequestStatus::Queued
        );
        let job = scheduler.take_next().unwrap();
        scheduler.set_view_epoch(ViewEpoch(2)); // turn away while work runs
        let stale = scheduler.complete(job).unwrap();
        assert!(!stale.may_publish);
        let mut cache = PageCache::new(32, 4);
        assert!(stale.may_cache);
        assert!(cache.insert(stale.key, "finished page", 8));
        scheduler.set_view_epoch(ViewEpoch(3)); // stop/resume elsewhere
        scheduler.set_view_epoch(ViewEpoch(4)); // return to same region
        assert_eq!(cache.get(&content), Some(&"finished page"));
        assert_eq!(
            scheduler.request(request(content, 11, 4, 1, 50)),
            RequestStatus::Queued
        );
    }

    #[test]
    fn revisions_invalidate_content_and_stale_epochs_cannot_publish() {
        let old = key(5, 7);
        let edited = key(5, 8);
        assert_ne!(old, edited);
        let mut scheduler = Scheduler::new(ViewEpoch(9), 16, 4);
        scheduler.request(request(old, 1, 9, 1, 10));
        let old_job = scheduler.take_next().unwrap();
        scheduler.set_view_epoch(ViewEpoch(10));
        scheduler.request(request(edited, 2, 10, 10, 1));
        let ready_old = scheduler.complete(old_job).unwrap();
        assert!(!ready_old.may_publish);
        let new_job = scheduler.take_next().unwrap();
        assert_eq!(new_job.key, edited);
        assert!(scheduler.complete(new_job).unwrap().may_publish);
    }

    #[test]
    fn scheduler_deduplicates_and_prioritizes_priority_then_deadline() {
        let mut scheduler = Scheduler::new(ViewEpoch(1), 16, 2);
        let a = key(1, 1);
        let b = key(2, 1);
        scheduler.request(request(a, 1, 1, 3, 20));
        assert_eq!(
            scheduler.request(request(a, 2, 1, 4, 19)),
            RequestStatus::Deduplicated
        );
        scheduler.request(request(b, 3, 1, 4, 21));
        assert_eq!(scheduler.take_next().unwrap().key, a);
        let running = scheduler.take_next().unwrap();
        assert_eq!(
            scheduler.request(request(b, 4, 1, 7, 1)),
            RequestStatus::AttachedToRunning
        );
        assert_eq!(
            scheduler.complete(running).unwrap().demand,
            Some(DemandId(4))
        );
    }

    #[test]
    fn frame_stats_are_bounded_and_ignore_non_finite_values() {
        let mut stats = FrameStats::new(4);
        for sample in [10.0, 20.0, 30.0, 40.0, 50.0] {
            assert!(stats.record(sample));
        }
        assert!(!stats.record(f64::NAN));
        assert!(!stats.record(f64::INFINITY));
        assert!(!stats.record(-1.0));
        let summary = stats.summary();
        assert_eq!(summary.sample_count, 4);
        assert_eq!(summary.accepted_frames, 5);
        assert_eq!(summary.missed_budget_frames, 4);
        assert_eq!(summary.rejected_inputs, 3);
        assert_eq!(summary.p50_ms, Some(30.0));
        assert_eq!(summary.p95_ms, Some(50.0));
        assert_eq!(summary.p99_ms, Some(50.0));
        let mut empty = FrameStats::new(0);
        empty.record(1.0);
        assert_eq!(empty.summary().p50_ms, None);
    }

    #[test]
    fn edit_invalidation_blocks_old_revision_completion_without_view_change() {
        let edited_region = key(7, 12);
        let mut scheduler = Scheduler::new(ViewEpoch(2), 2, 2);
        scheduler.request(request(edited_region, 1, 2, 5, 10));
        let stale_token = scheduler.take_next().unwrap();
        scheduler.invalidate_content(edited_region);
        // A later request for the same key is fresh work, not an attachment
        // to the pre-invalidation job.
        assert_eq!(
            scheduler.request(request(edited_region, 2, 2, 5, 10)),
            RequestStatus::Queued
        );
        let fresh_token = scheduler.take_next().unwrap();
        assert_ne!(stale_token, fresh_token);
        let stale_ready = scheduler.complete(stale_token).unwrap();
        assert!(!stale_ready.may_cache);
        assert!(!stale_ready.may_publish);
        assert_eq!(scheduler.active_epoch(), ViewEpoch(2));
        let fresh_ready = scheduler.complete(fresh_token).unwrap();
        assert!(fresh_ready.may_cache);
        assert!(fresh_ready.may_publish);

        let second_world = ContentKey {
            world_instance: 2,
            ..edited_region
        };
        assert_ne!(edited_region, second_world);
    }

    #[test]
    fn scheduler_caps_queue_and_running_jobs() {
        let mut scheduler = Scheduler::new(ViewEpoch(1), 1, 1);
        assert_eq!(
            scheduler.request(request(key(1, 1), 1, 1, 1, 1)),
            RequestStatus::Queued
        );
        assert_eq!(
            scheduler.request(request(key(2, 1), 2, 1, 1, 1)),
            RequestStatus::CapacityReached
        );
        let running = scheduler.take_next().unwrap();
        assert!(scheduler.take_next().is_none());
        assert_eq!(scheduler.running_len(), 1);
        scheduler.complete(running).unwrap();
        assert_eq!(scheduler.queued_len(), 0);
    }
}
