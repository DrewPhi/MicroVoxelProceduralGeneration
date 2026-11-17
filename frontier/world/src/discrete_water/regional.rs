//! Integer regional reservoir accounting, independent of terrain routing/rendering.
//! Volumes use complete native water cells. Rate phases are clocks, never mass.
use std::collections::BTreeMap;

pub type ReservoirId = u32;
pub type RegionId = u64;
const MAX_ENTRIES: usize = 8192;
const SECOND: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegionalLedger {
    pub initial: u64,
    pub supplied: u64,
    pub drained: u64,
    pub suppressed: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rate {
    units_per_second: u64,
    phase: u64,
}
impl Rate {
    fn new(rate: u64) -> Result<Self, String> {
        if rate == 0 || rate > SECOND {
            return Err("regional rate outside 1..=1e9 cells/s".into());
        }
        Ok(Self {
            units_per_second: rate,
            phase: 0,
        })
    }
    fn attempts(&mut self, ns: u64) -> u64 {
        let total = self.phase as u128 + self.units_per_second as u128 * ns as u128;
        self.phase = (total % SECOND as u128) as u64;
        (total / SECOND as u128) as u64
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservoir {
    pub capacity: u64,
    pub volume: u64,
    /// Storage reserved by detailed regions, unavailable to coarse connections.
    pub held: u64,
    /// Declared routing head; this foundation does not derive river geometry.
    pub head_mm: i64,
    source: Option<Rate>,
    outlet: Option<Rate>,
}
impl Reservoir {
    fn room(&self) -> u64 {
        self.capacity - self.volume - self.held
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnershipToken {
    pub region: RegionId,
    pub reservoir: ReservoirId,
    pub serial: u64,
    pub units: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Connection {
    from: ReservoirId,
    to: ReservoirId,
    rate: Rate,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionalWater {
    reservoirs: BTreeMap<ReservoirId, Reservoir>,
    connections: BTreeMap<u32, Connection>,
    owners: BTreeMap<RegionId, OwnershipToken>,
    ledger: RegionalLedger,
    tick: u64,
    serial: u64,
    last_transfer: u64,
}
impl RegionalWater {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn ledger(&self) -> RegionalLedger {
        self.ledger
    }
    /// High-water mark for accepted explicit ownership transfers.
    pub fn last_transfer_id(&self) -> u64 {
        self.last_transfer
    }
    pub fn tick(&self) -> u64 {
        self.tick
    }
    pub fn reservoir(&self, id: ReservoirId) -> Option<&Reservoir> {
        self.reservoirs.get(&id)
    }
    pub fn ownership(&self, region: RegionId) -> Option<OwnershipToken> {
        self.owners.get(&region).copied()
    }
    pub fn stored_units(&self) -> u64 {
        self.reservoirs.values().map(|r| r.volume).sum()
    }
    pub fn detailed_units(&self) -> u64 {
        self.owners.values().map(|t| t.units).sum()
    }
    pub fn conserved(&self) -> bool {
        self.ledger.initial as u128 + self.ledger.supplied as u128
            == self.stored_units() as u128
                + self.detailed_units() as u128
                + self.ledger.drained as u128
    }
    pub fn add_reservoir(
        &mut self,
        id: ReservoirId,
        capacity: u64,
        volume: u64,
        head_mm: i64,
    ) -> Result<(), String> {
        if self.reservoirs.contains_key(&id)
            || self.reservoirs.len() >= MAX_ENTRIES
            || volume > capacity
        {
            return Err("invalid/duplicate regional reservoir".into());
        }
        let initial = self
            .ledger
            .initial
            .checked_add(volume)
            .ok_or("regional ledger overflow")?;
        // Sum of capacities is bounded so summing all stored/held units is safe.
        self.reservoirs
            .values()
            .try_fold(capacity, |sum, r| sum.checked_add(r.capacity))
            .ok_or("regional capacity overflow")?;
        self.reservoirs.insert(
            id,
            Reservoir {
                capacity,
                volume,
                held: 0,
                head_mm,
                source: None,
                outlet: None,
            },
        );
        self.ledger.initial = initial;
        Ok(())
    }
    /// Change geometric storage capacity without creating or removing water.
    /// Exact native tiles can grow their empty space after a dry terrain edit.
    pub fn set_capacity(&mut self, id: ReservoirId, capacity: u64) -> Result<(), String> {
        let current = self.reservoirs.get(&id).ok_or("unknown reservoir")?;
        if capacity < current.volume + current.held {
            return Err("capacity would discard stored or detailed water".into());
        }
        self.reservoirs
            .iter()
            .filter(|(key, _)| **key != id)
            .try_fold(capacity, |sum, (_, r)| sum.checked_add(r.capacity))
            .ok_or("regional capacity overflow")?;
        self.reservoirs.get_mut(&id).unwrap().capacity = capacity;
        Ok(())
    }
    pub fn set_source(
        &mut self,
        id: ReservoirId,
        units_per_second: Option<u64>,
    ) -> Result<(), String> {
        let rate = units_per_second.map(Rate::new).transpose()?;
        self.reservoirs
            .get_mut(&id)
            .ok_or("unknown regional reservoir")?
            .source = rate;
        Ok(())
    }
    pub fn set_outlet(
        &mut self,
        id: ReservoirId,
        units_per_second: Option<u64>,
    ) -> Result<(), String> {
        let rate = units_per_second.map(Rate::new).transpose()?;
        self.reservoirs
            .get_mut(&id)
            .ok_or("unknown regional reservoir")?
            .outlet = rate;
        Ok(())
    }
    pub fn connect(
        &mut self,
        id: u32,
        from: ReservoirId,
        to: ReservoirId,
        units_per_second: u64,
    ) -> Result<(), String> {
        if from == to
            || !self.reservoirs.contains_key(&from)
            || !self.reservoirs.contains_key(&to)
            || self.connections.contains_key(&id)
            || self.connections.len() >= MAX_ENTRIES
        {
            return Err("invalid/duplicate regional connection".into());
        }
        self.connections.insert(
            id,
            Connection {
                from,
                to,
                rate: Rate::new(units_per_second)?,
            },
        );
        Ok(())
    }
    fn transaction<T>(
        &mut self,
        apply: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut next = self.clone();
        let value = apply(&mut next)?;
        debug_assert!(next.conserved());
        *self = next;
        Ok(value)
    }
    fn fresh_transfer(&mut self, id: u64) -> Result<(), String> {
        if id <= self.last_transfer {
            return Err("duplicate or stale regional transfer".into());
        }
        self.last_transfer = id;
        Ok(())
    }
    fn move_units(&mut self, from: ReservoirId, to: ReservoirId, units: u64) -> Result<(), String> {
        if from == to {
            return Err("regional transfer has identical endpoints".into());
        }
        let a = self
            .reservoirs
            .get(&from)
            .ok_or("unknown regional transfer source")?;
        let b = self
            .reservoirs
            .get(&to)
            .ok_or("unknown regional transfer target")?;
        if units > a.volume || units > b.room() {
            return Err("regional transfer lacks water/capacity".into());
        }
        self.reservoirs.get_mut(&from).unwrap().volume -= units;
        self.reservoirs.get_mut(&to).unwrap().volume += units;
        Ok(())
    }
    /// Globally monotone application ID. Retrying/reordering an applied ID is an
    /// error with no mutation; callers must retain their acknowledgment.
    pub fn transfer(
        &mut self,
        id: u64,
        from: ReservoirId,
        to: ReservoirId,
        units: u64,
    ) -> Result<(), String> {
        self.transaction(|w| {
            w.fresh_transfer(id)?;
            w.move_units(from, to, units)
        })
    }
    /// Ordered conservative moves under one replay-protected transaction ID.
    /// One staged graph copy covers the whole batch; any failed leg rolls back
    /// all prior legs and the ID. Use vacancy-propagation order for full nodes.
    pub fn transfer_batch(
        &mut self,
        id: u64,
        moves: &[(ReservoirId, ReservoirId, u64)],
    ) -> Result<(), String> {
        if moves.is_empty() || moves.len() > 65536 {
            return Err("invalid regional transfer batch length".into());
        }
        self.transaction(|world| {
            world.fresh_transfer(id)?;
            for &(from, to, units) in moves {
                world.move_units(from, to, units)?;
            }
            Ok(())
        })
    }
    /// Counted exchange with an explicitly modeled environment, not regeneration.
    /// Entries are (reservoir, supplied units, drained units); drains happen
    /// before supplies per entry. Caller applies the same changes to native
    /// cells in its enclosing transaction. A single ID protects the whole batch.
    pub fn external_exchange(
        &mut self,
        id: u64,
        changes: &[(ReservoirId, u64, u64)],
        suppressed_supply: u64,
    ) -> Result<(), String> {
        if changes.len() > MAX_ENTRIES {
            return Err("external exchange batch too large".into());
        }
        self.transaction(|world| {
            world.fresh_transfer(id)?;
            for &(reservoir, supplied, drained) in changes {
                let r = world
                    .reservoirs
                    .get_mut(&reservoir)
                    .ok_or("unknown external reservoir")?;
                if drained > r.volume {
                    return Err("external drain exceeds actual stored water".into());
                }
                r.volume -= drained;
                if supplied > r.room() {
                    return Err("external supply exceeds free capacity".into());
                }
                r.volume += supplied;
                world.ledger.supplied = world
                    .ledger
                    .supplied
                    .checked_add(supplied)
                    .ok_or("supply ledger overflow")?;
                world.ledger.drained = world
                    .ledger
                    .drained
                    .checked_add(drained)
                    .ok_or("drain ledger overflow")?;
            }
            world.ledger.suppressed = world
                .ledger
                .suppressed
                .checked_add(suppressed_supply)
                .ok_or("suppressed supply overflow")?;
            Ok(())
        })
    }
    /// At most one second per call bounds supply arithmetic. Sources, directed
    /// connections in ID order, then sinks. A connection runs once per call;
    /// directed chains may carry supply downstream in that deterministic order.
    pub fn advance(&mut self, elapsed_ns: u64) -> Result<(), String> {
        if elapsed_ns > SECOND {
            return Err("regional tick exceeds one second".into());
        }
        if elapsed_ns == 0 {
            return Ok(());
        }
        self.transaction(|w| {
            for reservoir in w.reservoirs.values_mut() {
                if let Some(rate) = &mut reservoir.source {
                    let attempted = rate.attempts(elapsed_ns);
                    let supplied = attempted.min(reservoir.room());
                    reservoir.volume += supplied;
                    w.ledger.supplied = w
                        .ledger
                        .supplied
                        .checked_add(supplied)
                        .ok_or("regional supply overflow")?;
                    w.ledger.suppressed = w
                        .ledger
                        .suppressed
                        .checked_add(attempted - supplied)
                        .ok_or("regional suppression overflow")?;
                }
            }
            let connections: Vec<_> = w
                .connections
                .values_mut()
                .map(|c| (c.from, c.to, c.rate.attempts(elapsed_ns)))
                .collect();
            for (from, to, attempted) in connections {
                let a = &w.reservoirs[&from];
                let b = &w.reservoirs[&to];
                if a.head_mm > b.head_mm {
                    let units = attempted.min(a.volume).min(b.room());
                    w.move_units(from, to, units)?;
                }
            }
            for reservoir in w.reservoirs.values_mut() {
                if let Some(rate) = &mut reservoir.outlet {
                    let drained = rate.attempts(elapsed_ns).min(reservoir.volume);
                    reservoir.volume -= drained;
                    w.ledger.drained = w
                        .ledger
                        .drained
                        .checked_add(drained)
                        .ok_or("regional drain overflow")?;
                }
            }
            w.tick = w.tick.checked_add(1).ok_or("regional tick overflow")?;
            Ok(())
        })
    }
    fn next_serial(&mut self) -> Result<u64, String> {
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or("regional token overflow")?;
        Ok(self.serial)
    }
    fn verify(&self, token: OwnershipToken) -> Result<(), String> {
        if self.owners.get(&token.region) != Some(&token) {
            return Err("stale/duplicate detailed ownership token".into());
        }
        Ok(())
    }
    /// Reserve exact volume and return capacity for one detailed domain.
    pub fn take_region(
        &mut self,
        region: RegionId,
        reservoir: ReservoirId,
        units: u64,
    ) -> Result<OwnershipToken, String> {
        self.transaction(|w| {
            if w.owners.contains_key(&region) || w.owners.len() >= MAX_ENTRIES {
                return Err("detailed region already owned or ownership budget exceeded".into());
            }
            let r = w
                .reservoirs
                .get_mut(&reservoir)
                .ok_or("unknown detailed reservoir")?;
            if units > r.volume {
                return Err("insufficient volume for detailed region".into());
            }
            r.volume -= units;
            r.held += units;
            let token = OwnershipToken {
                region,
                reservoir,
                serial: w.next_serial()?,
                units,
            };
            w.owners.insert(region, token);
            Ok(token)
        })
    }
    /// Caller supplies its exact occupied-cell count; mismatch rejects instead
    /// of fabricating or silently discarding detailed water.
    pub fn return_region(
        &mut self,
        token: OwnershipToken,
        occupied_units: u64,
    ) -> Result<(), String> {
        self.transaction(|w| {
            w.verify(token)?;
            if token.units != occupied_units {
                return Err("detailed region mass differs from ownership ledger".into());
            }
            let r = w.reservoirs.get_mut(&token.reservoir).unwrap();
            r.held -= token.units;
            r.volume += token.units;
            w.owners.remove(&token.region);
            Ok(())
        })
    }
    /// Boundary exchange must be paired atomically by the caller with the local
    /// whole-cell transaction. Refreshes the token so stale snapshots cannot
    /// later return or exchange the old ownership amount a second time.
    pub fn exchange_owned(
        &mut self,
        id: u64,
        token: OwnershipToken,
        reservoir: ReservoirId,
        units: u64,
        into_detail: bool,
    ) -> Result<OwnershipToken, String> {
        self.transaction(|w| {
            w.fresh_transfer(id)?;
            w.verify(token)?;
            if !w.reservoirs.contains_key(&reservoir) {
                return Err("unknown exchange reservoir".into());
            }
            if into_detail {
                if units > w.reservoirs[&reservoir].volume {
                    return Err("detailed import lacks regional supply".into());
                }
                // Free source volume first: importing from the owning reservoir
                // reuses that same reserved capacity.
                w.reservoirs.get_mut(&reservoir).unwrap().volume -= units;
                if units > w.reservoirs[&token.reservoir].room() {
                    return Err("detailed ownership exceeds reserved reservoir capacity".into());
                }
                w.reservoirs.get_mut(&token.reservoir).unwrap().held += units;
            } else {
                if units > token.units {
                    return Err("detailed export exceeds owned water".into());
                }
                w.reservoirs.get_mut(&token.reservoir).unwrap().held -= units;
                if units > w.reservoirs[&reservoir].room() {
                    return Err("detailed export lacks reservoir capacity".into());
                }
                w.reservoirs.get_mut(&reservoir).unwrap().volume += units;
            }
            let next = OwnershipToken {
                serial: w.next_serial()?,
                units: if into_detail {
                    token.units + units
                } else {
                    token.units - units
                },
                ..token
            };
            w.owners.insert(token.region, next);
            Ok(next)
        })
    }
    pub fn snapshot(&self) -> Vec<u8> {
        let mut bytes = b"DWRGN001".to_vec();
        let mut put = |value: u64| bytes.extend(value.to_le_bytes());
        for value in [
            self.tick,
            self.serial,
            self.last_transfer,
            self.ledger.initial,
            self.ledger.supplied,
            self.ledger.drained,
            self.ledger.suppressed,
            self.reservoirs.len() as u64,
            self.connections.len() as u64,
            self.owners.len() as u64,
        ] {
            put(value);
        }
        for (&id, r) in &self.reservoirs {
            for value in [
                id as u64,
                r.capacity,
                r.volume,
                r.held,
                r.head_mm as u64,
                r.source.map_or(0, |v| v.units_per_second),
                r.source.map_or(0, |v| v.phase),
                r.outlet.map_or(0, |v| v.units_per_second),
                r.outlet.map_or(0, |v| v.phase),
            ] {
                put(value);
            }
        }
        for (&id, c) in &self.connections {
            for value in [
                id as u64,
                c.from as u64,
                c.to as u64,
                c.rate.units_per_second,
                c.rate.phase,
            ] {
                put(value);
            }
        }
        for token in self.owners.values() {
            for value in [
                token.region,
                token.reservoir as u64,
                token.serial,
                token.units,
            ] {
                put(value);
            }
        }
        let checksum = super::checksum(&bytes);
        bytes.extend(checksum.to_le_bytes());
        bytes
    }
    pub fn restore(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 96 || &bytes[..8] != b"DWRGN001" {
            return Err("invalid/truncated regional snapshot".into());
        }
        let end = bytes.len() - 8;
        if super::checksum(&bytes[..end]) != u64::from_le_bytes(bytes[end..].try_into().unwrap()) {
            return Err("regional snapshot checksum mismatch".into());
        }
        let mut pos = 8;
        let mut get = || -> Result<u64, String> {
            if pos + 8 > end {
                return Err("truncated regional state".into());
            }
            let value = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            Ok(value)
        };
        let mut w = Self::new();
        w.tick = get()?;
        w.serial = get()?;
        w.last_transfer = get()?;
        w.ledger = RegionalLedger {
            initial: get()?,
            supplied: get()?,
            drained: get()?,
            suppressed: get()?,
        };
        let counts = [get()?, get()?, get()?];
        if counts.iter().any(|&v| v > MAX_ENTRIES as u64)
            || end
                != 88 + counts[0] as usize * 72 + counts[1] as usize * 40 + counts[2] as usize * 32
        {
            return Err("invalid regional snapshot size".into());
        }
        let id32 =
            |v| u32::try_from(v).map_err(|_| String::from("invalid reservoir/connection ID"));
        let rate = |units, phase| -> Result<Option<Rate>, String> {
            if units == 0 {
                if phase != 0 {
                    return Err("invalid disabled rate phase".into());
                }
                Ok(None)
            } else {
                let mut rate = Rate::new(units)?;
                if phase >= SECOND {
                    return Err("invalid regional rate phase".into());
                }
                rate.phase = phase;
                Ok(Some(rate))
            }
        };
        let mut capacity_sum = 0u64;
        for _ in 0..counts[0] {
            let id = id32(get()?)?;
            let capacity = get()?;
            let volume = get()?;
            let held = get()?;
            let head_mm = get()? as i64;
            let source = rate(get()?, get()?)?;
            let outlet = rate(get()?, get()?)?;
            capacity_sum = capacity_sum
                .checked_add(capacity)
                .ok_or("regional capacity overflow")?;
            if volume as u128 + held as u128 > capacity as u128
                || w.reservoirs
                    .insert(
                        id,
                        Reservoir {
                            capacity,
                            volume,
                            held,
                            head_mm,
                            source,
                            outlet,
                        },
                    )
                    .is_some()
            {
                return Err("invalid/duplicate regional reservoir state".into());
            }
        }
        for _ in 0..counts[1] {
            let id = id32(get()?)?;
            let from = id32(get()?)?;
            let to = id32(get()?)?;
            let rate = rate(get()?, get()?)?.ok_or("disabled connection rate")?;
            if from == to
                || !w.reservoirs.contains_key(&from)
                || !w.reservoirs.contains_key(&to)
                || w.connections
                    .insert(id, Connection { from, to, rate })
                    .is_some()
            {
                return Err("invalid regional connection state".into());
            }
        }
        let mut owned = BTreeMap::<ReservoirId, u64>::new();
        for _ in 0..counts[2] {
            let token = OwnershipToken {
                region: get()?,
                reservoir: id32(get()?)?,
                serial: get()?,
                units: get()?,
            };
            if token.serial == 0
                || token.serial > w.serial
                || !w.reservoirs.contains_key(&token.reservoir)
            {
                return Err("invalid ownership serial/reservoir".into());
            }
            let sum = owned.entry(token.reservoir).or_default();
            *sum = sum
                .checked_add(token.units)
                .ok_or("regional ownership overflow")?;
            if w.owners.insert(token.region, token).is_some() {
                return Err("duplicate regional owner".into());
            }
        }
        if w.reservoirs
            .iter()
            .any(|(&id, r)| r.held != owned.get(&id).copied().unwrap_or(0))
            || !w.conserved()
        {
            return Err("regional conservation/ownership ledger mismatch".into());
        }
        Ok(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn graph() -> RegionalWater {
        let mut w = RegionalWater::new();
        w.add_reservoir(1, 1000, 100, 2000).unwrap();
        w.add_reservoir(2, 1000, 0, 1000).unwrap();
        w.add_reservoir(3, 1000, 0, 0).unwrap();
        w.set_source(1, Some(7)).unwrap();
        w.connect(1, 1, 2, 5).unwrap();
        w.connect(2, 2, 3, 3).unwrap();
        w.set_outlet(3, Some(2)).unwrap();
        w
    }
    #[test]
    fn graph_and_external_ledger_conserve_exactly() {
        let mut w = graph();
        for _ in 0..600 {
            w.advance(100_000_000).unwrap();
            assert!(w.conserved());
        }
        assert_eq!(w.ledger().supplied, 420);
        assert_eq!(w.ledger().drained, 120);
        assert_eq!(w.stored_units(), 400);
        assert_eq!(w.reservoir(1).unwrap().volume, 220);
    }
    #[test]
    fn duplicate_stale_and_failed_transfer_are_atomic() {
        let mut w = graph();
        w.transfer(10, 1, 2, 20).unwrap();
        let before = w.snapshot();
        for id in [9, 10] {
            assert!(w.transfer(id, 1, 2, 20).is_err());
            assert_eq!(before, w.snapshot());
        }
        assert!(w.transfer(11, 1, 2, 10000).is_err());
        assert_eq!(before, w.snapshot());
        w.transfer(11, 1, 2, 1).unwrap();
        assert!(w.conserved());
    }
    #[test]
    fn blocked_sources_suppress_without_mass_or_backlog() {
        let mut w = RegionalWater::new();
        w.add_reservoir(1, 1, 0, 0).unwrap();
        w.set_source(1, Some(7)).unwrap();
        for _ in 0..10 {
            w.advance(100_000_000).unwrap();
        }
        assert_eq!(w.ledger().supplied, 1);
        assert_eq!(w.ledger().suppressed, 6);
        w.add_reservoir(2, 10, 0, 0).unwrap();
        w.transfer(1, 1, 2, 1).unwrap();
        w.advance(1).unwrap();
        assert_eq!(w.reservoir(1).unwrap().volume, 0);
        assert!(w.conserved());
    }
    #[test]
    fn detailed_ownership_reserves_capacity_and_rejects_stale_tokens() {
        let mut w = graph();
        let token = w.take_region(42, 1, 40).unwrap();
        assert_eq!(w.detailed_units(), 40);
        assert!(w.conserved());
        let next = w.exchange_owned(1, token, 2, 10, false).unwrap();
        assert!(w.return_region(token, 40).is_err());
        assert!(w.exchange_owned(2, token, 1, 1, true).is_err());
        let next = w.exchange_owned(2, next, 1, 5, true).unwrap();
        assert_eq!(next.units, 35);
        let before = w.snapshot();
        assert!(w.return_region(next, 34).is_err());
        assert_eq!(before, w.snapshot());
        w.return_region(next, 35).unwrap();
        assert_eq!(w.detailed_units(), 0);
        assert!(w.return_region(next, 35).is_err());
        assert!(w.conserved());
        let mut small = RegionalWater::new();
        small.add_reservoir(1, 10, 10, 0).unwrap();
        small.set_source(1, Some(10)).unwrap();
        let t = small.take_region(1, 1, 10).unwrap();
        small.advance(SECOND).unwrap();
        assert_eq!(small.ledger().supplied, 0);
        small.return_region(t, 10).unwrap();
    }
    #[test]
    fn regional_restart_preserves_clocks_ownership_and_transfer_replay() {
        let mut a = graph();
        a.advance(37_000_001).unwrap();
        let token = a.take_region(42, 1, 20).unwrap();
        a.transfer(17, 1, 2, 3).unwrap();
        let mut b = RegionalWater::restore(&a.snapshot()).unwrap();
        assert_eq!(a, b);
        for _ in 0..200 {
            a.advance(33_333_333).unwrap();
            b.advance(33_333_333).unwrap();
            assert_eq!(a.snapshot(), b.snapshot());
        }
        assert!(b.transfer(17, 1, 2, 3).is_err());
        a.return_region(token, 20).unwrap();
        b.return_region(token, 20).unwrap();
        assert_eq!(a, b);
        let mut bytes = a.snapshot();
        bytes[40] ^= 1;
        assert!(RegionalWater::restore(&bytes).is_err());
        for n in 0..96 {
            assert!(RegionalWater::restore(&bytes[..n]).is_err());
        }
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[test]
    fn ordered_batch_is_atomic_replay_safe_and_conservative() {
        let mut w = RegionalWater::new();
        w.add_reservoir(1, 2, 2, 0).unwrap();
        w.add_reservoir(2, 2, 0, 0).unwrap();
        w.add_reservoir(3, 2, 0, 0).unwrap();
        let original = w.snapshot();
        assert!(w.transfer_batch(1, &[(1, 2, 2), (2, 3, 3)]).is_err());
        assert_eq!(w.snapshot(), original);
        w.transfer_batch(1, &[(1, 2, 2), (2, 3, 2)]).unwrap();
        assert_eq!(w.reservoir(3).unwrap().volume, 2);
        assert_eq!(w.stored_units(), 2);
        let committed = w.snapshot();
        assert!(w.transfer_batch(1, &[(3, 1, 1)]).is_err());
        assert_eq!(w.snapshot(), committed);
        assert!(w.conserved());
    }
    #[test]
    fn explicit_external_flow_counts_both_directions_and_rejects_replay() {
        let mut w = RegionalWater::new();
        w.add_reservoir(1, 4, 2, 0).unwrap();
        w.external_exchange(1, &[(1, 2, 2)], 7).unwrap();
        assert_eq!(w.reservoir(1).unwrap().volume, 2);
        assert_eq!(w.ledger().supplied, 2);
        assert_eq!(w.ledger().drained, 2);
        assert_eq!(w.ledger().suppressed, 7);
        let before = w.snapshot();
        assert!(w.external_exchange(1, &[(1, 1, 0)], 0).is_err());
        assert!(w.external_exchange(2, &[(1, 3, 0)], 0).is_err());
        assert_eq!(w.snapshot(), before);
        assert!(w.conserved());
    }
    #[test]
    fn capacity_changes_never_discard_owned_water_or_overflow_totals() {
        let mut w = RegionalWater::new();
        w.add_reservoir(1, 2, 2, 0).unwrap();
        w.add_reservoir(2, 2, 0, 0).unwrap();
        w.take_region(7, 1, 1).unwrap();
        let original = w.snapshot();
        assert!(w.set_capacity(1, 1).is_err());
        assert!(w.set_capacity(2, u64::MAX).is_err());
        assert_eq!(w.snapshot(), original);
        w.set_capacity(1, 4096).unwrap();
        assert_eq!(w.stored_units() + w.detailed_units(), 2);
        assert!(w.conserved());
    }
}
