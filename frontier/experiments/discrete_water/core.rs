//! Greenfield experimental whole-cell water. Binary occupancy is authoritative.
//! MAC-style pressure projection guides moves; quantization/resolution is NOT
//! a consistent discretization of Navier–Stokes and does not conserve energy.
use std::time::Instant;
pub const CELL_SIZE_M: f64 = 0.0625;
pub const DT: f64 = 0.025;
#[derive(Clone, Debug)]
pub struct Face {
    pub a: usize,
    pub b: usize,
    pub axis: usize,
    pub velocity: f64,
    pub credit: f64,
}
#[derive(Clone)]
pub struct World {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub water: Vec<bool>,
    pub solid: Vec<bool>,
    pub pressure: Vec<f64>,
    pub faces: Vec<Face>,
    adjacent: Vec<Vec<(usize, usize, bool)>>,
    pub tick: usize,
    pub initial_mass: usize,
}
#[derive(Default, Clone, Debug)]
pub struct Metrics {
    pub mass: usize,
    pub moved: usize,
    pub velocity_energy_proxy: f64,
    pub pressure_max: f64,
    pub pressure_rms: f64,
    pub max_column: usize,
    pub step_ms: f64,
    pub divergence_rms: f64,
    pub candidates: usize,
    pub rejected: usize,
    pub cycles: usize,
    pub speed_clamps: usize,
    pub credit_saturations: usize,
    pub pressure_residual_rms: f64,
}
#[derive(Clone, Copy, Debug)]
pub struct Move {
    pub from: usize,
    pub to: usize,
    pub face: usize,
    pub score: f64,
}
impl World {
    pub fn new(nx: usize, ny: usize, nz: usize) -> Self {
        let n = nx * ny * nz;
        let mut w = Self {
            nx,
            ny,
            nz,
            water: vec![false; n],
            solid: vec![false; n],
            pressure: vec![0.; n],
            faces: Vec::new(),
            adjacent: vec![Vec::new(); n],
            tick: 0,
            initial_mass: 0,
        };
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let a = w.idx(x, y, z);
                    for (axis, valid, b) in [
                        (0, x + 1 < nx, a + 1),
                        (1, y + 1 < ny, a + nx),
                        (2, z + 1 < nz, a + nx * ny),
                    ] {
                        if valid {
                            let fi = w.faces.len();
                            w.faces.push(Face {
                                a,
                                b,
                                axis,
                                velocity: 0.,
                                credit: 0.,
                            });
                            w.adjacent[a].push((b, fi, true));
                            w.adjacent[b].push((a, fi, false));
                        }
                    }
                }
            }
        }
        w
    }
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }
    pub fn mass(&self) -> usize {
        self.water.iter().filter(|&&b| b).count()
    }
    pub fn seal_mass(&mut self) {
        self.initial_mass = self.mass();
    }
    pub fn validate(&self) {
        assert_eq!(self.mass(), self.initial_mass, "mass changed");
        assert!(
            self.water.iter().zip(&self.solid).all(|(&w, &s)| !(w && s)),
            "water in solid"
        );
    }
    pub fn step(&mut self, reference: bool) -> Metrics {
        let start = Instant::now();
        let n = self.water.len();
        let wet: Vec<usize> = (0..n).filter(|&i| self.water[i]).collect();
        let mut div = vec![0.; n];
        for f in &mut self.faces {
            if self.solid[f.a] || self.solid[f.b] || !(self.water[f.a] || self.water[f.b]) {
                f.velocity = 0.;
                f.credit = 0.;
                continue;
            }
            if f.axis == 1 {
                f.velocity -= 9.81 / CELL_SIZE_M * DT;
            }
            div[f.a] += f.velocity;
            div[f.b] -= f.velocity;
        }
        for i in 0..n {
            if !self.water[i] {
                self.pressure[i] = 0.;
            }
        }
        // Fixed-count SOR free-surface Poisson solve. Solid faces have zero flux;
        // dry cells impose p=0. Nonnegative pressure avoids tensile attraction.
        for _ in 0..64 {
            for &i in &wet {
                let mut sum = 0.;
                let mut degree = 0.;
                for &(j, _, _) in &self.adjacent[i] {
                    if !self.solid[j] {
                        degree += 1.;
                        sum += self.pressure[j];
                    }
                }
                if degree > 0. {
                    let target = ((sum - div[i] / DT) / degree).max(0.);
                    self.pressure[i] =
                        (self.pressure[i] + 1.4 * (target - self.pressure[i])).max(0.);
                }
            }
        }
        let pressure_residual_rms = (wet
            .iter()
            .map(|&i| {
                let lap: f64 = self.adjacent[i]
                    .iter()
                    .filter(|&&(j, _, _)| !self.solid[j])
                    .map(|&(j, _, _)| self.pressure[j] - self.pressure[i])
                    .sum();
                (lap - div[i] / DT).powi(2)
            })
            .sum::<f64>()
            / wet.len().max(1) as f64)
            .sqrt();
        div.fill(0.);
        let mut speed_clamps = 0;
        let mut credit_saturations = 0;
        let speed_limit = 0.8 / DT;
        for f in &mut self.faces {
            if self.solid[f.a] || self.solid[f.b] || !(self.water[f.a] || self.water[f.b]) {
                continue;
            }
            let proposed = f.velocity - DT * (self.pressure[f.b] - self.pressure[f.a]);
            if proposed.abs() > speed_limit {
                speed_clamps += 1;
            }
            f.velocity = proposed.clamp(-speed_limit, speed_limit);
            div[f.a] += f.velocity;
            div[f.b] -= f.velocity;
            let accumulated = f.credit + f.velocity * DT;
            if accumulated.abs() > 2. {
                credit_saturations += 1;
            }
            f.credit = accumulated.clamp(-2., 2.);
        }
        let mut candidates = Vec::new();
        for (fi, f) in self.faces.iter().enumerate() {
            let (from, to) = if f.credit >= 1. {
                (f.a, f.b)
            } else if f.credit <= -1. {
                (f.b, f.a)
            } else {
                continue;
            };
            if self.water[from] && !self.solid[to] {
                candidates.push(Move {
                    from,
                    to,
                    face: fi,
                    score: f.credit.abs(),
                });
            }
        }
        let (moves, cycles) = if reference && wet.len() <= 64 {
            (reference_assignment(&self.water, &candidates), 0)
        } else {
            resolve_cheap(&self.water, &candidates)
        };
        // Atomic occupancy publication is critical: chains and cycles are legal.
        for m in &moves {
            self.water[m.from] = false;
        }
        for m in &moves {
            self.water[m.to] = true;
            let f = &mut self.faces[m.face];
            f.credit -= if m.from == f.a { 1. } else { -1. };
        }
        self.tick += 1;
        self.validate();
        let mut max_column = 0;
        for z in 0..self.nz {
            for x in 0..self.nx {
                max_column = max_column.max(
                    (0..self.ny)
                        .filter(|&y| self.water[self.idx(x, y, z)])
                        .count(),
                );
            }
        }
        Metrics {
            mass: wet.len(),
            moved: moves.len(),
            velocity_energy_proxy: self
                .faces
                .iter()
                .map(|f| 0.5 * (f.velocity * CELL_SIZE_M).powi(2))
                .sum(),
            pressure_max: self.pressure.iter().copied().fold(0., f64::max)
                * CELL_SIZE_M
                * CELL_SIZE_M,
            pressure_rms: (wet.iter().map(|&i| self.pressure[i].powi(2)).sum::<f64>()
                / wet.len().max(1) as f64)
                .sqrt()
                * CELL_SIZE_M
                * CELL_SIZE_M,
            max_column,
            step_ms: start.elapsed().as_secs_f64() * 1000.,
            divergence_rms: (wet.iter().map(|&i| div[i].powi(2)).sum::<f64>()
                / wet.len().max(1) as f64)
                .sqrt(),
            candidates: candidates.len(),
            rejected: candidates.len() - moves.len(),
            cycles,
            speed_clamps,
            credit_saturations,
            pressure_residual_rms,
        }
    }
    pub fn snapshot(&self) -> String {
        // complete deterministic restart state, versioned
        let mut s = format!(
            "DW1 {} {} {} {} {}\n",
            self.nx, self.ny, self.nz, self.tick, self.initial_mass
        );
        for i in 0..self.water.len() {
            s.push_str(&format!(
                "{} {} {:016x}\n",
                self.water[i] as u8,
                self.solid[i] as u8,
                self.pressure[i].to_bits()
            ));
        }
        for f in &self.faces {
            s.push_str(&format!(
                "{:016x} {:016x}\n",
                f.velocity.to_bits(),
                f.credit.to_bits()
            ));
        }
        s
    }
    pub fn restore(s: &str) -> Self {
        let mut t = s.split_whitespace();
        assert_eq!(t.next(), Some("DW1"));
        let nx = t.next().unwrap().parse().unwrap();
        let ny = t.next().unwrap().parse().unwrap();
        let nz = t.next().unwrap().parse().unwrap();
        let mut w = Self::new(nx, ny, nz);
        w.tick = t.next().unwrap().parse().unwrap();
        w.initial_mass = t.next().unwrap().parse().unwrap();
        for i in 0..w.water.len() {
            w.water[i] = t.next() == Some("1");
            w.solid[i] = t.next() == Some("1");
            w.pressure[i] = f64::from_bits(u64::from_str_radix(t.next().unwrap(), 16).unwrap());
        }
        for f in &mut w.faces {
            f.velocity = f64::from_bits(u64::from_str_radix(t.next().unwrap(), 16).unwrap());
            f.credit = f64::from_bits(u64::from_str_radix(t.next().unwrap(), 16).unwrap());
        }
        assert!(t.next().is_none());
        w.validate();
        w
    }
}
/// Deterministic greedy source/target matching followed by dependency traversal.
/// Reject chains ending at a stationary occupant; accept empty-ended chains and
/// cycles. Arbitration is score-first, with rotating face-index ties each step
/// deliberately omitted so reproducibility does not depend on container order.
pub fn resolve_cheap(water: &[bool], candidates: &[Move]) -> (Vec<Move>, usize) {
    let n = water.len();
    let mut order = candidates.to_vec();
    order.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.face.cmp(&b.face)));
    let mut next = vec![None; n];
    let mut targets = vec![false; n];
    for m in order {
        if next[m.from].is_none() && !targets[m.to] {
            next[m.from] = Some(m);
            targets[m.to] = true;
        }
    }
    let mut state = vec![0u8; n];
    let mut cycles = 0;
    for root in 0..n {
        if next[root].is_none() || state[root] != 0 {
            continue;
        }
        let mut path = Vec::new();
        let mut at = root;
        let accept = loop {
            if !water[at] {
                break true;
            }
            match state[at] {
                1 => {
                    cycles += 1;
                    break true;
                }
                2 => break true,
                3 => break false,
                _ => {}
            }
            if let Some(m) = next[at] {
                state[at] = 1;
                path.push(at);
                at = m.to;
            } else {
                break false;
            }
        };
        for i in path {
            state[i] = if accept { 2 } else { 3 };
        }
    }
    (
        next.into_iter()
            .enumerate()
            .filter_map(|(i, m)| if state[i] == 2 { m } else { None })
            .collect(),
        cycles,
    )
}
/// Bounded maximum-weight injective assignment (Hungarian), all water sources
/// including a zero-score stay option. Small fixtures only (<=64 occupied cells).
/// This is a reference for the move resolver, NOT a continuum fluid reference.
pub fn reference_assignment(water: &[bool], candidates: &[Move]) -> Vec<Move> {
    let sources: Vec<usize> = (0..water.len()).filter(|&i| water[i]).collect();
    assert!(sources.len() <= 64);
    if sources.is_empty() {
        return vec![];
    }
    let mut targets = sources.clone();
    targets.extend(candidates.iter().map(|m| m.to));
    targets.sort_unstable();
    targets.dedup();
    let n = sources.len();
    let m = targets.len();
    let mut costs = vec![vec![1e9; m]; n];
    for (i, &s) in sources.iter().enumerate() {
        costs[i][targets.binary_search(&s).unwrap()] = 0.;
    }
    for c in candidates {
        let i = sources.binary_search(&c.from).unwrap();
        let j = targets.binary_search(&c.to).unwrap();
        costs[i][j] = -c.score;
    }
    let (mut u, mut v, mut p, mut way) = (
        vec![0.; n + 1],
        vec![0.; m + 1],
        vec![0usize; m + 1],
        vec![0usize; m + 1],
    );
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0;
        let mut minv = vec![f64::INFINITY; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = f64::INFINITY;
            let mut j1 = 0;
            for j in 1..=m {
                if !used[j] {
                    let cur = costs[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut out = Vec::new();
    for j in 1..=m {
        if p[j] > 0 {
            let s = sources[p[j] - 1];
            let t = targets[j - 1];
            if s != t {
                out.push(
                    *candidates
                        .iter()
                        .find(|c| c.from == s && c.to == t)
                        .unwrap(),
                );
            }
        }
    }
    out
}
pub fn fixture(name: &str) -> World {
    let mut w = World::new(32, 24, 8);
    for z in 0..w.nz {
        for y in 0..w.ny {
            for x in 0..w.nx {
                let i = w.idx(x, y, z);
                w.solid[i] = x == 0 || x == 31 || y == 0 || y == 23 || z == 0 || z == 7;
                if name == "vessels" && x == 15 && y >= 5 {
                    w.solid[i] = true;
                }
                if name == "obstacle" && (15..19).contains(&x) && y <= 8 {
                    w.solid[i] = true;
                }
                w.water[i] = !w.solid[i]
                    && match name {
                        "falling" => (10..18).contains(&x) && (14..21).contains(&y),
                        "dam" => x <= 11 && y <= 16,
                        "pool" => y <= 7,
                        "vessels" => {
                            if x < 15 {
                                y <= 17
                            } else {
                                y <= 5
                            }
                        }
                        "obstacle" => x <= 10 && y <= 15,
                        _ => panic!("unknown fixture"),
                    };
            }
        }
    }
    w.seal_mass();
    w
}
#[cfg(test)]
mod tests {
    use super::*;
    fn mv(a: usize, b: usize, f: usize, s: f64) -> Move {
        Move {
            from: a,
            to: b,
            face: f,
            score: s,
        }
    }
    #[test]
    fn chains() {
        let (w, c) = (
            vec![true, true, true, false],
            vec![mv(0, 1, 0, 1.), mv(1, 2, 1, 1.), mv(2, 3, 2, 1.)],
        );
        assert_eq!(resolve_cheap(&w, &c).0.len(), 3);
        assert_eq!(reference_assignment(&w, &c).len(), 3);
    }
    #[test]
    fn cycle() {
        let (w, c) = (
            vec![true; 3],
            vec![mv(0, 1, 0, 1.), mv(1, 2, 1, 1.), mv(2, 0, 2, 1.)],
        );
        let (m, k) = resolve_cheap(&w, &c);
        assert_eq!(m.len(), 3);
        assert_eq!(k, 1);
    }
    #[test]
    fn blocked_chain() {
        assert!(
            resolve_cheap(&[true, true, true], &[mv(0, 1, 0, 2.), mv(1, 2, 1, 2.)])
                .0
                .is_empty()
        );
    }
    #[test]
    fn conflict() {
        let m = resolve_cheap(&[true, true, false], &[mv(0, 2, 0, 1.), mv(1, 2, 1, 2.)]).0;
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].from, 1);
    }
    #[test]
    fn reference_improves_greedy() {
        let w = [true, true, false, false];
        let c = [mv(0, 2, 0, 2.), mv(0, 3, 1, 1.9), mv(1, 2, 2, 1.8)];
        assert_eq!(resolve_cheap(&w, &c).0.len(), 1);
        assert_eq!(reference_assignment(&w, &c).len(), 2);
    }
    #[test]
    fn fixtures_conserve_and_exclude_solids() {
        for name in ["falling", "dam", "pool", "vessels", "obstacle"] {
            let mut w = fixture(name);
            for _ in 0..40 {
                w.step(false);
                w.validate();
            }
        }
    }
    #[test]
    fn restart_determinism() {
        let mut w = fixture("dam");
        for _ in 0..9 {
            w.step(false);
        }
        let mut r = World::restore(&w.snapshot());
        for _ in 0..12 {
            w.step(false);
            r.step(false);
            assert_eq!(w.snapshot(), r.snapshot());
        }
    }
    #[test]
    fn pool_stays_still() {
        let mut w = fixture("pool");
        let initial = w.water.clone();
        for _ in 0..80 {
            w.step(false);
        }
        assert_eq!(w.water, initial);
    }
    #[test]
    fn falling_moves() {
        let mut w = fixture("falling");
        let initial = w.water.clone();
        for _ in 0..20 {
            w.step(false);
        }
        assert_ne!(w.water, initial);
    }
}
