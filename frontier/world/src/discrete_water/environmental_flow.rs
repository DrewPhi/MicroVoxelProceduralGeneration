//! Counted game-configured river boundary supply/drain clocks. This helper has
//! no geometry, occupancy or regional ledger authority. Callers transact those
//! updates only on continuing EXTERIOR halo river boundaries, never on load.
use crate::procedural_water::ReachKey;
use std::collections::{BTreeMap, BTreeSet};
const SECOND: u64 = 1_000_000_000;
const MAX_FEATURES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlowConfig {
    pub upstream_units_per_second: u64,
    pub downstream_units_per_second: u64,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlowCounters {
    pub upstream_attempted: u64,
    pub upstream_added: u64,
    pub upstream_suppressed: u64,
    pub downstream_attempted: u64,
    pub downstream_removed: u64,
    pub downstream_unsatisfied: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlowBudget {
    /// Canonical channel key: source version + owner, with segment always zero.
    pub key: ReachKey,
    pub serial: u64,
    pub upstream_attempts: u64,
    pub downstream_attempts: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlowFeatureState {
    pub lake: bool,
    pub config: FlowConfig,
    /// Fraction of an attempted whole unit, in billionths. This is a clock.
    pub upstream_phase: u64,
    pub downstream_phase: u64,
    pub active_ns: u64,
    /// Counts describe settled budgets; pending attempts are separately visible.
    pub counters: FlowCounters,
    pub pending: Option<FlowBudget>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentalFlow {
    max_features: usize,
    serial: u64,
    features: BTreeMap<ReachKey, FlowFeatureState>,
}
/// Sixteen geometric segments of one drainage edge share ONE supply clock.
pub fn canonical_key(key: ReachKey) -> ReachKey {
    ReachKey { segment: 0, ..key }
}
fn config_valid(lake: bool, c: FlowConfig) -> bool {
    c.upstream_units_per_second <= SECOND
        && c.downstream_units_per_second <= SECOND
        && (!lake || c == FlowConfig::default())
}
fn increment(value: u64, amount: u64) -> Result<u64, String> {
    value
        .checked_add(amount)
        .ok_or_else(|| "environmental flow counter overflow".into())
}
impl EnvironmentalFlow {
    pub fn new(max_features: usize) -> Result<Self, String> {
        if max_features == 0 || max_features > MAX_FEATURES {
            return Err("environmental flow feature budget outside1..4096".into());
        }
        Ok(Self {
            max_features,
            serial: 0,
            features: BTreeMap::new(),
        })
    }
    /// Registers clock metadata only. Idempotent configuration never refills,
    /// resets a clock, advances time or changes any water count.
    pub fn configure(
        &mut self,
        key: ReachKey,
        lake: bool,
        config: FlowConfig,
    ) -> Result<(), String> {
        if !config_valid(lake, config) {
            return Err("invalid environmental rate or finite-lake supply".into());
        }
        let key = canonical_key(key);
        if let Some(state) = self.features.get_mut(&key) {
            if state.lake != lake {
                return Err(
                    "environmental channel kind changed without source-version change".into(),
                );
            }
            if state.config != config {
                if state.pending.is_some() {
                    return Err("settle pending flow before changing rate".into());
                }
                state.config = config;
            }
            return Ok(());
        }
        if self.features.len() >= self.max_features {
            return Err("environmental flow feature budget exhausted".into());
        }
        self.features.insert(
            key,
            FlowFeatureState {
                lake,
                config,
                upstream_phase: 0,
                downstream_phase: 0,
                active_ns: 0,
                counters: FlowCounters::default(),
                pending: None,
            },
        );
        Ok(())
    }
    pub fn feature(&self, key: ReachKey) -> Option<&FlowFeatureState> {
        self.features.get(&canonical_key(key))
    }
    pub fn features(&self) -> impl Iterator<Item = (ReachKey, &FlowFeatureState)> {
        self.features.iter().map(|(&k, v)| (k, v))
    }
    pub fn pending_budgets(&self) -> impl Iterator<Item = FlowBudget> + '_ {
        self.features.values().filter_map(|s| s.pending)
    }
    /// Advance only the supplied active channels by ACTUAL simulated time.
    /// Duplicate/subsegment keys collapse to a single channel. Inactive clocks
    /// freeze; there is no offline or wall-clock catch-up. At most1s per call.
    pub fn advance(
        &mut self,
        active: &[ReachKey],
        elapsed_ns: u64,
    ) -> Result<Vec<FlowBudget>, String> {
        if elapsed_ns > SECOND {
            return Err("environmental advance exceeds one second".into());
        }
        if self.features.values().any(|s| s.pending.is_some()) {
            return Err("unsettled environmental budget".into());
        }
        let active: BTreeSet<_> = active.iter().copied().map(canonical_key).collect();
        if active.iter().any(|k| !self.features.contains_key(k)) {
            return Err("unconfigured environmental channel".into());
        }
        if elapsed_ns == 0 {
            return Ok(Vec::new());
        }
        let mut next = self.clone();
        let mut budgets = Vec::new();
        for key in active {
            let s = next.features.get_mut(&key).unwrap();
            s.active_ns = increment(s.active_ns, elapsed_ns)?;
            let upstream = s.upstream_phase as u128
                + s.config.upstream_units_per_second as u128 * elapsed_ns as u128;
            let downstream = s.downstream_phase as u128
                + s.config.downstream_units_per_second as u128 * elapsed_ns as u128;
            s.upstream_phase = (upstream % SECOND as u128) as u64;
            s.downstream_phase = (downstream % SECOND as u128) as u64;
            let (upstream_attempts, downstream_attempts) = (
                (upstream / SECOND as u128) as u64,
                (downstream / SECOND as u128) as u64,
            );
            if upstream_attempts != 0 || downstream_attempts != 0 {
                next.serial = increment(next.serial, 1)?;
                let budget = FlowBudget {
                    key,
                    serial: next.serial,
                    upstream_attempts,
                    downstream_attempts,
                };
                s.pending = Some(budget);
                budgets.push(budget);
            }
        }
        *self = next;
        Ok(budgets)
    }
    /// Settle actual accepted whole units after the caller's staged legal-cell
    /// and regional transaction. All unaccepted attempts are discarded/countable,
    /// never banked for a later burst. Duplicate/stale/tampered tokens reject.
    pub fn settle(
        &mut self,
        budget: FlowBudget,
        accepted_upstream: u64,
        accepted_downstream: u64,
    ) -> Result<(), String> {
        let mut s = *self
            .features
            .get(&budget.key)
            .ok_or("unknown environmental budget channel")?;
        if s.pending != Some(budget)
            || accepted_upstream > budget.upstream_attempts
            || accepted_downstream > budget.downstream_attempts
        {
            return Err("stale/duplicate/excess environmental settlement".into());
        }
        let c = &mut s.counters;
        c.upstream_attempted = increment(c.upstream_attempted, budget.upstream_attempts)?;
        c.upstream_added = increment(c.upstream_added, accepted_upstream)?;
        c.upstream_suppressed = increment(
            c.upstream_suppressed,
            budget.upstream_attempts - accepted_upstream,
        )?;
        c.downstream_attempted = increment(c.downstream_attempted, budget.downstream_attempts)?;
        c.downstream_removed = increment(c.downstream_removed, accepted_downstream)?;
        c.downstream_unsatisfied = increment(
            c.downstream_unsatisfied,
            budget.downstream_attempts - accepted_downstream,
        )?;
        s.pending = None;
        self.features.insert(budget.key, s);
        Ok(())
    }
    pub fn snapshot(&self) -> Vec<u8> {
        let mut bytes = b"DWENV001".to_vec();
        let mut put = |v: u64| bytes.extend(v.to_le_bytes());
        for v in [
            self.max_features as u64,
            self.serial,
            self.features.len() as u64,
        ] {
            put(v);
        }
        for (&k, s) in &self.features {
            let c = s.counters;
            let p = s.pending;
            for v in [
                k.source_version as u64,
                k.owner[0] as u64,
                k.owner[1] as u64,
                k.segment as u64,
                u64::from(s.lake),
                s.config.upstream_units_per_second,
                s.config.downstream_units_per_second,
                s.upstream_phase,
                s.downstream_phase,
                s.active_ns,
                c.upstream_attempted,
                c.upstream_added,
                c.upstream_suppressed,
                c.downstream_attempted,
                c.downstream_removed,
                c.downstream_unsatisfied,
                u64::from(p.is_some()),
                p.map_or(0, |v| v.serial),
                p.map_or(0, |v| v.upstream_attempts),
                p.map_or(0, |v| v.downstream_attempts),
            ] {
                put(v);
            }
        }
        let hash = super::checksum(&bytes);
        bytes.extend(hash.to_le_bytes());
        bytes
    }
    pub fn restore(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 40 || &bytes[..8] != b"DWENV001" {
            return Err("invalid environmental flow checkpoint".into());
        }
        let end = bytes.len() - 8;
        if super::checksum(&bytes[..end]) != u64::from_le_bytes(bytes[end..].try_into().unwrap()) {
            return Err("environmental flow checksum mismatch".into());
        }
        let mut at = 8;
        let mut get = || {
            let v = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
            at += 8;
            v
        };
        let max = usize::try_from(get()).map_err(|_| "invalid environmental feature budget")?;
        let serial = get();
        let count = get();
        let mut out = Self::new(max)?;
        out.serial = serial;
        if count > max as u64 || end != 32 + count as usize * 160 {
            return Err("invalid environmental state size".into());
        }
        let mut pending_ids = BTreeSet::new();
        for _ in 0..count {
            let source_version =
                u32::try_from(get()).map_err(|_| "invalid environmental source version")?;
            let owner = [get() as i64, get() as i64];
            let segment = get();
            let lake = get();
            if segment != 0 || lake > 1 {
                return Err("noncanonical environmental channel key/kind".into());
            }
            let key = ReachKey {
                source_version,
                owner,
                segment: 0,
            };
            let config = FlowConfig {
                upstream_units_per_second: get(),
                downstream_units_per_second: get(),
            };
            let upstream_phase = get();
            let downstream_phase = get();
            let active_ns = get();
            let counters = FlowCounters {
                upstream_attempted: get(),
                upstream_added: get(),
                upstream_suppressed: get(),
                downstream_attempted: get(),
                downstream_removed: get(),
                downstream_unsatisfied: get(),
            };
            let pending = get();
            let pserial = get();
            let upstream_attempts = get();
            let downstream_attempts = get();
            if !config_valid(lake == 1, config)
                || upstream_phase >= SECOND
                || downstream_phase >= SECOND
                || counters.upstream_attempted as u128
                    != counters.upstream_added as u128 + counters.upstream_suppressed as u128
                || counters.downstream_attempted as u128
                    != counters.downstream_removed as u128 + counters.downstream_unsatisfied as u128
                || pending > 1
            {
                return Err("invalid environmental rate/phase/counters".into());
            }
            if lake == 1
                && (upstream_phase != 0
                    || downstream_phase != 0
                    || counters != FlowCounters::default())
            {
                return Err("finite lake has environmental flow history".into());
            }
            let budget = if pending == 1 {
                if pserial == 0
                    || pserial > serial
                    || !pending_ids.insert(pserial)
                    || upstream_attempts > SECOND
                    || downstream_attempts > SECOND
                    || (upstream_attempts == 0 && downstream_attempts == 0)
                    || lake == 1
                {
                    return Err("invalid outstanding environmental budget".into());
                }
                Some(FlowBudget {
                    key,
                    serial: pserial,
                    upstream_attempts,
                    downstream_attempts,
                })
            } else {
                if pserial != 0 || upstream_attempts != 0 || downstream_attempts != 0 {
                    return Err("disabled environmental budget has data".into());
                }
                None
            };
            if out
                .features
                .insert(
                    key,
                    FlowFeatureState {
                        lake: lake == 1,
                        config,
                        upstream_phase,
                        downstream_phase,
                        active_ns,
                        counters,
                        pending: budget,
                    },
                )
                .is_some()
            {
                return Err("duplicate environmental channel".into());
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(segment: u8) -> ReachKey {
        ReachKey {
            source_version: 7,
            owner: [-12, 43],
            segment,
        }
    }
    fn configured() -> EnvironmentalFlow {
        let mut flow = EnvironmentalFlow::new(8).unwrap();
        flow.configure(
            key(3),
            false,
            FlowConfig {
                upstream_units_per_second: 7,
                downstream_units_per_second: 3,
            },
        )
        .unwrap();
        flow
    }
    fn tick(flow: &mut EnvironmentalFlow, keys: &[ReachKey], ns: u64, accept: bool) {
        for b in flow.advance(keys, ns).unwrap() {
            flow.settle(
                b,
                if accept { b.upstream_attempts } else { 0 },
                if accept { b.downstream_attempts } else { 0 },
            )
            .unwrap();
        }
    }
    #[test]
    fn sixteen_geometric_segments_have_one_channel_clock_and_budget() {
        let mut a = configured();
        let mut b = configured();
        let segments: Vec<_> = (0..16).map(key).collect();
        for k in &segments {
            b.configure(
                *k,
                false,
                FlowConfig {
                    upstream_units_per_second: 7,
                    downstream_units_per_second: 3,
                },
            )
            .unwrap();
        }
        for _ in 0..100 {
            tick(&mut a, &[key(9)], 100_000_000, true);
            tick(&mut b, &segments, 100_000_000, true);
            assert_eq!(a.snapshot(), b.snapshot());
        }
        let s = a.feature(key(15)).unwrap();
        assert_eq!(s.counters.upstream_added, 70);
        assert_eq!(s.counters.downstream_removed, 30);
        assert_eq!(a.features().count(), 1);
    }
    #[test]
    fn phase_and_counters_are_independent_of_simulated_time_partition() {
        let mut a = configured();
        let mut b = configured();
        for _ in 0..10 {
            tick(&mut a, &[key(0)], 100_000_000, true);
        }
        for _ in 0..1000 {
            tick(&mut b, &[key(15)], 1_000_000, true);
        }
        let x = a.feature(key(0)).unwrap();
        let y = b.feature(key(0)).unwrap();
        assert_eq!(x.counters, y.counters);
        assert_eq!(x.active_ns, y.active_ns);
        assert_eq!(x.upstream_phase, y.upstream_phase);
        assert_eq!(x.downstream_phase, y.downstream_phase);
        assert_eq!(x.counters.upstream_added, 7);
        assert_eq!(x.counters.downstream_removed, 3);
    }
    #[test]
    fn blocked_supply_does_not_bank_and_registration_inactivity_never_refill() {
        let mut flow = configured();
        let initial = flow.snapshot();
        for _ in 0..20 {
            flow.configure(
                key(12),
                false,
                FlowConfig {
                    upstream_units_per_second: 7,
                    downstream_units_per_second: 3,
                },
            )
            .unwrap();
            tick(&mut flow, &[], SECOND, true);
        }
        assert_eq!(flow.snapshot(), initial);
        for _ in 0..100 {
            tick(&mut flow, &[key(0)], SECOND, false);
        }
        let s = flow.feature(key(0)).unwrap();
        assert_eq!(s.counters.upstream_added, 0);
        assert_eq!(s.counters.upstream_suppressed, 700);
        assert_eq!(s.counters.downstream_unsatisfied, 300);
        assert!(flow.advance(&[key(0)], 1_000_000).unwrap().is_empty());
        assert_eq!(flow.feature(key(0)).unwrap().counters.upstream_added, 0);
    }
    #[test]
    fn settlements_reject_duplicate_stale_excess_and_tampered_budgets_atomically() {
        let mut flow = configured();
        let budget = flow.advance(&[key(0)], SECOND).unwrap()[0];
        let before = flow.snapshot();
        assert!(flow.advance(&[key(0)], SECOND).is_err());
        assert_eq!(before, flow.snapshot());
        assert!(flow.settle(budget, 8, 0).is_err());
        assert_eq!(before, flow.snapshot());
        let forged = FlowBudget {
            serial: budget.serial + 1,
            ..budget
        };
        assert!(flow.settle(forged, 1, 1).is_err());
        assert_eq!(before, flow.snapshot());
        flow.settle(budget, 2, 1).unwrap();
        let settled = flow.snapshot();
        assert!(flow.settle(budget, 2, 1).is_err());
        assert_eq!(settled, flow.snapshot());
        let c = flow.feature(key(0)).unwrap().counters;
        assert_eq!(
            c.upstream_added + c.upstream_suppressed,
            c.upstream_attempted
        );
        assert_eq!(
            c.downstream_removed + c.downstream_unsatisfied,
            c.downstream_attempted
        );
    }
    #[test]
    fn pending_and_settled_restart_continuation_are_bit_exact() {
        let mut a = configured();
        tick(&mut a, &[key(0)], 37_000_001, false);
        let budget = a.advance(&[key(0)], SECOND).unwrap()[0];
        let mut b = EnvironmentalFlow::restore(&a.snapshot()).unwrap();
        assert_eq!(b.pending_budgets().collect::<Vec<_>>(), vec![budget]);
        a.settle(budget, 3, 2).unwrap();
        b.settle(budget, 3, 2).unwrap();
        for _ in 0..100 {
            tick(&mut a, &[key(0)], 33_333_333, true);
            tick(&mut b, &[key(5)], 33_333_333, true);
            assert_eq!(a.snapshot(), b.snapshot());
        }
        let mut bytes = a.snapshot();
        bytes[40] ^= 1;
        assert!(EnvironmentalFlow::restore(&bytes).is_err());
        for n in 0..40 {
            assert!(EnvironmentalFlow::restore(&bytes[..n]).is_err());
        }
    }
    #[test]
    fn lakes_remain_finite_and_feature_work_is_bounded() {
        let mut flow = EnvironmentalFlow::new(1).unwrap();
        assert!(flow
            .configure(
                key(0),
                true,
                FlowConfig {
                    upstream_units_per_second: 1,
                    downstream_units_per_second: 0
                }
            )
            .is_err());
        flow.configure(key(0), true, FlowConfig::default()).unwrap();
        for _ in 0..10 {
            assert!(flow.advance(&[key(0)], SECOND).unwrap().is_empty());
        }
        assert_eq!(
            flow.feature(key(0)).unwrap().counters,
            FlowCounters::default()
        );
        let other = ReachKey {
            owner: [0, 0],
            ..key(0)
        };
        assert!(flow.configure(other, false, FlowConfig::default()).is_err());
        let before = flow.snapshot();
        assert!(flow.advance(&[key(0)], SECOND + 1).is_err());
        assert_eq!(before, flow.snapshot());
        assert!(flow
            .configure(key(0), false, FlowConfig::default())
            .is_err());
    }
}
