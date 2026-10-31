//! Annual runoff routing and conservative permanent-lake screening.
//! Virtual flood/spill elevations are routing constraints, not guaranteed water.
//! A depression is a permanent spill-level lake only if its catchment can meet
//! open-water evaporation. Otherwise it is a closed/seasonal sink, not an invented
//! full lake. Partial nested-lake filling is deferred (this is not Fill–Spill–Merge).
use super::atlas_climate::Cell;

pub const SECONDS_PER_YEAR: f64 = 31_557_600.;
pub const NO_LAKE: u32 = u32::MAX;

#[derive(Debug)]
pub struct Lake {
    pub outlet: usize,
    pub spill_m: f64,
    pub permanent: bool,
    pub inflow_m3_year: f64,
    /// Incremental loss relative to the land AET already removed from runoff.
    pub extra_evaporation_m3_year: f64,
    pub outflow_m3_year: f64,
    pub storage_m3: f64,
}

pub struct Hydrology {
    pub water: Vec<f64>,
    pub discharge: Vec<f64>,
    /// Endorheic reservoir outlet or marine/domain outlet node ID.
    pub watershed: Vec<u32>,
    pub lake_id: Vec<u32>,
    pub lakes: Vec<Lake>,
    pub runoff_m3_year: f64,
    pub export_m3_year: f64,
    pub extra_evaporation_m3_year: f64,
}
impl Hydrology {
    pub fn residual(&self) -> f64 {
        self.runoff_m3_year - self.export_m3_year - self.extra_evaporation_m3_year
    }
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Equal-spill connected components include dry saddle nodes, so collapsing a
/// reservoir cannot create cycles via its own spillway. Its first flood-order
/// node has a receiver outside the component. All other outgoing branches are
/// consolidated at that one outlet for annual accounting.
pub fn solve(
    height: &[f64],
    spill: &[f64],
    ocean: &[bool],
    triangles: &[[u32; 3]],
    order: &[usize],
    receiver: &[usize],
    cell_area: &[f64],
    climate: &[Cell],
) -> Hydrology {
    let n = height.len();
    assert!([
        spill.len(),
        ocean.len(),
        order.len(),
        receiver.len(),
        cell_area.len(),
        climate.len()
    ]
    .iter()
    .all(|&v| v == n));
    let mut parent: Vec<_> = (0..n).collect();
    for t in triangles {
        for (a, b) in [
            (t[0] as usize, t[1] as usize),
            (t[1] as usize, t[2] as usize),
            (t[2] as usize, t[0] as usize),
        ] {
            if !ocean[a] && !ocean[b] && spill[a] == spill[b] {
                let a = root(&mut parent, a);
                let b = root(&mut parent, b);
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    for i in 0..n {
        parent[i] = root(&mut parent, i);
    }
    let mut flooded = vec![false; n];
    for i in 0..n {
        if !ocean[i] && spill[i] > height[i] + 1e-6 {
            flooded[parent[i]] = true;
        }
    }
    let mut lake_id = vec![NO_LAKE; n];
    let mut root_id = vec![NO_LAKE; n];
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut outlets = Vec::new();
    for &i in order {
        let p = parent[i];
        if !flooded[p] {
            continue;
        }
        if root_id[p] == NO_LAKE {
            root_id[p] = members.len() as u32;
            members.push(Vec::new());
            outlets.push(i);
        }
        let id = root_id[p];
        lake_id[i] = id;
        members[id as usize].push(i);
    }
    drop(parent);
    drop(root_id);
    drop(flooded);
    let mut water = height.to_vec();
    let mut discharge: Vec<_> = (0..n)
        .map(|i| {
            if ocean[i] {
                0.
            } else {
                climate[i].runoff as f64 * 0.001 * cell_area[i]
            }
        })
        .collect();
    let runoff_m3_year = discharge.iter().sum();
    let mut basin_inflow = vec![0.; members.len()];
    let mut lakes: Vec<Option<Lake>> = (0..members.len()).map(|_| None).collect();
    let mut export_m3_year = 0.;
    let mut extra_evaporation_m3_year = 0.;
    for &i in order.iter().rev() {
        if ocean[i] {
            water[i] = 0.;
        }
        let j = receiver[i];
        let id = lake_id[i];
        if id != NO_LAKE {
            let b = id as usize;
            basin_inflow[b] += discharge[i];
            discharge[i] = 0.; // a reservoir is not a web of surface river lines
            if i != outlets[b] {
                continue;
            }
            let demand: f64 = members[b]
                .iter()
                .filter(|&&k| spill[k] > height[k] + 1e-6)
                .map(|&k| {
                    let c = climate[k];
                    // Replace land runoff by direct P minus open-water PET.
                    (c.pet as f64 - c.rain as f64 + c.runoff as f64).max(0.) * 0.001 * cell_area[k]
                })
                .sum();
            let incoming = basin_inflow[b];
            let permanent = incoming > 0. && incoming >= demand;
            let loss = incoming.min(demand);
            let outgoing = if permanent {
                (incoming - loss).max(0.)
            } else {
                0.
            };
            let mut storage = 0.;
            if permanent {
                for &k in &members[b] {
                    water[k] = spill[k];
                    storage += (spill[k] - height[k]).max(0.) * cell_area[k];
                }
            }
            extra_evaporation_m3_year += if permanent { loss } else { incoming };
            discharge[i] = outgoing;
            if j == i {
                export_m3_year += outgoing;
            } else {
                assert_ne!(lake_id[j], id, "lake outlet must leave component");
                discharge[j] += outgoing;
            }
            lakes[b] = Some(Lake {
                outlet: i,
                spill_m: spill[i],
                permanent,
                inflow_m3_year: incoming,
                extra_evaporation_m3_year: if permanent { loss } else { incoming },
                outflow_m3_year: outgoing,
                storage_m3: storage,
            });
        } else if j == i {
            export_m3_year += discharge[i];
        } else {
            discharge[j] += discharge[i];
        }
    }
    let lakes: Vec<_> = lakes.into_iter().map(Option::unwrap).collect();
    let mut watershed = vec![0; n];
    for &i in order {
        let id = lake_id[i];
        watershed[i] = if id != NO_LAKE {
            let l = &lakes[id as usize];
            if !l.permanent {
                l.outlet as u32
            } else if receiver[l.outlet] == l.outlet {
                l.outlet as u32
            } else {
                watershed[receiver[l.outlet]]
            }
        } else if receiver[i] == i {
            i as u32
        } else {
            watershed[receiver[i]]
        };
    }
    Hydrology {
        water,
        discharge,
        watershed,
        lake_id,
        lakes,
        runoff_m3_year,
        export_m3_year,
        extra_evaporation_m3_year,
    }
}

/// Broad hydraulic-geometry estimate for diagnostics and future refinement.
/// Not yet an excavated native channel or a claim of meter-scale accuracy.
pub fn channel_width(discharge_m3_year: f64) -> f64 {
    4. * (discharge_m3_year / SECONDS_PER_YEAR).max(0.).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wet_basin_spills_dry_basin_does_not_invent_water() {
        // A bowl drains through node 1 to boundary 0. Nodes 1/2/3 share spill 10.
        let h = [0., 10., 1., 3., 20.];
        let s = [0., 10., 10., 10., 20.];
        let ocean = [true, false, false, false, false];
        let t = [[0, 1, 2], [1, 2, 3], [2, 3, 4]];
        let order = [0, 1, 2, 3, 4];
        let receiver = [0, 0, 1, 2, 3];
        let area = [100.; 5];
        for wet in [false, true] {
            let rain = if wet { 2000. } else { 1. };
            let pet = 700.;
            let c = vec![
                Cell {
                    rain,
                    pet,
                    runoff: super::super::atlas_climate::runoff(rain as f64, pet as f64) as f32,
                    ..Cell::default()
                };
                5
            ];
            let r = solve(&h, &s, &ocean, &t, &order, &receiver, &area, &c);
            assert_eq!(r.lakes.len(), 1);
            assert_eq!(r.lakes[0].permanent, wet);
            assert!(r.residual().abs() < 1e-8);
            assert_eq!(r.water[0], 0.);
            if wet {
                assert_eq!(r.water[2], 10.);
                assert!(r.discharge[0] > 0.);
                assert_eq!(r.watershed[4], 0);
            } else {
                assert_eq!(r.water[2], h[2]);
                assert_eq!(r.discharge[0], 0.);
                assert_eq!(r.watershed[4], 1);
            }
        }
    }
    #[test]
    fn wet_and_dry_catchments_of_equal_area_carry_different_discharge() {
        let c = [
            Cell::default(),
            Cell {
                runoff: 1000.,
                ..Cell::default()
            },
            Cell {
                runoff: 10.,
                ..Cell::default()
            },
        ];
        let r = solve(
            &[0., 10., 10.],
            &[0., 10., 10.],
            &[true, false, false],
            &[[0, 1, 2]],
            &[0, 1, 2],
            &[0, 0, 0],
            &[1e6; 3],
            &c,
        );
        assert_eq!(r.discharge[1], 100. * r.discharge[2]);
        assert!(r.residual().abs() < 1e-8);
    }
}
