//! Preserved terrain mathematics. Only the discrete generator adapter exposes it.
use std::{cmp::Reverse, collections::BinaryHeap};
const REGION_SIZE: f64 = 10000.0;
const REGIONAL_SPACING: f64 = 16.0;
const HEIGHT_SCALE: f64 = 1024.0;
const EROSION_STEPS: usize = 24;
const CELL_AREA: f64 = REGIONAL_SPACING * REGIONAL_SPACING;
const RIVER_THRESHOLD: f64 = 100_000.0; // Rainfall-weighted catchment area, m².
const BIN_SIZE: f64 = 64.0;
const ROCK: u8 = 0;
const SOIL: u8 = 1;
const SAND: u8 = 2;
const GRAVEL: u8 = 3;
const NEIGHBORS: [(isize, isize); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

// Regional fields are dense, but contain only 626 × 626 samples for 100 km².
pub(crate) struct Region {
    side: usize,
    heights: Vec<f64>,
    bedrock: Vec<f64>,
    soil: Vec<f64>,
    resistance: Vec<f64>,
    rainfall: Vec<f64>,
    erosion: Vec<f64>,
    lakes: Vec<f64>,
    flow: Vec<f64>,
    channels: Vec<Channel>,
    bins: Vec<Vec<usize>>,
    budget: SedimentBudget,
}

#[derive(Default)]
struct SedimentBudget {
    eroded: f64,
    deposited: f64,
    exported: f64,
}

struct Drainage {
    receiver: Vec<usize>,
    order: Vec<usize>, // Downstream first; reversing gives upstream first.
    distance: Vec<f64>,
    spill: Vec<f64>, // Virtual fill surface: terrain itself is never filled.
    area: Vec<f64>,
}

struct Channel {
    start: [f64; 2],
    end: [f64; 2],
    level: [f64; 2],
    half_width: f64,
    depth: f64,
    bend: f64,
}

pub(crate) struct Surface {
    pub(crate) height: f64,
    pub(crate) water: f64,
    pub(crate) bedrock: f64,
    pub(crate) material: u8,
}

// ---- Smooth deterministic noise. Distances throughout this file are meters. ----

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + t * (b - a)
}
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
fn smoothstep(a: f64, b: f64, value: f64) -> f64 {
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn gradient_dot(x: i64, z: i64, dx: f64, dz: f64, seed: u64) -> f64 {
    let hash = mix(seed ^ mix(x as u64) ^ mix(z as u64).rotate_left(32));
    let d = std::f64::consts::FRAC_1_SQRT_2;
    match hash & 7 {
        0 => dx,
        1 => -dx,
        2 => dz,
        3 => -dz,
        4 => (dx + dz) * d,
        5 => (dx - dz) * d,
        6 => (-dx + dz) * d,
        _ => (-dx - dz) * d,
    }
}

fn perlin(x: f64, z: f64, seed: u64) -> f64 {
    let gx = x.floor() as i64;
    let gz = z.floor() as i64;
    let dx = x - x.floor();
    let dz = z - z.floor();
    let top = lerp(
        gradient_dot(gx, gz, dx, dz, seed),
        gradient_dot(gx + 1, gz, dx - 1.0, dz, seed),
        fade(dx),
    );
    let bottom = lerp(
        gradient_dot(gx, gz + 1, dx, dz - 1.0, seed),
        gradient_dot(gx + 1, gz + 1, dx - 1.0, dz - 1.0, seed),
        fade(dx),
    );
    lerp(top, bottom, fade(dz)) * std::f64::consts::SQRT_2
}

fn noise(x: f64, z: f64, wavelength: f64, seed: u64) -> f64 {
    perlin(x / wavelength, z / wavelength, seed)
}

// Rotated, offset octaves avoid lining up every layer's lattice corners.
fn octave_position(x: f64, z: f64) -> (f64, f64) {
    (0.8 * x - 0.6 * z + 19.19, 0.6 * x + 0.8 * z + 73.73)
}

// ---- Regional structure: plains, foothills, ridges, and coordinate warping. ----

fn mountain_mask(x: f64, z: f64, seed: u64) -> f64 {
    let broad = noise(x + 731.0, z + 271.0, 1800.0, seed ^ 11);
    smoothstep(-0.22, 0.35, broad)
}

fn warped_position(x: f64, z: f64, seed: u64) -> (f64, f64) {
    (
        x + 120.0 * noise(x, z, 900.0, seed ^ 21),
        z + 120.0 * noise(x + 513.0, z - 291.0, 900.0, seed ^ 22),
    )
}

fn ridged_mountains(mut x: f64, mut z: f64, seed: u64) -> f64 {
    let mut wavelength = 700.0;
    let mut amplitude = 1.0;
    let mut weight = 1.0;
    let mut sum = 0.0;
    let mut normalization = 0.0;
    for octave in 0..5 {
        let ridge = (1.0 - noise(x, z, wavelength, seed ^ (100 + octave)).abs()).max(0.0);
        let signal = ridge * ridge * weight;
        sum += signal * amplitude;
        normalization += amplitude;
        weight = (signal * 2.0).min(1.0);
        amplitude *= 0.48;
        wavelength *= 0.5;
        (x, z) = octave_position(x, z);
    }
    sum / normalization
}

fn base_height(x: f64, z: f64, seed: u64) -> f64 {
    let (wx, wz) = warped_position(x, z, seed);
    let mountains = mountain_mask(wx, wz, seed);
    let plains = 38.0 + 22.0 * noise(wx, wz, 1200.0, seed ^ 31);
    let foothills = 30.0 * noise(wx, wz, 280.0, seed ^ 32);
    plains + mountains * (70.0 + 650.0 * ridged_mountains(wx, wz, seed) + foothills)
}

// ---- Regional geology and hydrology. No fluid simulation per microvoxel. ----

impl Region {
    fn from_heights(side: usize, heights: Vec<f64>) -> Self {
        let count = heights.len();
        let soil: Vec<f64> = heights.iter().map(|h| h.min(3.0)).collect();
        let bedrock = heights.iter().zip(&soil).map(|(h, s)| h - s).collect();
        Self {
            side,
            heights,
            bedrock,
            soil,
            resistance: vec![0.15; count],
            rainfall: vec![1.0; count],
            erosion: vec![0.0; count],
            lakes: vec![0.0; count],
            flow: vec![0.0; count],
            channels: Vec::new(),
            bins: Vec::new(),
            budget: SedimentBudget::default(),
        }
    }

    pub(crate) fn generate(seed: u64) -> Self {
        let side = (REGION_SIZE / REGIONAL_SPACING) as usize + 1;
        let original: Vec<f64> = (0..side * side)
            .map(|i| {
                base_height(
                    (i % side) as f64 * REGIONAL_SPACING,
                    (i / side) as f64 * REGIONAL_SPACING,
                    seed,
                )
            })
            .collect();
        let mut region = Self::from_heights(side, original.clone());
        for i in 0..region.heights.len() {
            let (x, z) = region.position(i);
            region.soil[i] = 1.5 + 4.0 * (1.0 - mountain_mask(x, z, seed));
            region.bedrock[i] = region.heights[i] - region.soil[i];
            // Higher resistance values here mean a larger fraction can erode.
            region.resistance[i] = 0.06 + 0.14 * (0.5 + 0.5 * noise(x, z, 700.0, seed ^ 401));
            region.rainfall[i] = 1.0 + 0.35 * noise(x, z, 2200.0, seed ^ 402);
        }
        for _ in 0..EROSION_STEPS {
            let drainage = region.drainage_tree(seed);
            region.transport_sediment(&drainage);
            region.relax_slopes();
        }
        let drainage = region.drainage_tree(seed);
        region.preserve_lakes(&drainage);
        region.build_channels(&drainage, seed);
        region.flow = drainage.area;
        region.erosion = region
            .heights
            .iter()
            .zip(original)
            .map(|(new, old)| new - old)
            .collect();
        region
    }

    fn position(&self, i: usize) -> (f64, f64) {
        (
            (i % self.side) as f64 * REGIONAL_SPACING,
            (i / self.side) as f64 * REGIONAL_SPACING,
        )
    }

    fn neighbors(&self, i: usize) -> impl Iterator<Item = (usize, f64)> + '_ {
        let x = (i % self.side) as isize;
        let z = (i / self.side) as isize;
        NEIGHBORS.into_iter().filter_map(move |(dx, dz)| {
            let nx = x + dx;
            let nz = z + dz;
            if nx < 0 || nz < 0 || nx >= self.side as isize || nz >= self.side as isize {
                return None;
            }
            let length = if dx != 0 && dz != 0 {
                std::f64::consts::SQRT_2
            } else {
                1.0
            };
            Some((
                nz as usize * self.side + nx as usize,
                length * REGIONAL_SPACING,
            ))
        })
    }

    fn is_boundary(&self, i: usize) -> bool {
        let x = i % self.side;
        let z = i / self.side;
        x == 0 || z == 0 || x + 1 == self.side || z + 1 == self.side
    }

    fn drainage_tree(&self, seed: u64) -> Drainage {
        let count = self.heights.len();
        let mut receiver: Vec<usize> = (0..count).collect();
        let mut distance = vec![REGIONAL_SPACING; count];
        let mut visited = vec![false; count];
        let mut spill = self.heights.clone();
        let mut rank = vec![0; count];
        let mut queue = BinaryHeap::new();
        for i in 0..count {
            if self.is_boundary(i) {
                visited[i] = true;
                queue.push(Reverse((spill[i].to_bits(), i)));
            }
        }
        let mut order = Vec::with_capacity(count);
        while let Some(Reverse((_, i))) = queue.pop() {
            rank[i] = order.len();
            order.push(i);
            for (next, length) in self.neighbors(i) {
                if visited[next] {
                    continue;
                }
                visited[next] = true;
                receiver[next] = i;
                distance[next] = length;
                // This is a routing surface, not deposited material or ground.
                spill[next] = spill[next].max(spill[i] + 0.0000001);
                queue.push(Reverse((spill[next].to_bits(), next)));
            }
        }
        // Prefer downhill directions by slope rather than the flood discovery
        // direction. Tiny seed-based tie breaking and diagonal slope relaxation
        // reduce grid alignment; a regular D8 grid still has directional bias.
        for &i in &order {
            if self.is_boundary(i) {
                continue;
            }
            let mut best = 0.0;
            for (next, length) in self.neighbors(i) {
                if rank[next] >= rank[i] {
                    continue;
                }
                let tie = (mix(seed ^ i as u64 ^ (next as u64).rotate_left(17)) >> 11) as f64
                    / ((1u64 << 53) as f64);
                let slope = ((self.heights[i] - self.heights[next]) / length).max(0.0);
                let score = slope * (1.0 + tie * 0.015);
                if score > best {
                    best = score;
                    receiver[i] = next;
                    distance[i] = length;
                }
            }
        }
        let mut area: Vec<f64> = self.rainfall.iter().map(|rain| CELL_AREA * rain).collect();
        for &i in order.iter().rev() {
            if receiver[i] != i {
                area[receiver[i]] += area[i];
            }
        }
        Drainage {
            receiver,
            order,
            distance,
            spill,
            area,
        }
    }

    // Sediment is measured in m³. Every eroded unit is deposited or explicitly
    // exported at an open boundary. Water flow is steady-state, not simulated.
    fn transport_sediment(&mut self, drainage: &Drainage) {
        let mut load = vec![0.0; self.heights.len()];
        for &i in drainage.order.iter().rev() {
            let next = drainage.receiver[i];
            if next == i {
                self.budget.exported += load[i];
                continue;
            }
            let slope = ((self.heights[i] - self.heights[next]) / drainage.distance[i]).max(0.0);
            let submerged = drainage.spill[i] - self.heights[i] > 0.75;
            let capacity = if submerged {
                0.0
            } else {
                CELL_AREA * 0.70 * (drainage.area[i] / CELL_AREA).sqrt() * slope.sqrt()
            };
            if load[i] > capacity {
                let fraction = if submerged { 0.95 } else { 0.45 };
                let deposit = ((load[i] - capacity) * fraction).min(4.0 * CELL_AREA);
                self.soil[i] += deposit / CELL_AREA;
                self.heights[i] += deposit / CELL_AREA;
                load[i] -= deposit;
                self.budget.deposited += deposit;
            } else {
                let demand = ((capacity - load[i]) * 0.45 / CELL_AREA).min(4.0);
                let loose = demand.min(self.soil[i]);
                let rock = ((demand - loose) * self.resistance[i]).min(self.bedrock[i].max(0.0));
                self.soil[i] -= loose;
                self.bedrock[i] -= rock;
                let removed = loose + rock;
                self.heights[i] -= removed;
                load[i] += removed * CELL_AREA;
                self.budget.eroded += removed * CELL_AREA;
            }
            load[next] += load[i];
        }
    }

    fn relax_slopes(&mut self) {
        let mut changes = vec![0.0; self.heights.len()];
        for i in 0..self.heights.len() {
            for (j, length) in self.neighbors(i) {
                if j <= i {
                    continue;
                } // Each undirected edge exactly once.
                let difference = self.heights[i] - self.heights[j];
                let (source, target) = if difference > 0.0 { (i, j) } else { (j, i) };
                let excess = (difference.abs() - 0.75 * length).max(0.0);
                // Each cell has <=8 neighbors, so simultaneous transfers cannot
                // exhaust more than its available soil. Bedrock stays exposed.
                let transfer = (excess * 0.10).min(self.soil[source] / 8.0);
                changes[source] -= transfer;
                changes[target] += transfer;
            }
        }
        for (i, change) in changes.into_iter().enumerate() {
            self.soil[i] += change;
            self.heights[i] = self.bedrock[i] + self.soil[i];
        }
    }

    fn preserve_lakes(&mut self, drainage: &Drainage) {
        let mut visited = vec![false; self.heights.len()];
        for start in 0..self.heights.len() {
            if visited[start] || drainage.spill[start] - self.heights[start] < 0.75 {
                continue;
            }
            let level = drainage.spill[start];
            let mut cells = vec![start];
            visited[start] = true;
            let mut cursor = 0;
            while cursor < cells.len() {
                let i = cells[cursor];
                cursor += 1;
                for (j, _) in self.neighbors(i) {
                    if !visited[j]
                        && drainage.spill[j] - self.heights[j] >= 0.75
                        && (drainage.spill[j] - level).abs() < 0.01
                    {
                        visited[j] = true;
                        cells.push(j);
                    }
                }
            }
            // Exclude tiny pits; selected basins are full to their spillway.
            if cells.len() >= 4 {
                for i in cells {
                    self.lakes[i] = level;
                }
            }
        }
    }

    fn build_channels(&mut self, drainage: &Drainage, seed: u64) {
        let mut level = vec![0.0; self.heights.len()];
        for &i in &drainage.order {
            let next = drainage.receiver[i];
            level[i] = if self.lakes[i] > 0.0 {
                self.lakes[i]
            } else {
                (self.heights[i] - 0.15).max(if next == i { 0.0 } else { level[next] })
            };
        }
        for &i in &drainage.order {
            let next = drainage.receiver[i];
            if next == i || drainage.area[i] < RIVER_THRESHOLD {
                continue;
            }
            let (x, z) = self.position(i);
            let (nx, nz) = self.position(next);
            let width = (1.5 + 2.3 * (drainage.area[i] / RIVER_THRESHOLD).powf(0.35)).min(40.0);
            let bend = ((mix(seed ^ i as u64) >> 11) as f64 / (1u64 << 53) as f64 - 0.5)
                * drainage.distance[i]
                * 0.12;
            self.channels.push(Channel {
                start: [x, z],
                end: [nx, nz],
                level: [level[i], level[next]],
                half_width: width * 0.5,
                depth: (0.3 + width * 0.12).min(5.0),
                bend,
            });
        }
        let side = (REGION_SIZE / BIN_SIZE).ceil() as usize + 1;
        self.bins = vec![Vec::new(); side * side];
        for (index, channel) in self.channels.iter().enumerate() {
            let radius = channel.half_width + 12.0 + channel.bend.abs();
            let bounds = |axis: usize| {
                let low = ((channel.start[axis].min(channel.end[axis]) - radius) / BIN_SIZE)
                    .floor()
                    .max(0.0) as usize;
                let high = ((channel.start[axis].max(channel.end[axis]) + radius) / BIN_SIZE)
                    .ceil()
                    .min((side - 1) as f64) as usize;
                (low, high)
            };
            let (x0, x1) = bounds(0);
            let (z0, z1) = bounds(1);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    self.bins[z * side + x].push(index);
                }
            }
        }
    }

    // Interpolation of scalar regional fields; coordinates remain in meters.
    fn field_at(&self, field: &[f64], x: f64, z: f64) -> f64 {
        let (indices, weights) = self.interpolation(x, z);
        indices
            .iter()
            .zip(weights)
            .map(|(&i, w)| field[i] * w)
            .sum()
    }

    fn interpolation(&self, x: f64, z: f64) -> ([usize; 4], [f64; 4]) {
        let gx = (x / REGIONAL_SPACING).clamp(0.0, (self.side - 1) as f64);
        let gz = (z / REGIONAL_SPACING).clamp(0.0, (self.side - 1) as f64);
        let ix = (gx.floor() as usize).min(self.side - 2);
        let iz = (gz.floor() as usize).min(self.side - 2);
        let tx = gx - ix as f64;
        let tz = gz - iz as f64;
        let i = iz * self.side + ix;
        (
            [i, i + 1, i + self.side, i + self.side + 1],
            [
                (1.0 - tx) * (1.0 - tz),
                tx * (1.0 - tz),
                (1.0 - tx) * tz,
                tx * tz,
            ],
        )
    }

    fn lake_at(&self, x: f64, z: f64) -> f64 {
        let (indices, weights) = self.interpolation(x, z);
        let mut weight = 0.0;
        let mut level = 0.0;
        for (&i, w) in indices.iter().zip(weights) {
            if self.lakes[i] > 0.0 {
                weight += w;
                level += self.lakes[i] * w;
            }
        }
        if weight > 0.25 {
            level / weight
        } else {
            0.0
        }
    }
}

impl Channel {
    // Local curved segment, anchored at both drainage nodes. Bins avoid testing
    // the entire network for every output pixel or microvoxel column.
    fn sample(&self, x: f64, z: f64) -> Option<(f64, f64, f64)> {
        let vx = self.end[0] - self.start[0];
        let vz = self.end[1] - self.start[1];
        let length_squared = vx * vx + vz * vz;
        let t = (((x - self.start[0]) * vx + (z - self.start[1]) * vz) / length_squared)
            .clamp(0.0, 1.0);
        let bend = self.bend * (std::f64::consts::PI * t).sin() / length_squared.sqrt();
        let px = self.start[0] + t * vx - vz * bend;
        let pz = self.start[1] + t * vz + vx * bend;
        let squared_distance = (x - px).powi(2) + (z - pz).powi(2);
        if squared_distance > (self.half_width + 12.0).powi(2) {
            return None;
        }
        Some((
            squared_distance.sqrt(),
            lerp(self.level[0], self.level[1], t),
            self.half_width,
        ))
    }
}

// ---- Local detail: terrain-dependent roughness down to 12.5 cm wavelengths. ----

fn surface_detail(mut x: f64, mut z: f64, spacing: f64, seed: u64, roughness: f64) -> f64 {
    let mut wavelength: f64 = 32.0;
    let mut amplitude = roughness;
    let mut sum = 0.0;
    for octave in 0..9 {
        // Smoothly fade the last octave rather than abruptly adding an alias.
        let visibility = smoothstep(1.0, 2.0, wavelength / spacing);
        if visibility == 0.0 {
            break;
        }
        sum += amplitude * visibility * noise(x, z, wavelength, seed ^ (200 + octave));
        wavelength *= 0.5;
        amplitude *= 0.57;
        (x, z) = octave_position(x, z);
    }
    sum
}

pub(crate) fn terrain_surface(region: &Region, x: f64, z: f64, spacing: f64, seed: u64) -> Surface {
    let base = base_height(x, z, seed) + region.field_at(&region.erosion, x, z);
    let soil = region.field_at(&region.soil, x, z).max(0.0);
    let runoff = region.field_at(&region.flow, x, z);
    let wet_valley = smoothstep(RIVER_THRESHOLD * 0.3, RIVER_THRESHOLD * 2.0, runoff);
    // Deposited soil and alluvial lowlands are smoother than exposed bedrock.
    let rockiness = 1.0 - smoothstep(0.25, 2.5, soil);
    let roughness = lerp(0.25, 5.0, rockiness) * lerp(1.0, 0.2, wet_valley);
    let detail = surface_detail(x, z, spacing, seed, roughness);
    let mut height = base + detail;
    let mut water: f64 = 0.0;
    let mut bank = 0.0_f64;
    let bins_side = (REGION_SIZE / BIN_SIZE).ceil() as usize + 1;
    if !region.bins.is_empty() {
        let bx = (x / BIN_SIZE) as usize;
        let bz = (z / BIN_SIZE) as usize;
        let mut closest = None;
        let mut best = f64::INFINITY;
        for &index in &region.bins[bz * bins_side + bx] {
            let channel = &region.channels[index];
            if let Some((distance, level, half_width)) = channel.sample(x, z) {
                let score = distance / half_width;
                if score < best {
                    best = score;
                    closest = Some((distance, level, half_width, channel.depth));
                }
            }
        }
        if let Some((distance, level, half_width, depth)) = closest {
            let edge = distance / half_width;
            let influence = 1.0 - smoothstep(half_width, half_width + 12.0, distance);
            bank = influence;
            // Use one nearest channel profile; combining the highest water
            // with a different segment's lowest bed would overfill confluences.
            let bed = level - depth * (1.0 - edge.min(1.0).powi(2));
            let target = if edge <= 1.0 {
                bed
            } else {
                level + (distance - half_width) * 0.35
            };
            height = lerp(base + detail, base.min(target), influence);
            if edge <= 1.0 {
                // Gravel-scale bed variation stays well below channel depth.
                let grain = surface_detail(x + 37.0, z - 19.0, spacing, seed ^ 701, 0.025);
                height = bed + grain.clamp(-0.04, 0.04);
            }
            if edge < 1.0 {
                water = level;
            }
        }
    }
    let lake = region.lake_at(x, z);
    if lake > 0.0 {
        // Remove high-frequency detail underwater and at the lake shore.
        let shore = 1.0 - smoothstep(lake - 0.5, lake + 1.0, base);
        height = lerp(height, height.min(base), shore);
        if height < lake {
            water = water.max(lake);
        }
    }
    height = height.clamp(0.0, HEIGHT_SCALE);
    if water <= height + 0.02 {
        water = 0.0;
    }
    let material = if bank > 0.25 {
        if water > 0.0 {
            GRAVEL
        } else {
            SAND
        }
    } else if soil < 0.6 {
        ROCK
    } else if soil > 5.0 && wet_valley > 0.25 {
        SAND
    } else {
        SOIL
    };
    // Rock outcrops follow fine rock detail; channel carving can strip soil.
    let bedrock = (base - soil + detail * rockiness * (1.0 - bank))
        .min(height)
        .max(0.0);
    Surface {
        height,
        water,
        bedrock,
        material,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drainage_reaches_outlets_and_preserves_lake_bed() {
        let mut region = Region::from_heights(5, vec![10.0; 25]);
        for i in [6, 7, 11, 12] {
            region.heights[i] = 1.0;
        }
        let before = region.heights.clone();
        let drainage = region.drainage_tree(42);
        let mut seen = vec![false; 25];
        for &i in &drainage.order {
            assert!(drainage.receiver[i] == i || seen[drainage.receiver[i]]);
            seen[i] = true;
        }
        assert!(seen.iter().all(|v| *v));
        assert_eq!(region.heights, before); // Virtual filling adds no terrain.
        region.preserve_lakes(&drainage);
        for i in [6, 7, 11, 12] {
            assert!((region.lakes[i] - 10.0).abs() < 0.001);
        }
    }

    #[test]
    fn sediment_is_conserved_and_layers_stay_nonnegative() {
        let heights: Vec<f64> = (0..49).map(|i| 10.0 + (i / 7) as f64 * 12.0).collect();
        let mut region = Region::from_heights(7, heights);
        let before: f64 = region.heights.iter().sum::<f64>() * CELL_AREA;
        for _ in 0..8 {
            let drainage = region.drainage_tree(42);
            region.transport_sediment(&drainage);
            region.relax_slopes();
        }
        let budget = &region.budget;
        assert!(budget.eroded > 0.0);
        assert!((budget.eroded - budget.deposited - budget.exported).abs() < 1e-7);
        let after = region.heights.iter().sum::<f64>() * CELL_AREA;
        assert!((before - after - budget.exported).abs() < 1e-7);
        for i in 0..region.heights.len() {
            assert!(region.soil[i] >= -1e-12);
            assert!((region.heights[i] - region.soil[i] - region.bedrock[i]).abs() < 1e-10);
        }
    }

    #[test]
    fn soil_slumping_conserves_mass_and_preserves_bedrock() {
        let mut region = Region::from_heights(3, vec![5.0; 9]);
        region.heights[4] = 100.0;
        region.soil[4] = 95.0;
        region.bedrock[4] = 5.0;
        let before: f64 = region.heights.iter().sum();
        let bedrock = region.bedrock.clone();
        region.relax_slopes();
        assert!((region.heights.iter().sum::<f64>() - before).abs() < 1e-10);
        assert_eq!(region.bedrock, bedrock);
        assert!(region.soil.iter().all(|h| *h >= 0.0));
        assert!(region.heights[4] < 100.0);
    }

    #[test]
    fn channel_carves_bed_below_downhill_water_surface() {
        let channel = Channel {
            start: [0.0, 0.0],
            end: [16.0, 0.0],
            level: [12.0, 10.0],
            half_width: 2.0,
            depth: 1.0,
            bend: 0.0,
        };
        let (distance, level, width) = channel.sample(8.0, 0.0).unwrap();
        assert_eq!(distance, 0.0);
        assert_eq!(level, 11.0);
        assert_eq!(width, 2.0);
        assert!(channel.sample(8.0, 30.0).is_none());
        let mut region = Region::from_heights(3, vec![20.0; 9]);
        region.channels.push(channel);
        let side = (REGION_SIZE / BIN_SIZE).ceil() as usize + 1;
        region.bins = vec![Vec::new(); side * side];
        region.bins[0].push(0);
        let sample = terrain_surface(&region, 8.0, 0.0, 0.0625, 42);
        assert!((sample.height - 10.0).abs() < 0.04);
        assert_eq!(sample.water, 11.0);
        assert_eq!(sample.material, GRAVEL);
        assert!(sample.bedrock <= sample.height);
    }

    #[test]
    fn routed_flow_is_conserved_and_river_levels_descend() {
        let heights = (0..81)
            .map(|i| 20.0 + ((mix(i as u64) >> 48) as f64) / 2048.0)
            .collect();
        let mut region = Region::from_heights(9, heights);
        region.rainfall.fill(500.0); // Force this tiny test to contain channels.
        let drainage = region.drainage_tree(42);
        let source: f64 = region.rainfall.iter().sum::<f64>() * CELL_AREA;
        let outlets: f64 = drainage
            .order
            .iter()
            .filter(|&&i| drainage.receiver[i] == i)
            .map(|&i| drainage.area[i])
            .sum();
        assert!((source - outlets).abs() < 1e-6);
        region.preserve_lakes(&drainage);
        region.build_channels(&drainage, 42);
        assert!(!region.channels.is_empty());
        for channel in &region.channels {
            assert!(channel.level[0] >= channel.level[1] - 0.001);
            assert!(channel.depth > 0.04);
        }
    }

    #[test]
    fn fine_detail_is_repeatable_and_has_centimeter_variation() {
        let a = surface_detail(100.03, 201.07, 0.0625, 42, 6.0);
        assert_eq!(a, surface_detail(100.03, 201.07, 0.0625, 42, 6.0));
        assert_ne!(a, surface_detail(100.0925, 201.07, 0.0625, 42, 6.0));
    }
}
