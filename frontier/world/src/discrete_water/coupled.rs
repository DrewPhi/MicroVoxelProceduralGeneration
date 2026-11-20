//! Atomic finite-reservoir exchange for one bounded detailed region.
//!
//! This correctness baseline stages a clone per step. It is intended for small
//! (e.g. 16³) tiles, not a whole lake. Nothing is published until both the binary
//! occupancy and regional ownership ledger accept every boundary transfer.
use super::regional::{OwnershipToken, RegionalWater, ReservoirId};
use super::{BoundaryGhost, DiscreteWater, OpenBoundary, StepStats};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct CoupledWater {
    sim: DiscreteWater,
    regional: RegionalWater,
    token: OwnershipToken,
    ports: BTreeMap<u64, ReservoirId>,
    limits: BTreeMap<u64, [u64; 2]>,
    next_transfer: u64,
    port_cursor: u64,
}
impl CoupledWater {
    pub fn new(
        sim: DiscreteWater,
        regional: RegionalWater,
        token: OwnershipToken,
        next_transfer: u64,
    ) -> Result<Self, String> {
        if sim.has_internal_endpoints() || sim.boundaries().next().is_some() {
            return Err("coupled region must start without unaccounted endpoints".into());
        }
        if regional.ownership(token.region) != Some(token)
            || token.units != sim.mass() as u64
            || !regional.conserved()
            || next_transfer <= regional.last_transfer_id()
        {
            return Err("detailed occupancy and regional ownership disagree".into());
        }
        Ok(Self {
            sim,
            regional,
            token,
            ports: BTreeMap::new(),
            limits: BTreeMap::new(),
            next_transfer,
            port_cursor: 0,
        })
    }
    /// Dry terrain edits preserve water ownership; wet placement is rejected.
    pub fn set_solid(&mut self, cell: crate::Cell, solid: bool) -> Result<(), String> {
        if solid && self.sim.is_boundary_cell(cell) {
            return Err(
                "remove or reconfigure the hydraulic boundary before closing its face".into(),
            );
        }
        self.sim.set_solid(cell, solid)
    }
    pub fn simulation(&self) -> &DiscreteWater {
        &self.sim
    }
    pub fn regional(&self) -> &RegionalWater {
        &self.regional
    }
    pub fn ownership(&self) -> OwnershipToken {
        self.token
    }
    /// Commit conservative routing between dormant reservoirs. Detailed
    /// occupancy/token stay unchanged; the transfer serial cannot rewind.
    pub fn adopt_regional_after_transfers(
        &mut self,
        regional: RegionalWater,
    ) -> Result<(), String> {
        if !regional.conserved()
            || regional.ownership(self.token.region) != Some(self.token)
            || regional.ledger() != self.regional.ledger()
            || regional.stored_units() != self.regional.stored_units()
            || regional.detailed_units() != self.regional.detailed_units()
            || regional.last_transfer_id() < self.regional.last_transfer_id()
            || self
                .ports
                .values()
                .any(|&id| regional.reservoir(id).is_none())
        {
            return Err("dormant routing changed detailed ownership or total mass".into());
        }
        let next = regional
            .last_transfer_id()
            .checked_add(1)
            .ok_or("water transfer serial exhausted")?;
        self.regional = regional;
        self.next_transfer = self.next_transfer.max(next);
        Ok(())
    }
    /// Commit explicit environmental inflow/outflow outside the detailed region.
    /// Exact expected counter deltas prevent treating a reload as fresh water.
    pub fn adopt_regional_after_external_flow(
        &mut self,
        regional: RegionalWater,
        supplied: u64,
        drained: u64,
        suppressed: u64,
    ) -> Result<(), String> {
        let mut expected = self.regional.ledger();
        expected.supplied = expected
            .supplied
            .checked_add(supplied)
            .ok_or("supply overflow")?;
        expected.drained = expected
            .drained
            .checked_add(drained)
            .ok_or("drain overflow")?;
        expected.suppressed = expected
            .suppressed
            .checked_add(suppressed)
            .ok_or("suppression overflow")?;
        if !regional.conserved()
            || regional.ownership(self.token.region) != Some(self.token)
            || regional.ledger() != expected
            || regional.detailed_units() != self.regional.detailed_units()
            || regional.stored_units() as u128 + drained as u128
                != self.regional.stored_units() as u128 + supplied as u128
            || regional.last_transfer_id() < self.regional.last_transfer_id()
            || self
                .ports
                .values()
                .any(|&id| regional.reservoir(id).is_none())
        {
            return Err("environment exchange disagrees with exact declared deltas".into());
        }
        let next = regional
            .last_transfer_id()
            .checked_add(1)
            .ok_or("water transfer serial exhausted")?;
        self.regional = regional;
        self.next_transfer = self.next_transfer.max(next);
        Ok(())
    }
    /// Configure geometry/head. Supply/capacity quotas are derived from the
    /// finite reservoirs by step(), never trusted from this input.
    pub fn set_port(
        &mut self,
        id: u64,
        reservoir: ReservoirId,
        mut port: OpenBoundary,
    ) -> Result<(), String> {
        if !self.ports.contains_key(&id) && self.ports.len() >= 65536 {
            return Err("coupled port budget exceeded".into());
        }
        if reservoir == self.token.reservoir || self.regional.reservoir(reservoir).is_none() {
            return Err("port needs a distinct existing exterior reservoir".into());
        }
        port.import_quota = 0;
        port.export_quota = 0;
        self.sim.set_boundary(id, port)?;
        self.ports.insert(id, reservoir);
        Ok(())
    }
    /// Caps reserve individual exterior native cells without making them
    /// inexhaustible aggregate sources. Limits persist until explicitly changed.
    pub fn set_port_limits(&mut self, id: u64, imports: u64, exports: u64) -> Result<(), String> {
        if !self.ports.contains_key(&id) {
            return Err("unknown coupled port".into());
        }
        self.limits.insert(id, [imports, exports]);
        Ok(())
    }
    pub fn set_port_ghost(&mut self, id: u64, ghost: Option<BoundaryGhost>) -> Result<(), String> {
        if !self.ports.contains_key(&id) {
            return Err("unknown coupled port".into());
        }
        self.sim.set_boundary_ghost(id, ghost)
    }
    /// Rebind a canonical lower-cell positive face to this exterior port.
    pub fn set_port_face(&mut self, id: u64, face: super::NativeFaceState) -> Result<(), String> {
        let condition = self.sim.boundary(id).ok_or("unknown coupled port")?;
        let ghost = self.sim.boundary_ghost(id);
        let mut state = super::NativeBoundaryState::from_positive_face(id, condition, face);
        state.ghost = ghost;
        self.sim.set_boundary_state(state)
    }
    pub fn remove_port(&mut self, id: u64) {
        self.limits.remove(&id);
        self.ports.remove(&id);
        self.sim.remove_boundary(id);
    }
    pub fn step(&mut self, dt: f64) -> Result<StepStats, String> {
        // No externally visible half-commit, even on invalid/stale transfer IDs.
        let mut staged = self.clone();
        let owner = staged.regional.reservoir(staged.token.reservoir).unwrap();
        let mut owner_room = owner.capacity - owner.volume - owner.held;
        let mut exportable = staged.token.units;
        let mut reserved_in = BTreeMap::<ReservoirId, u64>::new();
        let mut reserved_out = BTreeMap::<ReservoirId, u64>::new();
        let mut ids: Vec<_> = staged.ports.keys().copied().collect();
        if !ids.is_empty() {
            let offset = staged.port_cursor as usize % ids.len();
            ids.rotate_left(offset);
        }
        let share = |total: u64| {
            total
                .div_ceil(ids.len().max(1) as u64)
                .max(1)
                .min(super::MAX_SUBSTEPS as u64)
        };
        let import_share = share(owner_room);
        let export_share = share(exportable);
        for id in ids {
            let reservoir = staged.ports[&id];
            let limits = staged.limits.get(&id).copied().unwrap_or([u64::MAX; 2]);
            let r = staged.regional.reservoir(reservoir).unwrap();
            let used_in = reserved_in.entry(reservoir).or_default();
            let used_out = reserved_out.entry(reservoir).or_default();
            let mut port = staged.sim.boundary(id).ok_or("missing coupled boundary")?;
            // At most one transfer per boundary per substep. This bound avoids
            // reserving an entire lake for the first port in iteration order.
            port.import_quota = (r.volume - *used_in)
                .min(owner_room)
                .min(import_share)
                .min(limits[0]);
            port.export_quota = (r.capacity - r.volume - r.held - *used_out)
                .min(exportable)
                .min(export_share)
                .min(limits[1]);
            let exterior_y = port.cell.y as f64
                + if port.axis == 1 {
                    if port.positive {
                        1.
                    } else {
                        -1.
                    }
                } else {
                    0.
                };
            // A dry exterior cannot supply a unit. Avoid reserving finite supply
            // for such ports and starving wet neighbors behind an empty quota.
            if let Some(ghost) = staged.sim.boundary_ghost(id) {
                if ghost.wet {
                    port.export_quota = 0;
                } else {
                    port.import_quota = 0;
                }
            } else if port.head_m <= (exterior_y + 0.5) * crate::voxel_size() {
                port.import_quota = 0;
            }
            // Only currently wet cells reserve outgoing whole units. A unit
            // arriving at a dry boundary later in this call may leave next call;
            // dry faces cannot monopolize every scarce export reservation.
            if !staged.sim.water_at(port.cell) {
                port.export_quota = 0;
            }
            *used_in += port.import_quota;
            *used_out += port.export_quota;
            owner_room -= port.import_quota;
            exportable -= port.export_quota;
            staged.sim.set_boundary(id, port)?;
        }
        let stats = staged.sim.step(dt);
        for transfer in &stats.boundary_transfers {
            let reservoir = *staged
                .ports
                .get(&transfer.boundary_id)
                .ok_or("unowned boundary transfer")?;
            let next = staged
                .next_transfer
                .checked_add(1)
                .ok_or("water transfer serial exhausted")?;
            staged.token = staged.regional.exchange_owned(
                staged.next_transfer,
                staged.token,
                reservoir,
                transfer.units,
                transfer.into_detail,
            )?;
            staged.next_transfer = next;
        }
        if staged.token.units != staged.sim.mass() as u64 || !staged.regional.conserved() {
            return Err("coupled water conservation failed".into());
        }
        staged.port_cursor = staged.port_cursor.wrapping_add(1);
        *self = staged;
        Ok(stats)
    }
}

impl CoupledWater {
    pub fn snapshot(&self) -> Vec<u8> {
        let sim = self.sim.snapshot();
        let region = self.regional.snapshot();
        let mut bytes = b"DWCOUP03".to_vec();
        for value in [
            self.token.region,
            self.next_transfer,
            sim.len() as u64,
            region.len() as u64,
            self.ports.len() as u64,
        ] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(self.port_cursor.to_le_bytes());
        bytes.extend(sim);
        bytes.extend(region);
        for (&id, &reservoir) in &self.ports {
            bytes.extend(id.to_le_bytes());
            bytes.extend(reservoir.to_le_bytes());
            for cap in self.limits.get(&id).copied().unwrap_or([u64::MAX; 2]) {
                bytes.extend(cap.to_le_bytes());
            }
        }
        bytes.extend(checksum(&bytes).to_le_bytes());
        bytes
    }
    pub fn restore(bytes: &[u8]) -> Result<Self, String> {
        let bad = || "invalid coupled water checkpoint".to_string();
        if bytes.len() < 64 || (&bytes[..8] != b"DWCOUP02" && &bytes[..8] != b"DWCOUP03") {
            return Err(bad());
        }
        let end = bytes.len() - 8;
        if checksum(&bytes[..end]) != u64::from_le_bytes(bytes[end..].try_into().unwrap()) {
            return Err(bad());
        }
        let word = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let region_id = word(8);
        let next_transfer = word(16);
        let sim_len = usize::try_from(word(24)).map_err(|_| bad())?;
        let region_len = usize::try_from(word(32)).map_err(|_| bad())?;
        let ports_len = usize::try_from(word(40)).map_err(|_| bad())?;
        let port_cursor = word(48);
        let region_start = 56usize.checked_add(sim_len).ok_or_else(bad)?;
        let ports_start = region_start.checked_add(region_len).ok_or_else(bad)?;
        let port_stride = if &bytes[..8] == b"DWCOUP03" { 28 } else { 12 };
        if ports_len > 65536 || ports_start.checked_add(ports_len * port_stride) != Some(end) {
            return Err(bad());
        }
        let sim = DiscreteWater::restore(&bytes[56..region_start])?;
        let regional = RegionalWater::restore(&bytes[region_start..ports_start])?;
        let token = regional.ownership(region_id).ok_or_else(bad)?;
        let mut ports = BTreeMap::new();
        let mut limits = BTreeMap::new();
        for i in 0..ports_len {
            let offset = ports_start + i * port_stride;
            let id = word(offset);
            let reservoir = u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into().unwrap());
            if port_stride == 28 {
                limits.insert(id, [word(offset + 12), word(offset + 20)]);
            }
            if ports.insert(id, reservoir).is_some()
                || sim.boundary(id).is_none()
                || reservoir == token.reservoir
                || regional.reservoir(reservoir).is_none()
            {
                return Err(bad());
            }
        }
        if next_transfer <= regional.last_transfer_id()
            || sim.has_internal_endpoints()
            || sim.boundaries().count() != ports.len()
            || token.units != sim.mass() as u64
            || !regional.conserved()
        {
            return Err(bad());
        }
        Ok(Self {
            sim,
            regional,
            token,
            ports,
            limits,
            next_transfer,
            port_cursor,
        })
    }
}
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, &v| {
        (h ^ v as u64).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cell;
    fn fixture() -> CoupledWater {
        let origin = Cell { x: 0, y: 0, z: 0 };
        let sim = DiscreteWater::new(origin, [2, 1, 1]).unwrap();
        let mut regional = RegionalWater::new();
        regional.add_reservoir(1, 2, 0, 0).unwrap();
        regional.add_reservoir(2, 16, 8, 1000).unwrap();
        let token = regional.take_region(1, 1, 0).unwrap();
        let mut water = CoupledWater::new(sim, regional, token, 1).unwrap();
        water
            .set_port(
                1,
                2,
                OpenBoundary {
                    cell: origin,
                    axis: 0,
                    positive: false,
                    head_m: 1.,
                    velocity_mps: [1., 0., 0.],
                    import_quota: 999,
                    export_quota: 999,
                },
            )
            .unwrap();
        water
    }
    #[test]
    fn import_is_finite_conserved_and_restart_exact() {
        let mut a = fixture();
        for _ in 0..20 {
            a.step(0.025).unwrap();
        }
        assert!(a.sim.mass() > 0, "fixture must actually import water");
        assert_eq!(a.regional.stored_units() + a.sim.mass() as u64, 8);
        assert_eq!(a.token.units, a.sim.mass() as u64);
        let mut b = CoupledWater::restore(&a.snapshot()).unwrap();
        for _ in 0..20 {
            a.step(0.025).unwrap();
            b.step(0.025).unwrap();
            assert_eq!(a.snapshot(), b.snapshot());
        }
        let mut corrupt = a.snapshot();
        corrupt[24] ^= 1;
        assert!(CoupledWater::restore(&corrupt).is_err());
    }
    #[test]
    fn export_returns_owned_units_to_finite_storage() {
        let mut a = fixture();
        for _ in 0..20 {
            a.step(0.025).unwrap();
        }
        let filled = a.sim.mass();
        assert!(filled > 0);
        a.remove_port(1);
        for x in 0..2 {
            a.set_port(
                10 + x as u64,
                2,
                OpenBoundary {
                    cell: Cell { x, y: 0, z: 0 },
                    axis: 1,
                    positive: false,
                    head_m: -1.,
                    velocity_mps: [0.; 3],
                    import_quota: 0,
                    export_quota: 0,
                },
            )
            .unwrap();
        }
        for _ in 0..80 {
            a.step(0.025).unwrap();
        }
        assert_eq!(a.sim.mass(), 0);
        assert_eq!(a.token.units, 0);
        assert_eq!(a.regional.stored_units(), 8);
        assert!(a.regional.conserved());
    }
    #[test]
    fn explicit_port_caps_and_ghost_occupancy_survive_restart() {
        let mut water = fixture();
        water.set_port_limits(1, 1, 0).unwrap();
        water
            .set_port_ghost(
                1,
                Some(BoundaryGhost {
                    wet: true,
                    pressure: 100.,
                    velocity_mps: [2., 0., 0.],
                }),
            )
            .unwrap();
        let mut water = CoupledWater::restore(&water.snapshot()).unwrap();
        let mut imported = 0;
        for _ in 0..40 {
            let stats = water.step(0.025).unwrap();
            assert!(stats.boundary_transfers.len() <= 1);
            imported += stats
                .boundary_transfers
                .iter()
                .filter(|t| t.into_detail)
                .map(|t| t.units)
                .sum::<u64>();
        }
        assert_eq!(
            imported, 1,
            "one exact ghost cell cannot supply a second water unit"
        );
        assert_eq!(water.sim.mass(), 1);
        assert_eq!(water.regional.stored_units(), 7);
    }
    #[test]
    fn failed_transfer_rolls_back_both_models() {
        let mut a = fixture();
        a.next_transfer = u64::MAX;
        let mut failed = false;
        for _ in 0..40 {
            let before = a.snapshot();
            if a.step(0.025).is_err() {
                assert_eq!(before, a.snapshot());
                failed = true;
                break;
            }
        }
        assert!(failed, "fixture must reach a boundary transfer");
    }
    #[test]
    fn dry_first_port_cannot_starve_wet_port_when_substeps_share_port_count() {
        let origin = Cell { x: 0, y: 0, z: 0 };
        let sim = DiscreteWater::new(origin, [2, 1, 1]).unwrap();
        let mut regional = RegionalWater::new();
        regional.add_reservoir(1, 1, 0, 0).unwrap();
        regional.add_reservoir(2, 1, 1, 1000).unwrap();
        let token = regional.take_region(1, 1, 0).unwrap();
        let mut coupled = CoupledWater::new(sim, regional, token, 1).unwrap();
        for (id, x, head_m) in [(1, 0, -1.), (2, 1, 1.)] {
            coupled
                .set_port(
                    id,
                    2,
                    OpenBoundary {
                        cell: Cell { x, y: 0, z: 0 },
                        axis: 1,
                        positive: false,
                        head_m,
                        velocity_mps: [0.; 3],
                        import_quota: 0,
                        export_quota: 0,
                    },
                )
                .unwrap();
        }
        let mut imported = false;
        for _ in 0..10 {
            let stats = coupled.step(0.1).unwrap();
            imported |= stats
                .boundary_transfers
                .iter()
                .any(|t| t.boundary_id == 2 && t.into_detail);
            assert_eq!(
                coupled.regional().stored_units() + coupled.simulation().mass() as u64,
                1
            );
            assert!(coupled.regional().conserved());
        }
        assert!(
            imported,
            "one dry face permanently reserved the only supply unit"
        );
    }
    #[test]
    fn scarce_quota_keeps_fractional_credit_across_small_step_port_rotation() {
        let run = |port_count| {
            let origin = Cell { x: 0, y: 0, z: 0 };
            let sim = DiscreteWater::new(origin, [2, 1, 1]).unwrap();
            let mut regional = RegionalWater::new();
            regional.add_reservoir(1, 1, 0, 0).unwrap();
            regional.add_reservoir(2, 1, 1, 1000).unwrap();
            let token = regional.take_region(1, 1, 0).unwrap();
            let mut coupled = CoupledWater::new(sim, regional, token, 1).unwrap();
            for x in 0..port_count {
                coupled
                    .set_port(
                        1 + x as u64,
                        2,
                        OpenBoundary {
                            cell: Cell { x, y: 0, z: 0 },
                            axis: 1,
                            positive: false,
                            head_m: 1.,
                            velocity_mps: [0.; 3],
                            import_quota: 0,
                            export_quota: 0,
                        },
                    )
                    .unwrap();
            }
            let mut imported = 0;
            for _ in 0..1000 {
                let stats = coupled.step(0.001).unwrap();
                imported += stats
                    .boundary_transfers
                    .iter()
                    .filter(|t| t.into_detail)
                    .map(|t| t.units)
                    .sum::<u64>();
                assert!(imported <= 1, "finite one-cell supply exceeded");
                assert_eq!(
                    coupled.regional().stored_units() + coupled.simulation().mass() as u64,
                    1
                );
                assert!(coupled.regional().conserved());
            }
            assert_eq!(
                imported, 1,
                "rotating accounting quota erased fractional displacement"
            );
            assert_eq!(coupled.simulation().mass(), 1);
            assert_eq!(coupled.regional().stored_units(), 0);
            imported
        };
        assert_eq!(run(1), run(2));
    }
}
