//! Isolated landscape-evolution experiment. Units: meters, years, square meters.
//! Uplift and drainage shape the same surface; no post-hoc ridge noise/carving.
//! References and scope: agent_context/TERRAIN_GENERATION_REPLACEMENT_PLAN.md.
use super::{
    atlas_climate, atlas_hydrology,
    noise::{mix, noise, smoothstep},
};
use std::{cmp::Ordering, collections::BinaryHeap};

#[derive(Clone, Copy)]
pub struct Config {
    pub seed: u64,
    pub side: usize,
    pub extent: f64,
    pub steps: usize,
    pub dt: f64,
    pub irregular: bool,
    pub atlas: bool,
    pub climate: bool,
    pub climate_settings: atlas_climate::Settings,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            seed: 42,
            side: 513,
            extent: 100_000.,
            steps: 320,
            dt: 10_000.,
            irregular: true,
            atlas: false,
            climate: false,
            climate_settings: Default::default(),
        }
    }
}
#[derive(Clone, Copy)]
struct Edge {
    node: usize,
    length: f64,
}
#[derive(Clone, Copy)]
struct Entry(f64, usize);
impl PartialEq for Entry {
    fn eq(&self, b: &Self) -> bool {
        self.0 == b.0 && self.1 == b.1
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}
impl Ord for Entry {
    fn cmp(&self, b: &Self) -> Ordering {
        b.0.total_cmp(&self.0).then_with(|| b.1.cmp(&self.1))
    }
}

pub struct Landscape {
    pub config: Config,
    pub positions: Vec<[f64; 2]>,
    pub triangles: Vec<[u32; 3]>,
    pub height: Vec<f64>,
    pub initial: Vec<f64>,
    pub uplift: Vec<f64>,
    pub sediment: Vec<f64>,
    pub area: Vec<f64>,
    pub receiver: Vec<usize>,
    pub exported_m3: f64,
    pub uplifted_m3: f64,
    pub initial_volume_m3: f64,
    pub eroded_m3: f64,
    pub deposited_m3: f64,
    pub climate: Vec<atlas_climate::Cell>,
    pub hydrology: Option<atlas_hydrology::Hydrology>,
    coast_distance: Vec<f32>,
    runoff_flow: Vec<f64>,
    edges: Vec<Vec<Edge>>,
    cell_area: Vec<f64>,
    erodibility: Vec<f64>,
    old_fraction: Vec<f64>,
    ocean: Vec<bool>,
    order: Vec<usize>,
    filled: Vec<f64>,
    submerged: Vec<bool>,
    load: Vec<f64>,
    delta: Vec<f64>,
    boundary: Vec<bool>,
    diffusion_rate: f64,
}

impl Landscape {
    pub fn new(config: Config) -> Self {
        assert!(config.side >= 3 && config.extent > 0. && config.dt > 0.);
        assert!(!config.climate || (config.atlas && config.climate_settings.valid()));
        let n = config.side;
        let count = n * n;
        let spacing = config.extent / (n - 1) as f64;
        let mut positions = Vec::with_capacity(count);
        let mut boundary = Vec::with_capacity(count);
        for z in 0..n {
            for x in 0..n {
                let border = x == 0 || z == 0 || x == n - 1 || z == n - 1;
                let jitter = |salt| {
                    if border || !config.irregular {
                        0.
                    } else {
                        ((mix(config.seed ^ (z * n + x) as u64 ^ salt) >> 11) as f64
                            / (1u64 << 53) as f64
                            - 0.5)
                            * 0.8
                    }
                };
                positions.push([
                    (x as f64 + jitter(17)) * spacing,
                    (z as f64 + jitter(31)) * spacing,
                ]);
                boundary.push(border);
            }
        }
        // Start with a valid triangulation, then remove grid-imposed connectivity
        // with local Delaunay flips. Wider jitter spreads routing directions.
        let mut triangles = Vec::with_capacity(2 * (n - 1) * (n - 1));
        let distance = |a: usize, b: usize| {
            (positions[a][0] - positions[b][0]).hypot(positions[a][1] - positions[b][1])
        };
        for z in 0..n - 1 {
            for x in 0..n - 1 {
                let a = z * n + x;
                let b = a + 1;
                let c = a + n;
                let d = c + 1;
                let positive = |a: usize, b: usize, c: usize| {
                    super::landscape_mesh::orient(positions[a], positions[b], positions[c]) > 0.
                };
                let ad_valid = positive(a, b, d) && positive(a, d, c);
                let bc_valid = positive(a, b, c) && positive(b, d, c);
                assert!(ad_valid || bc_valid, "invalid initial quadrilateral");
                if ad_valid && (!bc_valid || distance(a, d) < distance(b, c)) {
                    triangles.extend([
                        [a as u32, b as u32, d as u32],
                        [a as u32, d as u32, c as u32],
                    ]);
                } else {
                    triangles.extend([
                        [a as u32, b as u32, c as u32],
                        [b as u32, d as u32, c as u32],
                    ]);
                }
            }
        }
        if config.irregular {
            super::landscape_mesh::legalize(&positions, &mut triangles);
        }
        let mut edges: Vec<Vec<Edge>> = vec![Vec::new(); count];
        let mut cell_area = vec![0.; count];
        for t in &triangles {
            let [a, b, c] = t.map(|i| i as usize);
            let [p, q, r] = [positions[a], positions[b], positions[c]];
            let area = ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])).abs() * 0.5;
            for i in [a, b, c] {
                cell_area[i] += area / 3.;
            }
            for (i, j) in [(a, b), (b, c), (c, a)] {
                for (u, v) in [(i, j), (j, i)] {
                    if !edges[u].iter().any(|e| e.node == v) {
                        edges[u].push(Edge {
                            node: v,
                            length: distance(u, v),
                        });
                    }
                }
            }
        }
        // Raster comparison has all eight routing directions, same physical area.
        if !config.irregular {
            for z in 0..n {
                for x in 0..n {
                    let i = z * n + x;
                    edges[i].clear();
                    for dz in -1isize..=1 {
                        for dx in -1isize..=1 {
                            let (xx, zz) = (x as isize + dx, z as isize + dz);
                            if (dx != 0 || dz != 0)
                                && xx >= 0
                                && zz >= 0
                                && xx < n as isize
                                && zz < n as isize
                            {
                                let j = zz as usize * n + xx as usize;
                                edges[i].push(Edge {
                                    node: j,
                                    length: distance(i, j),
                                });
                            }
                        }
                    }
                }
            }
        }
        let mut height = Vec::with_capacity(count);
        let mut uplift = Vec::with_capacity(count);
        let mut erodibility = Vec::with_capacity(count);
        let atlas = config.atlas.then(|| {
            super::geographic_atlas::Atlas::new(config.seed, [0., 0., config.extent, config.extent])
        });
        let mut old_fraction = Vec::with_capacity(count);
        let mut ocean = Vec::with_capacity(count);
        for &[x, z] in &positions {
            if let Some(atlas) = &atlas {
                let g = atlas.sample(x, z);
                height.push(g.basement_m);
                uplift.push(g.uplift_m_per_year);
                erodibility.push(g.erodibility);
                old_fraction.push(g.old_fraction);
                ocean.push(g.basement_m < 0.);
                continue;
            }
            old_fraction.push(1. - smoothstep(35_000., 48_000., x));
            ocean.push(false);
            // A broad low-relief starting surface; small perturbations only seed flow.
            let base = 180.
                + 0.0005 * z
                + 80. * noise(x, z, 18_000., config.seed ^ 811)
                + 65. * noise(x, z, 4200., config.seed ^ 812)
                + 15. * noise(x, z, 1200., config.seed ^ 813);
            height.push(base);
            // Finite curved young belt and an older broad upland, not tiled plates.
            let bend = 60_000. + 11_000. * ((z - 50_000.) / 24_000.).sin();
            let width = 11_000. + 4500. * noise(x, z, 24_000., config.seed ^ 814);
            let along = (-((z - 54_000.) / 31_000.).powi(4)).exp();
            let young = (-((x - bend) / width).powi(2)).exp()
                * along
                * (0.85 + 0.5 * noise(x, z, 16_000., config.seed ^ 816));
            let old =
                (-((x - 24_000.) / 16_000.).powi(2) - ((z - 68_000.) / 23_000.).powi(2)).exp();
            let rim =
                (x.min(z).min(config.extent - x).min(config.extent - z) / 9000.).clamp(0., 1.);
            uplift.push(
                (0.00125 * young
                    + 0.0005 * old * (0.85 + 0.5 * noise(x, z, 9000., config.seed ^ 817)))
                    * smoothstep(0., 1., rim),
            );
            erodibility.push(4.5e-6 * (1. + 0.45 * noise(x + 791., z, 16_000., config.seed ^ 815)));
        }
        // Only below-sea terrain connected to the domain edge is ocean. Enclosed
        // depressions remain lakes, including any basins below sea level.
        if config.atlas {
            ocean.fill(false);
            let mut stack: Vec<_> = (0..count)
                .filter(|&i| boundary[i] && height[i] < 0.)
                .collect();
            for &i in &stack {
                ocean[i] = true;
            }
            while let Some(i) = stack.pop() {
                for e in &edges[i] {
                    if !ocean[e.node] && height[e.node] < 0. {
                        ocean[e.node] = true;
                        stack.push(e.node);
                    }
                }
            }
        }
        // Fixed sea-level sinks. Marine deposition/coast migration are deferred.
        for i in 0..count {
            boundary[i] |= ocean[i];
        }
        let diffusion_rate = (0..count)
            .map(|i| {
                edges[i]
                    .iter()
                    .map(|e| {
                        cell_area[i].min(cell_area[e.node]) / (e.length * e.length * cell_area[i])
                    })
                    .sum::<f64>()
            })
            .fold(0f64, f64::max);
        let coast_distance = if config.climate {
            atlas_climate::coast_distance(&ocean, n, spacing)
        } else {
            Vec::new()
        };
        let initial = height.clone();
        let initial_volume_m3 = height.iter().zip(&cell_area).map(|(h, a)| h * a).sum();
        Self {
            config,
            positions,
            triangles,
            height,
            initial,
            uplift,
            sediment: vec![0.; count],
            area: cell_area.clone(),
            receiver: (0..count).collect(),
            exported_m3: 0.,
            uplifted_m3: 0.,
            initial_volume_m3,
            eroded_m3: 0.,
            deposited_m3: 0.,
            climate: Vec::new(),
            hydrology: None,
            coast_distance,
            runoff_flow: if config.climate {
                vec![0.; count]
            } else {
                Vec::new()
            },
            edges,
            cell_area,
            erodibility,
            old_fraction,
            ocean,
            order: Vec::with_capacity(count),
            filled: vec![0.; count],
            submerged: vec![false; count],
            load: vec![0.; count],
            delta: vec![0.; count],
            boundary,
            diffusion_rate,
        }
    }

    /// Fixed marine sinks connected to the atlas boundary at initialization.
    pub fn ocean_mask(&self) -> &[bool] {
        &self.ocean
    }

    /// Exact spill elevations preserve flat lake surfaces; flood-tree order resolves
    /// equal-height routing without imposing an artificial slope on the water.
    /// Before final climate compilation these are virtual spill surfaces. After
    /// compilation they contain only supply-supported permanent lakes and ocean.
    pub fn water_surface(&self) -> &[f64] {
        self.hydrology.as_ref().map_or(&self.filled, |h| &h.water)
    }

    /// Priority flood establishes acyclic spill routes. It never fills the real bed.
    pub fn route(&mut self) {
        self.hydrology = None;
        let mut heap = BinaryHeap::new();
        self.filled.fill(f64::INFINITY);
        self.order.clear();
        for i in 0..self.height.len() {
            if self.boundary[i] {
                self.filled[i] = if self.ocean[i] { 0. } else { self.height[i] };
                self.receiver[i] = i;
                heap.push(Entry(self.filled[i], i));
            }
        }
        while let Some(Entry(h, i)) = heap.pop() {
            self.order.push(i);
            for e in &self.edges[i] {
                let j = e.node;
                if self.filled[j].is_finite() {
                    continue;
                }
                self.filled[j] = self.height[j].max(h);
                self.receiver[j] = i;
                heap.push(Entry(self.filled[j], j));
            }
        }
        // Choose physical steepest descent wherever available; use spill tree in pits.
        for &i in &self.order {
            if self.boundary[i] {
                continue;
            }
            let mut slope = 0.;
            for e in &self.edges[i] {
                let s = (self.filled[i] - self.filled[e.node]) / e.length;
                if s > slope {
                    slope = s;
                    self.receiver[i] = e.node;
                }
            }
        }
        for i in 0..self.height.len() {
            self.submerged[i] = self.filled[i] - self.height[i] > 1e-6;
        }
        self.area.clone_from(&self.cell_area);
        if !self.climate.is_empty() {
            for i in 0..self.height.len() {
                self.runoff_flow[i] = if self.ocean[i] {
                    0.
                } else {
                    self.climate[i].runoff as f64 * 0.001 * self.cell_area[i]
                };
            }
        }
        for &i in self.order.iter().rev() {
            if self.receiver[i] != i {
                self.area[self.receiver[i]] += self.area[i];
                if !self.climate.is_empty() {
                    self.runoff_flow[self.receiver[i]] += self.runoff_flow[i];
                }
            }
        }
    }

    pub fn evolve(&mut self) {
        // Four bounded climate/erosion coupling stages, then a final climate.
        // The old regional fixture remains unchanged for comparisons.
        for step in 0..self.config.steps {
            if self.config.climate && step % ((self.config.steps + 3) / 4).max(1) == 0 {
                self.refresh_climate();
            }
            self.step(step);
            if step % 40 == 0 {
                eprintln!("landscape epoch {}/{}", step + 1, self.config.steps);
            }
        }
        if self.config.climate {
            self.refresh_climate();
        }
        self.route();
        if self.config.climate {
            self.hydrology = Some(atlas_hydrology::solve(
                &self.height,
                &self.filled,
                &self.ocean,
                &self.triangles,
                &self.order,
                &self.receiver,
                &self.cell_area,
                &self.climate,
            ));
        }
    }

    fn refresh_climate(&mut self) {
        // Release the previous field before allocating the next compiled stage.
        self.climate = Vec::new();
        self.climate = atlas_climate::compute(
            &self.height,
            &self.positions,
            &self.ocean,
            &self.coast_distance,
            self.config.side,
            self.config.extent / (self.config.side - 1) as f64,
            self.config.climate_settings,
            self.config.seed,
        );
    }

    fn step(&mut self, step: usize) {
        let dt = self.config.dt;
        // Old uplands stop uplifting early; the young belt remains active.
        let age = step as f64 / self.config.steps.max(1) as f64;
        for i in 0..self.height.len() {
            if self.boundary[i] {
                continue;
            }
            let old_fraction = self.old_fraction[i];
            let rate = self.uplift[i] * (1. - old_fraction * smoothstep(0.25, 0.6, age) * 0.9);
            let rise = rate * dt;
            self.height[i] += rise;
            self.uplifted_m3 += rise * self.cell_area[i];
        }
        self.route();
        self.load.fill(0.);
        // Downstream-first implicit n=1 stream-power solve, with drainage held fixed
        // during each step. Virtual spill paths cannot erode uphill through a ridge.
        for &i in &self.order {
            let j = self.receiver[i];
            let receiver_height = if self.submerged[j] {
                self.filled[j]
            } else {
                self.height[j]
            };
            if j == i || self.submerged[i] || self.height[i] <= receiver_height {
                continue;
            }
            let distance = (self.positions[i][0] - self.positions[j][0])
                .hypot(self.positions[i][1] - self.positions[j][1]);
            let k = self.erodibility[i] * (1. + 2. * smoothstep(0., 5., self.sediment[i]));
            // 300 mm/year is the reference runoff for the old area-based law.
            let erosive_area = if self.climate.is_empty() {
                self.area[i]
            } else {
                self.runoff_flow[i] / 0.3
            };
            let factor = dt * k * erosive_area.powf(0.45) / distance;
            let next = (self.height[i] + factor * receiver_height) / (1. + factor);
            let cut = self.height[i] - next;
            self.height[i] = next;
            self.sediment[i] = (self.sediment[i] - cut).max(0.);
            let volume = cut * self.cell_area[i];
            self.load[i] += volume;
            self.eroded_m3 += volume;
        }
        // Route sediment downstream; preferential settling on gentle terrain.
        // Deposition is limited by supply and donor headroom, never invented volume.
        for &i in self.order.iter().rev() {
            let j = self.receiver[i];
            if j == i {
                self.exported_m3 += self.load[i];
                continue;
            }
            let distance = (self.positions[i][0] - self.positions[j][0])
                .hypot(self.positions[i][1] - self.positions[j][1]);
            let slope = ((self.height[i] - self.height[j]) / distance).max(0.);
            let settle = (1. - (-distance / 3000.).exp()) * (1. - smoothstep(0.005, 0.08, slope));
            let min_donor = self.edges[i]
                .iter()
                .filter(|e| self.receiver[e.node] == i)
                .map(|e| self.height[e.node])
                .fold(f64::INFINITY, f64::min);
            let headroom = (min_donor - self.height[i]).max(0.) * 0.4 * self.cell_area[i];
            let lake_depth = (self.filled[i] - self.height[i]).max(0.);
            let deposit = if self.submerged[i] && lake_depth > 1e-6 {
                // Sediment entering a lake settles before it can leave via the spill.
                // Excess supply passes on once this node reaches the spill elevation.
                self.load[i].min(lake_depth * self.cell_area[i])
            } else {
                (self.load[i] * settle).min(headroom)
            };
            self.height[i] += deposit / self.cell_area[i];
            self.sediment[i] += deposit / self.cell_area[i];
            self.deposited_m3 += deposit;
            self.load[j] += self.load[i] - deposit;
        }
        // Symmetric hillslope transfer: the same volume leaves and enters an edge.
        // Bounded explicit diffusion; adapt substeps to spacing and dt.
        let diffusivity = 0.12;
        // Positivity bound uses actual edge lengths/control areas, not nominal
        // spacing: irregular points can have much shorter edges than a grid.
        let substeps = (dt * diffusivity * self.diffusion_rate / 0.25)
            .ceil()
            .max(1.) as usize;
        for _ in 0..substeps {
            self.delta.fill(0.);
            for i in 0..self.height.len() {
                for e in &self.edges[i] {
                    let j = e.node;
                    if j <= i {
                        continue;
                    }
                    let (hi, lo) = if self.height[i] > self.height[j] {
                        (i, j)
                    } else {
                        (j, i)
                    };
                    if self.boundary[hi] {
                        continue;
                    }
                    let volume = diffusivity * dt / substeps as f64
                        * (self.height[hi] - self.height[lo])
                        * (self.cell_area[hi].min(self.cell_area[lo]) / e.length.powi(2));
                    self.delta[hi] -= volume;
                    if self.boundary[lo] {
                        self.exported_m3 += volume;
                    } else {
                        self.delta[lo] += volume;
                    }
                }
            }
            for i in 0..self.height.len() {
                let change = self.delta[i] / self.cell_area[i];
                self.height[i] += change;
                self.sediment[i] = (self.sediment[i] + change).max(0.);
            }
        }
    }

    pub fn volume_error(&self) -> f64 {
        let volume: f64 = self
            .height
            .iter()
            .zip(&self.cell_area)
            .map(|(h, a)| h * a)
            .sum();
        volume + self.exported_m3 - self.initial_volume_m3 - self.uplifted_m3
    }

    pub fn diagnostics(&self) -> String {
        let min = self.height.iter().copied().fold(f64::INFINITY, f64::min);
        let max = self
            .height
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let water = self.water_surface();
        let lake_nodes = self
            .height
            .iter()
            .zip(water)
            .enumerate()
            .filter(|(i, (h, w))| !self.ocean[*i] && **w - **h > 1e-6)
            .count();
        let lake_volume: f64 = self
            .height
            .iter()
            .zip(water)
            .zip(&self.cell_area)
            .enumerate()
            .map(|(i, ((h, w), a))| {
                if self.ocean[i] {
                    0.
                } else {
                    (w - h).max(0.) * a
                }
            })
            .sum();
        let mut uphill = 0;
        let mut bed_uphill = 0;
        let mut channels = 0;
        let mut gentle = 0;
        let mut angle = [0usize; 18];
        for i in 0..self.height.len() {
            let j = self.receiver[i];
            if j == i {
                continue;
            }
            let dx = self.positions[j][0] - self.positions[i][0];
            let dz = self.positions[j][1] - self.positions[i][1];
            let slope = (self.height[i] - self.height[j]) / dx.hypot(dz);
            gentle += usize::from(slope.abs() < 0.05);
            if self.area[i] > 5e6 {
                channels += 1;
                bed_uphill += usize::from(slope < -1e-8);
                uphill += usize::from(self.filled[j] > self.filled[i] + 1e-8);
                let a = dz.atan2(dx).rem_euclid(std::f64::consts::PI);
                angle[((a / std::f64::consts::PI * 18.) as usize).min(17)] += 1;
            }
        }
        let ocean_nodes = self.ocean.iter().filter(|&&v| v).count();
        let geography = if self.config.atlas {
            "seeded_atlas_v1"
        } else {
            "regional_fixture"
        };
        let hydro = if self.hydrology.is_some() {
            "annual_runoff_permanent_lakes_v1"
        } else {
            "static_wet_spill_surfaces"
        };
        format!("geography={geography}\nsea_level_m=0\nocean_nodes={ocean_nodes}\nseed={}\nside={}\nextent_m={}\nsteps={}\ndt_years={}\nirregular={}\nmin_m={min:.3}\nmax_m={max:.3}\ngentle_fraction={:.4}\nlarge_drainage_nodes={channels}\nhydrology={hydro}\nlake_nodes={lake_nodes}\nlake_storage_m3={lake_volume:.3}\nunresolved_spill_links={uphill}\nlake_traversal_uphill_bed_links={bed_uphill}\nchannel_orientation_bins_10deg={angle:?}\nuplifted_m3={:.3}\nexported_m3={:.3}\neroded_m3={:.3}\ndeposited_m3={:.3}\nvolume_residual_m3={:.6}\n",self.config.seed,self.config.side,self.config.extent,self.config.steps,self.config.dt,self.config.irregular,gentle as f64/self.height.len() as f64,self.uplifted_m3,self.exported_m3,self.eroded_m3,self.deposited_m3,self.volume_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coupled_atlas_preserves_geography_and_balances_final_water() {
        for seed in [1, 42, 99] {
            let c = Config {
                atlas: true,
                climate: true,
                extent: 1e6,
                side: 65,
                steps: 16,
                seed,
                ..Config::default()
            };
            let mut l = Landscape::new(c);
            let controls = Landscape::new(Config {
                climate: false,
                ..c
            });
            assert_eq!(l.initial, controls.initial);
            assert_eq!(l.uplift, controls.uplift);
            l.evolve();
            let h = l.hydrology.as_ref().unwrap();
            assert!(h.residual().abs() < h.runoff_m3_year * 1e-10);
            assert!(l.volume_error().abs() < l.uplifted_m3 * 1e-8);
            for i in 0..l.height.len() {
                assert!(h.water[i].is_finite() && h.water[i] >= l.height[i]);
                assert!(l.climate[i].fields().iter().all(|v| v.is_finite()));
                assert!(h.discharge[i] >= 0.);
                if h.discharge[i] > 0. && l.receiver[i] != i {
                    assert!(h.water[l.receiver[i]] <= h.water[i] + 1e-8);
                }
            }
            l.route();
            assert!(
                l.hydrology.is_none(),
                "editing/rerouting invalidates compiled hydrology"
            );
        }
    }
    #[test]
    fn atlas_sea_outlets_conserve_volume_and_drainage() {
        for seed in [1, 42, 99] {
            let mut l = Landscape::new(Config {
                atlas: true,
                extent: 1e6,
                seed,
                side: 65,
                steps: 20,
                ..Config::default()
            });
            assert!(l.ocean.iter().any(|&o| o));
            l.evolve();
            let mut seen = vec![false; l.height.len()];
            for &i in &l.order {
                assert!(l.receiver[i] == i || seen[l.receiver[i]]);
                seen[i] = true;
                assert!(l.height[i].is_finite());
                assert!(l.filled[l.receiver[i]] <= l.filled[i]);
                if l.ocean[i] {
                    assert_eq!(l.filled[i], 0.);
                    assert_eq!(l.height[i], l.initial[i]);
                }
            }
            let area: f64 = (0..l.height.len())
                .filter(|&i| l.receiver[i] == i)
                .map(|i| l.area[i])
                .sum();
            assert!((area - l.config.extent.powi(2)).abs() < 0.1);
            assert!(l.volume_error().abs() < l.uplifted_m3.abs() * 1e-8);
        }
    }
    #[test]
    fn mesh_has_no_inverted_faces_and_covers_domain() {
        for seed in [1, 42, 99, 12345] {
            let l = Landscape::new(Config {
                side: 65,
                seed,
                ..Config::default()
            });
            for t in &l.triangles {
                let [a, b, c] = t.map(|i| l.positions[i as usize]);
                assert!((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.);
            }
            assert!((l.cell_area.iter().sum::<f64>() - l.config.extent.powi(2)).abs() < 0.01);
            // Independent local Delaunay check: opposite angles may not exceed pi.
            let mut pairs = std::collections::HashMap::<(u32, u32), Vec<u32>>::new();
            for t in &l.triangles {
                for i in 0..3 {
                    let (a, b, c) = (t[i], t[(i + 1) % 3], t[(i + 2) % 3]);
                    pairs.entry((a.min(b), a.max(b))).or_default().push(c);
                }
            }
            for ((a, b), opposites) in pairs {
                if opposites.len() != 2 {
                    continue;
                }
                let angle = |c: u32| {
                    let a = l.positions[a as usize];
                    let b = l.positions[b as usize];
                    let c = l.positions[c as usize];
                    let u = [a[0] - c[0], a[1] - c[1]];
                    let v = [b[0] - c[0], b[1] - c[1]];
                    (u[0] * v[1] - u[1] * v[0])
                        .abs()
                        .atan2(u[0] * v[0] + u[1] * v[1])
                };
                assert!(angle(opposites[0]) + angle(opposites[1]) <= std::f64::consts::PI + 1e-9);
            }
        }
    }
    #[test]
    fn routing_conserves_area_and_is_acyclic() {
        let mut l = Landscape::new(Config {
            side: 33,
            steps: 12,
            ..Config::default()
        });
        // An explicit closed bowl exercises spill routing, not just downhill slopes.
        for (i, p) in l.positions.iter().enumerate() {
            l.height[i] = ((p[0] - 50_000.).hypot(p[1] - 50_000.) / 100.).min(300.);
        }
        l.route();
        // All bowl water is level with its outlet; flow may cross an uphill bed,
        // but never an uphill water surface. No artificial lake slope or bed fill.
        for i in 0..l.height.len() {
            assert!(l.filled[i] >= l.height[i]);
            assert!(l.filled[l.receiver[i]] <= l.filled[i]);
            assert!((l.filled[i] - 300.).abs() < 1e-8);
        }
        let mut seen = vec![false; l.height.len()];
        for &i in &l.order {
            assert!(l.receiver[i] == i || seen[l.receiver[i]]);
            seen[i] = true;
        }
        let outlets: f64 = (0..l.height.len())
            .filter(|&i| l.receiver[i] == i)
            .map(|i| l.area[i])
            .sum();
        assert!((outlets - l.config.extent.powi(2)).abs() < 0.01);
    }
    #[test]
    fn evolution_is_repeatable_and_conserves_volume() {
        let c = Config {
            side: 33,
            steps: 20,
            ..Config::default()
        };
        let mut a = Landscape::new(c);
        let mut b = Landscape::new(c);
        a.evolve();
        b.evolve();
        assert_eq!(a.height, b.height);
        assert!(a.height.iter().all(|v| v.is_finite() && *v >= 0.));
        assert!(a.sediment.iter().all(|v| *v >= 0.));
        assert!(a.volume_error().abs() < a.uplifted_m3 * 1e-9);
        for i in 0..a.height.len() {
            assert!(a.filled[a.receiver[i]] <= a.filled[i]);
        }
    }
    #[test]
    fn incision_of_dry_slopes_does_not_turn_old_ground_into_lakes() {
        let mut l = Landscape::new(Config {
            side: 33,
            steps: 1,
            ..Config::default()
        });
        for (i, p) in l.positions.iter().enumerate() {
            l.height[i] = 100. + p[0] * 0.2;
        }
        l.uplift.fill(0.);
        l.route();
        assert!(l.submerged.iter().all(|wet| !*wet));
        l.step(0);
        assert!(l.eroded_m3 > 0.);
        // A pre-incision ground elevation is not a standing water surface.
        assert!(l.deposited_m3 < l.eroded_m3 * 0.5);
        assert!(l.exported_m3 > l.eroded_m3 * 0.5);
    }
    #[test]
    fn flat_surface_does_not_erode_or_create_sediment() {
        let mut l = Landscape::new(Config {
            side: 17,
            steps: 5,
            ..Config::default()
        });
        l.height.fill(100.);
        l.uplift.fill(0.);
        l.evolve();
        assert!(l.height.iter().all(|h| *h == 100.));
        assert_eq!(l.exported_m3, 0.);
    }
}
