//! Immutable compiler output and compact triangle lookup. Runtime never evolves it.
use super::refine::{Channel, Patch};
use crate::generation::{atlas_climate::Cell, landscape_paint::Context};
use std::{fs, path::Path};

#[derive(Clone, Copy)]
pub struct Node {
    pub p: [f64; 2],
    pub height: f64,
    pub water: f64,
    pub climate: Cell,
    pub sediment: f32,
    pub discharge: f32,
    pub receiver: u32,
    pub watershed: u32,
    pub gradient: [f32; 2],
    /// Derived runtime geometry; never written into the physical atlas.
    pub river_bend: f32,
    pub river_profile: u32,
    pub river_level: f64,
}
pub struct Data {
    pub seed: u64,
    pub extent: f64,
    pub side: usize,
    pub nodes: Vec<Node>,
    pub triangles: Vec<[u32; 3]>,
    offsets: Vec<u32>,
    bins: Vec<u32>,
    pub fingerprint: u64,
    pub low: f64,
    pub high: f64,
    channel_offsets: Vec<u32>,
    channel_bins: Vec<u32>,
    river_profiles: Vec<[f32; 17]>,
}

fn bytes(path: &Path, name: &str, size: usize, hash: &mut u64) -> Result<Vec<u8>, String> {
    let p = path.join(name);
    if fs::metadata(&p)
        .map_err(|e| format!("{}: {e}", p.display()))?
        .len()
        != size as u64
    {
        return Err(format!("invalid atlas {name} length"));
    }
    let b = fs::read(&p).map_err(|e| e.to_string())?;
    for &v in &b {
        *hash = (*hash ^ v as u64).wrapping_mul(0x100000001b3);
    }
    Ok(b)
}
fn f64_at(b: &[u8], i: usize) -> f64 {
    f64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}
fn f32_at(b: &[u8], i: usize) -> f32 {
    f32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}

impl Data {
    pub fn load(path: &Path) -> Result<Self, String> {
        let meta = fs::read_to_string(path.join("summary.txt")).map_err(|e| e.to_string())?;
        let value = |key: &str| {
            meta.lines()
                .find_map(|l| l.strip_prefix(&format!("{key}=")))
                .ok_or_else(|| format!("missing atlas {key}"))
        };
        if value("climate_model")? != "ocean_transport_v1" {
            return Err("atlas requires compiled climate".into());
        }
        let seed = value("seed")?.parse::<u64>().map_err(|e| e.to_string())?;
        let side = value("side")?.parse::<usize>().map_err(|e| e.to_string())?;
        let extent = value("extent_m")?
            .parse::<f64>()
            .map_err(|e| e.to_string())?;
        if !(3..=2049).contains(&side)
            || !extent.is_finite()
            || !(10_000. ..=1_000_000.).contains(&extent)
        {
            return Err("invalid atlas dimensions".into());
        }
        let n = side * side;
        let mut fingerprint = 0xcbf29ce484222325;
        let raw = bytes(path, "nodes.bin", n * 64, &mut fingerprint)?;
        let mut nodes = Vec::with_capacity(n);
        for b in raw.chunks_exact(64) {
            let v: [f64; 8] = std::array::from_fn(|i| f64_at(b, i * 8));
            if v.iter().any(|x| !x.is_finite())
                || v[0] < 0.
                || v[1] < 0.
                || v[0] > extent
                || v[1] > extent
                || v[2].abs() > 30_000.
                || v[6] < 0.
                || v[7] < 0.
                || v[7] >= n as f64
                || v[7].fract() != 0.
            {
                return Err("invalid atlas node".into());
            }
            nodes.push(Node {
                p: [v[0], v[1]],
                height: v[2],
                water: v[2],
                climate: Cell::default(),
                sediment: v[6] as f32,
                discharge: 0.,
                receiver: v[7] as u32,
                watershed: 0,
                gradient: [0.; 2],
                river_bend: 0.,
                river_profile: u32::MAX,
                river_level: 0.,
            });
        }
        drop(raw);
        let raw = bytes(path, "water.bin", n * 8, &mut fingerprint)?;
        for (i, b) in raw.chunks_exact(8).enumerate() {
            let w = f64_at(b, 0);
            if !w.is_finite() || w < nodes[i].height || w > 30_000. {
                return Err("invalid atlas water".into());
            }
            nodes[i].water = w;
        }
        drop(raw);
        let raw = bytes(path, "climate.bin", n * 32, &mut fingerprint)?;
        for (i, b) in raw.chunks_exact(32).enumerate() {
            let v = std::array::from_fn(|j| f32_at(b, j * 4));
            if v.iter().any(|x| !x.is_finite()) || !(0. ..=1.).contains(&v[6]) {
                return Err("invalid atlas climate".into());
            }
            nodes[i].climate = Cell::from_fields(v);
        }
        drop(raw);
        let raw = bytes(path, "flow.bin", n * 16, &mut fingerprint)?;
        for (i, b) in raw.chunks_exact(16).enumerate() {
            let q = f64_at(b, 0);
            if !q.is_finite() || !(0. ..=1e8).contains(&q) {
                return Err("invalid atlas flow".into());
            }
            nodes[i].discharge = q as f32;
        }
        drop(raw);
        let raw = bytes(path, "watershed.bin", n * 4, &mut fingerprint)?;
        for (i, b) in raw.chunks_exact(4).enumerate() {
            let id = u32::from_le_bytes(b.try_into().unwrap());
            if id as usize >= n {
                return Err("invalid watershed".into());
            }
            nodes[i].watershed = id;
        }
        drop(raw);
        let raw = bytes(
            path,
            "triangles.bin",
            2 * (side - 1) * (side - 1) * 12,
            &mut fingerprint,
        )?;
        let triangles: Vec<[u32; 3]> = raw
            .chunks_exact(12)
            .map(|b| {
                std::array::from_fn(|i| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
            })
            .collect();
        if triangles.iter().flatten().any(|&i| i as usize >= n) {
            return Err("atlas triangle index out of bounds".into());
        }
        drop(raw);
        let mut data = Self {
            seed,
            extent,
            side,
            nodes,
            triangles,
            offsets: Vec::new(),
            bins: Vec::new(),
            fingerprint,
            low: f64::INFINITY,
            high: f64::NEG_INFINITY,
            channel_offsets: Vec::new(),
            channel_bins: Vec::new(),
            river_profiles: Vec::new(),
        };
        data.index()?;
        Ok(data)
    }
    fn rect(&self, t: [u32; 3]) -> [usize; 4] {
        let p = t.map(|i| self.nodes[i as usize].p);
        let cell = self.extent / (self.side - 1) as f64;
        let bin = |v: f64| ((v / cell).floor() as usize).min(self.side - 2);
        [
            bin(p.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min)),
            bin(p.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)),
            bin(p.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max)),
            bin(p.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max)),
        ]
    }
    fn index(&mut self) -> Result<(), String> {
        let side = self.side - 1;
        let mut counts = vec![0u32; side * side];
        let mut weights = vec![[0f64; 3]; self.nodes.len()];
        for &t in &self.triangles {
            let [a, b, c] = t.map(|i| self.nodes[i as usize]);
            let dx = b.p[0] - a.p[0];
            let dz = b.p[1] - a.p[1];
            let ex = c.p[0] - a.p[0];
            let ez = c.p[1] - a.p[1];
            let det = dx * ez - dz * ex;
            if det <= 1e-12 {
                return Err("invalid atlas triangle winding".into());
            }
            let gx = ((b.height - a.height) * ez - (c.height - a.height) * dz) / det;
            let gz = (dx * (c.height - a.height) - ex * (b.height - a.height)) / det;
            for i in t {
                weights[i as usize][0] += gx * det;
                weights[i as usize][1] += gz * det;
                weights[i as usize][2] += det;
            }
            let [x0, z0, x1, z1] = self.rect(t);
            if x1 - x0 > 8 || z1 - z0 > 8 {
                return Err("atlas triangle exceeds bounded lookup span".into());
            }
            for z in z0..=z1 {
                for x in x0..=x1 {
                    counts[z * side + x] += 1;
                }
            }
        }
        for (n, w) in self.nodes.iter_mut().zip(weights) {
            if w[2] <= 0. {
                return Err("unconnected atlas node".into());
            }
            n.gradient = [(w[0] / w[2]) as f32, (w[1] / w[2]) as f32];
            self.low = self.low.min(n.height);
            self.high = self.high.max(n.water);
        }
        self.offsets = Vec::with_capacity(counts.len() + 1);
        self.offsets.push(0);
        for c in &counts {
            self.offsets.push(self.offsets.last().unwrap() + c);
        }
        self.bins = vec![0; *self.offsets.last().unwrap() as usize];
        counts.copy_from_slice(&self.offsets[..side * side]);
        for (id, &t) in self.triangles.iter().enumerate() {
            let [x0, z0, x1, z1] = self.rect(t);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let i = z * side + x;
                    self.bins[counts[i] as usize] = id as u32;
                    counts[i] += 1;
                }
            }
        }
        self.compile_rivers()?;
        // Full discharge-derived influence rectangles, storing only source IDs.
        let mut counts = vec![0u32; side * side];
        let mut total = 0u64;
        for node in &self.nodes {
            if let Some(channel) = self.channel(node) {
                let [x0, z0, x1, z1] = self.channel_rect(channel);
                total += ((x1 - x0 + 1) * (z1 - z0 + 1)) as u64;
                if total > 64_000_000 {
                    return Err("atlas channel index exceeds memory budget".into());
                }
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        counts[z * side + x] += 1;
                    }
                }
            }
        }
        self.channel_offsets = Vec::with_capacity(counts.len() + 1);
        self.channel_offsets.push(0);
        for c in &counts {
            self.channel_offsets
                .push(self.channel_offsets.last().unwrap() + c);
        }
        self.channel_bins = vec![0; total as usize];
        counts.copy_from_slice(&self.channel_offsets[..side * side]);
        for (id, node) in self.nodes.iter().enumerate() {
            if let Some(channel) = self.channel(node) {
                let [x0, z0, x1, z1] = self.channel_rect(channel);
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        let i = z * side + x;
                        self.channel_bins[counts[i] as usize] = id as u32;
                        counts[i] += 1;
                    }
                }
            }
        }
        Ok(())
    }
    fn compile_rivers(&mut self) -> Result<(), String> {
        self.river_profiles.clear();
        for n in &mut self.nodes {
            n.river_profile = u32::MAX;
            n.river_bend = 0.;
            n.river_level = n.water;
        }
        let count = self.nodes.len();
        let active: Vec<bool> = self
            .nodes
            .iter()
            .map(|n| self.channel(n).is_some())
            .collect();
        let mut incoming = vec![0u32; count];
        for (i, n) in self.nodes.iter().enumerate() {
            if active[i] {
                incoming[n.receiver as usize] += 1;
            }
        }
        let mut order: Vec<usize> = incoming
            .iter()
            .enumerate()
            .filter(|(_, d)| **d == 0)
            .map(|(i, _)| i)
            .collect();
        let mut cursor = 0;
        while cursor < order.len() {
            let i = order[cursor];
            cursor += 1;
            if active[i] {
                let down = self.nodes[i].receiver as usize;
                incoming[down] -= 1;
                if incoming[down] == 0 {
                    order.push(down);
                }
            }
        }
        if order.len() != count {
            return Err("compiled permanent river receiver graph contains a cycle".into());
        }
        let fixed: Vec<bool> = self
            .nodes
            .iter()
            .map(|n| n.water > n.height + 0.01)
            .collect();
        let mut floor: Vec<f64> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| n.water - if fixed[i] { 0. } else { 20. })
            .collect();
        for &i in order.iter().rev() {
            if active[i] {
                floor[i] = floor[i].max(floor[self.nodes[i].receiver as usize]);
            }
        }
        let mut levels: Vec<f64> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if fixed[i] {
                    n.water
                } else {
                    (n.water - 0.6).max(floor[i])
                }
            })
            .collect();
        for &i in &order {
            if !active[i] {
                continue;
            }
            let n = self.nodes[i];
            let down_id = n.receiver as usize;
            let down = self.nodes[down_id];
            let mut channel = self.channel(&n).unwrap();
            let dx = down.p[0] - n.p[0];
            let dz = down.p[1] - n.p[1];
            let cross = -(n.gradient[0] as f64 + down.gradient[0] as f64) * dz
                + (n.gradient[1] as f64 + down.gradient[1] as f64) * dx;
            let sign = if cross.abs() > 1e-8 {
                cross.signum()
            } else if crate::painting::hash(self.seed ^ i as u64) & 1 == 0 {
                1.
            } else {
                -1.
            };
            let preferred = sign * (dx.hypot(dz) * 0.12).min(150.);
            let mut chosen = [0.; 17];
            let mut bend = preferred;
            for attempt in 0..6 {
                if attempt == 5 {
                    bend = 0.;
                }
                channel.bend = bend;
                let mut banks = [0.; 65];
                let mut valid = true;
                for (j, bank) in banks.iter_mut().enumerate() {
                    let t = j as f64 / 64.;
                    let q = channel.position(t);
                    let Some((id, w)) = self.locate([q[0], q[2]]) else {
                        valid = false;
                        break;
                    };
                    let patch = Patch::bank(self, id);
                    let mut height = patch.height(w) - 0.6;
                    // Compiled lake support stays fixed rather than being
                    // drained by the visible river reconstruction.
                    if (0..3).any(|k| patch.wet_depth[k] > 0. && w[k] > 0.) {
                        height = height.max(n.water + (down.water - n.water) * t);
                    }
                    *bank = height;
                    let original = n.water + (down.water - n.water) * t;
                    if j > 0 && j < 64 && (height < floor[down_id] || height - original > 70.) {
                        valid = false;
                    }
                }
                if !valid && attempt < 5 {
                    bend *= 0.5;
                    continue;
                }
                let minimum = banks[1..64]
                    .iter()
                    .copied()
                    .fold(down.water, f64::min)
                    .max(floor[down_id]);
                let end = levels[down_id].min(minimum).min(levels[i]);
                let mut caps = std::array::from_fn(|j| {
                    banks[j * 4].min(n.water + (down.water - n.water) * j as f64 / 16.)
                });
                caps[0] = levels[i];
                caps[16] = end;
                // Constrain interpolation between stored samples, without a
                // voxel-resolution dependent terrain solve at query time.
                for j in 1..64 {
                    let k = j / 4;
                    let u = (j % 4) as f64 / 4.;
                    let estimate = caps[k] * (1. - u) + caps[k + 1] * u;
                    let excess = (estimate - banks[j]).max(0.);
                    if excess > 0. {
                        if k == 0 && u > 0. {
                            caps[1] -= excess / u;
                        } else if k == 15 && u < 1. {
                            caps[15] -= excess / (1. - u);
                        } else {
                            caps[k] -= excess;
                            caps[k + 1] -= excess;
                        }
                    }
                }
                for value in &mut caps[1..16] {
                    *value = value.max(floor[down_id]);
                }
                chosen = caps;
                break;
            }
            if !fixed[down_id] {
                levels[down_id] = levels[down_id]
                    .min(chosen[1..17].iter().copied().fold(levels[i], f64::min))
                    .max(floor[down_id]);
            }
            self.nodes[i].river_bend = bend as f32;
            self.nodes[i].river_profile = self.river_profiles.len() as u32;
            self.river_profiles.push(chosen.map(|v| v as f32));
        }
        for (n, level) in self.nodes.iter_mut().zip(&levels) {
            n.river_level = *level;
        }
        // Incoming branches and the outgoing segment use the exact same
        // quantized junction value. Prefix minima preserve downstream flow.
        for &i in &order {
            if !active[i] {
                continue;
            }
            let n = self.nodes[i];
            let down = n.receiver as usize;
            let profile = &mut self.river_profiles[n.river_profile as usize];
            profile[0] = levels[i] as f32;
            for j in 1..16 {
                profile[j] = profile[j].min(profile[j - 1]).max(levels[down] as f32);
            }
            profile[16] = levels[down] as f32;
        }
        Ok(())
    }
    fn channel_rect(&self, s: Channel) -> [usize; 4] {
        let cell = self.extent / (self.side - 1) as f64;
        let bin = |v: f64| ((v.max(0.) / cell).floor() as usize).min(self.side - 2);
        let [lo, hi] = s.bounds();
        [
            bin(lo[0] - 160.),
            bin(lo[1] - 160.),
            bin(hi[0] + 160.),
            bin(hi[1] + 160.),
        ]
    }
    fn channel(&self, n: &Node) -> Option<Channel> {
        let down = self.nodes.get(n.receiver as usize)?;
        if n.p == down.p || n.discharge < 0.02 || n.water < down.water {
            return None;
        }
        let profile = self
            .river_profiles
            .get(n.river_profile as usize)
            .copied()
            .unwrap_or_else(|| {
                std::array::from_fn(|i| (n.water + (down.water - n.water) * i as f64 / 16.) as f32)
            });
        let start = n.river_level;
        let end = down.river_level;
        Some(Channel {
            profile,
            bend: n.river_bend as f64,
            a: [n.p[0], start, n.p[1]],
            b: [down.p[0], end, down.p[1]],
            width: (10. + 4. * (n.discharge as f64).sqrt()).min(1200.),
            flow: n.discharge as f64,
            local: false,
            compiled: true,
        })
    }
    /// Sorted unique triangles whose spatial bins overlap a physical rectangle.
    pub fn triangle_ids_in_rect(&self, lo: [f64; 2], hi: [f64; 2]) -> Vec<u32> {
        let side = self.side - 1;
        let cell = self.extent / side as f64;
        let bin = |v: f64| ((v.max(0.) / cell).floor() as usize).min(side - 1);
        let mut ids = Vec::new();
        for z in bin(lo[1])..=bin(hi[1]) {
            for x in bin(lo[0])..=bin(hi[0]) {
                let i = z * side + x;
                ids.extend_from_slice(
                    &self.bins[self.offsets[i] as usize..self.offsets[i + 1] as usize],
                );
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }
    /// All compiled receiver channels influencing this rectangle, sorted by source ID.
    pub fn channels_in_rect(&self, lo: [f64; 2], hi: [f64; 2]) -> Vec<Channel> {
        let side = self.side - 1;
        let cell = self.extent / side as f64;
        let bin = |v: f64| ((v.max(0.) / cell).floor() as usize).min(side - 1);
        let mut sources = Vec::new();
        for z in bin(lo[1])..=bin(hi[1]) {
            for x in bin(lo[0])..=bin(hi[0]) {
                let i = z * side + x;
                sources.extend_from_slice(
                    &self.channel_bins
                        [self.channel_offsets[i] as usize..self.channel_offsets[i + 1] as usize],
                );
            }
        }
        sources.sort_unstable();
        sources.dedup();
        sources
            .into_iter()
            .filter_map(|id| self.channel(&self.nodes[id as usize]))
            .filter(|s| {
                let [a, b] = s.bounds();
                (0..2).all(|k| a[k] - 160. <= hi[k] && b[k] + 160. >= lo[k])
            })
            .collect()
    }
    pub fn locate(&self, p: [f64; 2]) -> Option<(u32, [f64; 3])> {
        if p.iter()
            .any(|v| !v.is_finite() || *v < 0. || *v > self.extent)
        {
            return None;
        }
        let side = self.side - 1;
        let spacing = self.extent / side as f64;
        let x = ((p[0] / spacing) as usize).min(side - 1);
        let z = ((p[1] / spacing) as usize).min(side - 1);
        let bin = z * side + x;
        for &id in &self.bins[self.offsets[bin] as usize..self.offsets[bin + 1] as usize] {
            let [a, b, c] = self.triangles[id as usize].map(|i| self.nodes[i as usize].p);
            let dx = b[0] - a[0];
            let dz = b[1] - a[1];
            let ex = c[0] - a[0];
            let ez = c[1] - a[1];
            let det = dx * ez - dz * ex;
            let u = ((p[0] - a[0]) * ez - (p[1] - a[1]) * ex) / det;
            let v = (dx * (p[1] - a[1]) - dz * (p[0] - a[0])) / det;
            let w = [1. - u - v, u, v];
            if w.iter().all(|v| *v >= -1e-9) {
                let w = w.map(|v| v.max(0.));
                let sum = w.iter().sum::<f64>();
                return Some((id, w.map(|v| v / sum)));
            }
        }
        None
    }
    pub fn context(&self, id: u32, w: [f64; 3]) -> Context {
        let n = self.triangles[id as usize].map(|i| self.nodes[i as usize]);
        let sum = |f: fn(Node) -> f64| (0..3).map(|i| w[i] * f(n[i])).sum();
        Context {
            temperature: sum(|n| n.climate.mean as f64),
            warm_temperature: sum(|n| n.climate.warm as f64),
            moisture: sum(|n| n.climate.moisture as f64),
            sediment_m: sum(|n| n.sediment as f64),
            drainage_m2: sum(|n| n.discharge as f64 * 31_557_600. / 0.3),
            gradient: [sum(|n| n.gradient[0] as f64), sum(|n| n.gradient[1] as f64)],
            water_depth_m: 0.,
        }
    }
    pub fn memory_bytes(&self) -> usize {
        self.nodes.len() * std::mem::size_of::<Node>()
            + self.triangles.len() * 12
            + self.offsets.len() * 4
            + self.bins.len() * 4
            + self.channel_offsets.len() * 4
            + self.channel_bins.len() * 4
            + self.river_profiles.len() * 17 * 4
    }
}

#[cfg(test)]
mod tests {
    use super::super::refine::Channel;
    use super::*;

    fn fixture() -> Data {
        let mut nodes: Vec<_> = (0..9)
            .map(|i| Node {
                p: [(i % 3) as f64 * 10., (i / 3) as f64 * 10.],
                height: 30. - (i / 3) as f64 * 8.,
                water: 30. - (i / 3) as f64 * 8.,
                climate: Cell::default(),
                sediment: 0.,
                discharge: 0.,
                receiver: i,
                watershed: 0,
                gradient: [0.; 2],
                river_bend: 0.,
                river_profile: u32::MAX,
                river_level: 0.,
            })
            .collect();
        for (up, down, flow) in [(1, 4, 25.), (3, 4, 9.), (4, 7, 30.)] {
            nodes[up].receiver = down as u32;
            nodes[up].discharge = flow;
        }
        // A wet corner must not flood the opposite shared edge.
        nodes[0].water += 4.;
        let mut triangles = Vec::new();
        for z in 0..2 {
            for x in 0..2 {
                let a = z * 3 + x;
                triangles.push([a, a + 1, a + 4]);
                triangles.push([a, a + 4, a + 3]);
            }
        }
        let mut d = Data {
            seed: 42,
            extent: 20.,
            side: 3,
            nodes,
            triangles,
            offsets: vec![],
            bins: vec![],
            channel_offsets: vec![],
            channel_bins: vec![],
            river_profiles: vec![],
            fingerprint: 0,
            low: f64::INFINITY,
            high: f64::NEG_INFINITY,
        };
        d.index().unwrap();
        d
    }

    #[test]
    fn shared_edges_and_confluences_match_across_every_incident_patch() {
        let data = fixture();
        for (ia, ta) in data.triangles.iter().enumerate() {
            for (ib, tb) in data.triangles.iter().enumerate().skip(ia + 1) {
                let shared: Vec<_> = ta.iter().copied().filter(|id| tb.contains(id)).collect();
                if shared.len() != 2 {
                    continue;
                }
                let a = Patch::build(&data, ia as u32);
                let b = Patch::build(&data, ib as u32);
                for t in [0., 1e-6, 0.1, 0.5, 0.9, 1. - 1e-6, 1.] {
                    let mut wa = [0.; 3];
                    let mut wb = [0.; 3];
                    for (id, weight) in [(shared[0], 1. - t), (shared[1], t)] {
                        wa[ta.iter().position(|i| *i == id).unwrap()] = weight;
                        wb[tb.iter().position(|i| *i == id).unwrap()] = weight;
                    }
                    let p = std::array::from_fn(|k| {
                        (0..3)
                            .map(|i| wa[i] * data.nodes[ta[i] as usize].p[k])
                            .sum()
                    });
                    let sa = a.sample(p, wa, 42, 0.001);
                    let sb = b.sample(p, wb, 42, 0.001);
                    assert!((sa.height - sb.height).abs() < 1e-9, "edge {ia}/{ib} t={t}");
                    assert!((sa.water - sb.water).abs() < 1e-9, "water {ia}/{ib} t={t}");
                }
            }
        }
    }

    #[test]
    fn overlapping_channels_do_not_depend_on_iteration_order() {
        let data = fixture();
        for id in 0..data.triangles.len() {
            let a = Patch::build(&data, id as u32);
            let mut b = a.clone();
            b.channels.reverse();
            for w in [[0.2, 0.3, 0.5], [0.8, 0.1, 0.1], [1., 0., 0.]] {
                let p = std::array::from_fn(|k| (0..3).map(|i| w[i] * a.points[i][k]).sum());
                let sa = a.sample(p, w, 42, 0.001);
                let sb = b.sample(p, w, 42, 0.001);
                assert_eq!(sa.height, sb.height);
                assert_eq!(sa.water, sb.water);
            }
        }
    }

    #[test]
    fn local_channel_water_and_carving_vanish_at_patch_boundary() {
        let data = fixture();
        let mut patch = Patch::build(&data, 0);
        patch.channels = vec![Channel {
            bend: 0.,
            profile: [0.; 17],
            a: [10., 50., 0.],
            b: [5., 50., 5.],
            width: 8.,
            flow: 1.,
            local: true,
            compiled: false,
        }];
        patch.wet_depth = [0.; 3];
        let p = [10., 0.];
        let w = [0., 1., 0.];
        let sampled = patch.sample(p, w, 42, 10.);
        assert_eq!(sampled.height, patch.height(w));
        assert_eq!(sampled.water, sampled.height);
    }

    #[test]
    fn channel_index_includes_nonincident_confluence_branches() {
        let data = fixture();
        let channels = data.channels_in_rect([10., 10.], [10., 10.]);
        assert_eq!(channels.len(), 3);
        // An incoming branch not on triangle 2 still reaches its central corner.
        let patch = Patch::build(&data, 2);
        assert_eq!(patch.channels.iter().filter(|s| !s.local).count(), 3);
    }
    #[test]
    fn curved_receiver_has_native_water_through_patch_crossings_and_confluence() {
        let mut data = fixture();
        for n in &mut data.nodes {
            n.p = n.p.map(|v| v * 100.);
            n.height = 100. - n.p[1] * 0.01;
            n.water = n.height;
        }
        data.extent = 2000.;
        data.index().unwrap();
        let channels = data.channels_in_rect([0., 0.], [2000., 2000.]);
        let branch = channels.iter().find(|s| s.a[0] == 0.).unwrap();
        assert!(branch.bend.abs() > 50., "river must leave its atlas edge");
        assert!(branch.width >= 10. && branch.depth() >= 3.);
        let spacing = 0.625; // 6.25cm native voxels at uniform world scale ten.
        let mut sectors = std::collections::BTreeSet::new();
        for channel in &channels {
            let steps = ((channel.b[0] - channel.a[0]).hypot(channel.b[2] - channel.a[2])
                / (spacing * 0.5))
                .ceil() as usize;
            let mut previous = None;
            for i in 0..=steps {
                let q = channel.position(i as f64 / steps as f64);
                // Test actual quantized occupied columns, rather than nominal
                // float depth. Consecutive samples visit touching native cells.
                let cell = [
                    (q[0] / spacing).floor() as i64,
                    (q[2] / spacing).floor() as i64,
                ];
                let p = cell.map(|v| (v as f64 + 0.5) * spacing);
                let Some((id, w)) = data.locate(p) else {
                    continue;
                };
                let patch = Patch::build(&data, id);
                assert!(patch.channels.iter().all(|s| !s.local));
                let surface = patch.sample(p, w, 42, spacing);
                assert!(
                    (surface.water / spacing).floor() > (surface.height / spacing).floor(),
                    "dry native river cell {cell:?} triangle {id}"
                );
                let mut bank = patch.clone();
                bank.channels.clear();
                assert!(surface.water <= bank.sample(p, w, 42, spacing).height + 1e-8);
                sectors.insert(id);
                if let Some(last) = previous {
                    let last: [i64; 2] = last;
                    assert!((last[0] - cell[0]).abs() <= 1 && (last[1] - cell[1]).abs() <= 1);
                    if last[0] != cell[0] && last[1] != cell[1] {
                        let bridge = [(last[0] as f64 + 0.5) * spacing, p[1]];
                        let (bid, bw) = data.locate(bridge).unwrap();
                        let wet = Patch::build(&data, bid).sample(bridge, bw, 42, spacing);
                        assert!(
                            (wet.water / spacing).floor() > (wet.height / spacing).floor(),
                            "diagonal river cells need a face-connected water bridge"
                        );
                    }
                }
                previous = Some(cell);
            }
        }
        assert!(
            sectors.len() >= 3,
            "river must traverse several canonical patches"
        );
        // A curve's widened bank bounds must participate even outside its chord
        // rectangle; this catches stale endpoint-only feature indexing.
        let q = branch.position(0.5);
        assert!(data
            .channels_in_rect([q[0], q[2]], [q[0], q[2]])
            .iter()
            .any(|s| s.a == branch.a && s.b == branch.b));
        let [lo, hi] = branch.bounds();
        let protection_only = [(lo[0] + hi[0]) * 0.5, hi[1] + 80.];
        assert!(protection_only[1] > hi[1]);
        assert!(
            data.channels_in_rect(protection_only, protection_only)
                .iter()
                .any(|s| s.a == branch.a && s.b == branch.b),
            "shared protection halo needs index membership beyond carve bounds"
        );
    }

    #[test]
    fn compiled_profiles_are_monotone_and_share_exact_junction_levels() {
        let data = fixture();
        let channels = data.channels_in_rect([0., 0.], [20., 20.]);
        for channel in &channels {
            let mut previous = channel.position(0.)[1];
            for i in 1..=256 {
                let level = channel.position(i as f64 / 256.)[1];
                assert!(level <= previous, "river profile rises downstream");
                previous = level;
            }
            for next in &channels {
                if [channel.b[0], channel.b[2]] == [next.a[0], next.a[2]] {
                    assert_eq!(channel.position(1.)[1], next.position(0.)[1]);
                }
            }
        }
        for n in &data.nodes {
            assert!(n.river_level >= n.water - 20.);
            if n.water > n.height + 0.01 {
                assert_eq!(n.river_level, n.water);
            }
        }
    }

    #[test]
    fn locate_normalizes_barycentrics_after_edge_tolerance_clamp() {
        let data = fixture();
        for p in [[10., 10. + 1e-10], [10. + 1e-10, 10.], [0., 0.], [20., 20.]] {
            let (_, w) = data.locate(p).unwrap();
            assert!(w.iter().all(|v| *v >= 0.));
            assert!((w.iter().sum::<f64>() - 1.).abs() < 1e-15);
        }
    }

    #[test]
    fn river_profiles_above_uncarved_ground_cannot_create_water_ribbons() {
        let data = fixture();
        let mut patch = Patch::build(&data, 0);
        patch.controls = [10.; 10];
        patch.wet_depth = [0.; 3];
        for local in [false, true] {
            for level in [9., 10.1, 11., 50.] {
                patch.channels = vec![Channel {
                    bend: 0.,
                    profile: [0.; 17],
                    a: [0., level, 5.],
                    b: [20., level, 5.],
                    width: 2.,
                    flow: 1.,
                    local,
                    compiled: false,
                }];
                let s = patch.sample([10., 5.], [1. / 3.; 3], 42, 10.);
                assert!(s.water <= 10.);
                assert!(s.water >= s.height);
                if level > 10.25 {
                    assert!((s.height - 10.).abs() < 1e-12);
                    assert!((s.water - 10.).abs() < 1e-12);
                } else {
                    assert!(s.height < s.water);
                }
            }
        }
    }

    #[test]
    fn a_lake_stays_level_against_dry_banks() {
        let mut data = fixture();
        for (i, n) in data.nodes.iter_mut().enumerate() {
            n.height = 10.;
            n.water = 10.;
            n.discharge = 0.;
            n.receiver = i as u32;
        }
        for i in [0, 1] {
            data.nodes[i].height = -5.;
            data.nodes[i].water = 0.;
        }
        data.index().unwrap();
        let patch = Patch::build(&data, 0);
        let mut wet_samples = 0;
        for dry_weight in [0., 0.1, 0.2, 0.3, 0.4, 0.5, 0.8, 1.] {
            let w = [(1. - dry_weight) / 2., (1. - dry_weight) / 2., dry_weight];
            let p = std::array::from_fn(|k| (0..3).map(|i| w[i] * patch.points[i][k]).sum());
            let sample = patch.sample(p, w, 42, 10.);
            if sample.height < 0. {
                assert!(sample.water.abs() < 1e-12);
                wet_samples += 1;
            } else {
                assert_eq!(sample.water, sample.height);
            }
        }
        assert!(wet_samples >= 3);
    }

    #[test]
    #[ignore = "loads existing million-node saved atlas; no generation"]
    fn saved_atlas_random_edges_and_node_1237_confluence_are_continuous() {
        let path = std::env::var_os("ATLAS_TEST_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../output/atlas-climate/seed42")
            });
        let data = Data::load(&path).unwrap();
        let incident: Vec<_> = data
            .triangles
            .iter()
            .enumerate()
            .filter(|(_, t)| t.contains(&1237))
            .map(|(id, _)| id as u32)
            .collect();
        let mut surfaces = incident.iter().map(|&id| {
            let ids = data.triangles[id as usize];
            let w = ids.map(|i| if i == 1237 { 1. } else { 0. });
            Patch::build(&data, id).sample(data.nodes[1237].p, w, 42, 0.001)
        });
        let first = surfaces.next().unwrap();
        for surface in surfaces {
            assert!((first.height - surface.height).abs() < 1e-8);
            assert!((first.water - surface.water).abs() < 1e-8);
        }
        let mut checked = 0;
        for k in 0..64u64 {
            let id = crate::painting::hash(k + 4242) as usize % data.triangles.len();
            let ids = data.triangles[id];
            let a = Patch::build(&data, id as u32);
            for (ia, ib) in [(0, 1), (1, 2), (2, 0)] {
                let na = data.nodes[ids[ia] as usize];
                let nb = data.nodes[ids[ib] as usize];
                let mid = std::array::from_fn(|j| (na.p[j] + nb.p[j]) * 0.5);
                let near =
                    data.triangle_ids_in_rect(mid.map(|v| v - 0.001), mid.map(|v| v + 0.001));
                for other in near.into_iter().filter(|&v| v as usize != id) {
                    let tid = data.triangles[other as usize];
                    if !tid.contains(&ids[ia]) || !tid.contains(&ids[ib]) {
                        continue;
                    }
                    let b = Patch::build(&data, other);
                    for t in [0., 0.01, 0.25, 0.5, 0.75, 0.99, 1.] {
                        let mut wa = [0.; 3];
                        wa[ia] = 1. - t;
                        wa[ib] = t;
                        let wb = tid.map(|v| {
                            if v == ids[ia] {
                                1. - t
                            } else if v == ids[ib] {
                                t
                            } else {
                                0.
                            }
                        });
                        let p = std::array::from_fn(|j| na.p[j] * (1. - t) + nb.p[j] * t);
                        let sa = a.sample(p, wa, 42, 0.001);
                        let sb = b.sample(p, wb, 42, 0.001);
                        assert!(
                            (sa.height - sb.height).abs() < 1e-7,
                            "triangles {id}/{other}"
                        );
                        assert!((sa.water - sb.water).abs() < 1e-7, "water {id}/{other}");
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked >= 100);
        eprintln!(
            "checked {checked} saved-atlas shared edges and {} confluence sectors; data bytes {}",
            incident.len(),
            data.memory_bytes()
        );
    }
    #[test]
    #[ignore = "loads existing million-node saved atlas; no generation"]
    fn saved_valley_curves_have_connected_native_wet_columns() {
        let path = std::env::var_os("ATLAS_TEST_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../output/atlas-climate/seed42")
            });
        let data = Data::load(&path).unwrap();
        let channels = data.channels_in_rect([967_000., 474_000.], [977_000., 484_000.]);
        let spacing = 0.625;
        let mut tested = 0;
        let mut curved = 0;
        let mut meaningful = 0;
        let total_reaches = channels.len();
        let mut max_profile_bank_excess = 0f64;
        let mut patches = std::collections::BTreeMap::new();
        for channel in channels {
            curved += usize::from(channel.bend.abs() > 0.);
            meaningful += usize::from(
                channel.bend.abs()
                    >= 0.05 * (channel.b[0] - channel.a[0]).hypot(channel.b[2] - channel.a[2]),
            );
            let mut prior_level = channel.a[1];
            let mut prior_actual = f64::INFINITY;
            for j in 0..=256 {
                let q = channel.position(j as f64 / 256.);
                assert!(q[1] <= prior_level + 1e-10);
                prior_level = q[1];
                if let Some((id, w)) = data.locate([q[0], q[2]]) {
                    let bank = Patch::bank(&data, id);
                    let actual = Patch::build(&data, id)
                        .sample([q[0], q[2]], w, data.seed, 0.001)
                        .water;
                    assert!(
                        actual <= prior_actual + 1e-7,
                        "actual river water rises near {:?}: {} -> {}, profile {}, bend {}",
                        [q[0], q[2]],
                        prior_actual,
                        actual,
                        q[1],
                        channel.bend
                    );
                    prior_actual = actual;
                    if bank.wet_depth.iter().all(|d| *d == 0.) {
                        max_profile_bank_excess =
                            max_profile_bank_excess.max(q[1] - bank.height(w));
                    }
                }
            }
            let steps = ((channel.b[0] - channel.a[0]).hypot(channel.b[2] - channel.a[2])
                / (spacing * 0.5))
                .ceil() as usize;
            let mut frontier: Vec<([i64; 2], i64, i64)> = Vec::new();
            for i in 0..=steps {
                let q = channel.position(i as f64 / steps as f64);
                let center = [
                    (q[0] / spacing).floor() as i64,
                    (q[2] / spacing).floor() as i64,
                ];
                let mut next = Vec::new();
                // Follow integer occupied cells across the full local river
                // bed. A sloping centerline may round onto its dry bank.
                let mut candidates = Vec::new();
                for dz in -2..=2 {
                    for dx in -2..=2 {
                        let cell = [center[0] + dx, center[1] + dz];
                        let p = cell.map(|v| (v as f64 + 0.5) * spacing);
                        let Some((id, w)) = data.locate(p) else {
                            continue;
                        };
                        let patch = patches.entry(id).or_insert_with(|| Patch::build(&data, id));
                        let surface = patch.sample(p, w, data.seed, spacing);
                        let low = (surface.height / spacing).floor() as i64;
                        let high = (surface.water / spacing).floor() as i64;
                        if high > low {
                            candidates.push((cell, low, high));
                        }
                    }
                }
                if i == 0 {
                    next = candidates.clone();
                }
                loop {
                    let mut changed = false;
                    for &(cell, low, high) in &candidates {
                        if next.iter().any(|&(c, _, _)| c == cell) {
                            continue;
                        }
                        if frontier.iter().chain(next.iter()).any(|&(last, lo, hi)| {
                            (last[0] - cell[0]).abs() + (last[1] - cell[1]).abs() <= 1
                                && high.min(hi) > low.max(lo)
                        }) {
                            next.push((cell, low, high));
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }
                assert!(
                    !next.is_empty(),
                    "disconnected native river near {q:?}, bend {}, t {}",
                    channel.bend,
                    i as f64 / steps as f64
                );
                frontier = next;
                tested += 1;
            }
        }
        eprintln!(
            "saved valley: {tested} face-connected native river steps, {curved} curved reaches"
        );
        eprintln!("curvature: {meaningful}/{total_reaches} reaches have midpoint bend >=5% chord; max dry-bank profile excess {max_profile_bank_excess:.4}m; data bytes {}",data.memory_bytes());
        assert!(
            meaningful * 2 >= total_reaches,
            "most river reaches must retain visible curvature"
        );
        assert!(
            max_profile_bank_excess < 0.5,
            "bank clipping must only be residual profile safety"
        );
        assert!(tested > 1000);
    }

    #[test]
    #[ignore = "loads existing million-node saved atlas; no generation"]
    fn saved_atlas_river_levels_stay_within_supported_banks() {
        let path = std::env::var_os("ATLAS_TEST_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../output/atlas-climate/seed42")
            });
        let data = Data::load(&path).unwrap();
        let channels = data.channels_in_rect([967_000., 474_000.], [977_000., 484_000.]);
        let mut max_excess = 0f64;
        let mut max_profile_excess = 0f64;
        let mut count = 0;
        for s in channels {
            for i in 0..=64 {
                let t = i as f64 / 64.;
                let p = [
                    s.a[0] * (1. - t) + s.b[0] * t,
                    s.a[2] * (1. - t) + s.b[2] * t,
                ];
                let dx = s.b[0] - s.a[0];
                let dz = s.b[2] - s.a[2];
                let length = dx.hypot(dz);
                for across in [-0.99, -0.5, 0., 0.5, 0.99] {
                    let offset = across * s.width * 0.5;
                    let p = [p[0] - dz / length * offset, p[1] + dx / length * offset];
                    let Some((id, w)) = data.locate(p) else {
                        continue;
                    };
                    let patch = Patch::build(&data, id);
                    if patch.wet_depth.iter().any(|d| *d > 0.) {
                        continue;
                    }
                    let base = patch.height(w);
                    let surface = patch.sample(p, w, 42, 10.);
                    max_excess = max_excess.max(surface.water - base);
                    max_profile_excess =
                        max_profile_excess.max(s.a[1] * (1. - t) + s.b[1] * t - base);
                    count += 1;
                }
            }
        }
        eprintln!("river bank check {count} dry samples; max raw profile/base excess {max_profile_excess:.6}m; max final water/base excess {max_excess:.6}m");
        assert!(count > 1000);
        assert!(max_excess < 1e-8);
    }
}
