// Land stage of world/src/seeded_world.rs and world/src/eroded_relief.rs.
// Explicit precision maintains native integer height parity at large coordinates.
@group(0) @binding(0) var<storage,read> positions: array<vec2i>;
@group(0) @binding(1) var<storage,read> params: array<u64>;
@group(0) @binding(2) var<storage,read_write> heights: array<f64>;
@group(0) @binding(3) var<storage,read> drainage: array<f64>;
override FULL_SOURCE: bool = true;

fn cubic(value: f64) -> f64 {
    let t = clamp(value, 0.lf, 1.lf);
    return t * t * (3.lf - 2.lf * t);
}

fn scramble(x: i64, z: i64, seed: u64) -> u64 {
    var h = (u64(x) * 0x9e3779b97f4a7c15lu) ^ (u64(z) * 0xbf58476d1ce4e5b9lu) ^ seed;
    h = (h ^ (h >> 30u)) * 0xbf58476d1ce4e5b9lu;
    h = (h ^ (h >> 27u)) * 0x94d049bb133111eblu;
    return h ^ (h >> 31u);
}

fn gradient(h: u64, x: f64, z: f64) -> f64 {
    let diagonal = 0.707106781186547524400844362104849lf;
    switch u32(h & 7lu) {
        case 0u: { return x; }
        case 1u: { return -x; }
        case 2u: { return z; }
        case 3u: { return -z; }
        case 4u: { return (x + z) * diagonal; }
        case 5u: { return (x - z) * diagonal; }
        case 6u: { return (-x + z) * diagonal; }
        default: { return (-x - z) * diagonal; }
    }
}

fn quintic(t: f64) -> f64 {
    return t * t * t * (t * (t * 6.lf - 15.lf) + 10.lf);
}

fn noise(x: f64, z: f64, seed: u64) -> f64 {
    let x0 = floor(x);
    let z0 = floor(z);
    let i = i64(x0);
    let j = i64(z0);
    let u = x - x0;
    let v = z - z0;
    let s = quintic(u);
    let t = quintic(v);
    let a = gradient(scramble(i, j, seed), u, v);
    let b = gradient(scramble(i + 1li, j, seed), u - 1.lf, v);
    let c = gradient(scramble(i, j + 1li, seed), u, v - 1.lf);
    let d = gradient(scramble(i + 1li, j + 1li, seed), u - 1.lf, v - 1.lf);
    let lower = a + (b - a) * s;
    let upper = c + (d - c) * s;
    return clamp((lower + (upper - lower) * t) * 1.6lf, f64(-1.), 1.lf);
}

fn fbm(x: f64, z: f64, wavelength: f64, count: u32, gain: f64, salt: u64, seed: u64) -> f64 {
    var weight = 1.lf;
    var frequency = 1.lf / wavelength;
    var sum = 0.lf;
    var norm = 0.lf;
    for (var i = 0u; i < count; i += 1u) {
        let octave_seed = seed ^ (salt + u64(i) * 0x9e3779b9lu);
        sum += weight * noise(x * frequency + 13.7lf, z * frequency - 8.3lf, octave_seed);
        norm += weight;
        weight *= gain;
        frequency *= 2.lf;
    }
    return sum / norm;
}

fn province_relief(seed: u64, x: f64, z: f64, base: f64, uplift: f64, strata: f64, rain: f64) -> f64 {
    let mountain = cubic((uplift - 0.28lf) / 0.64lf) * cubic((base - 95.lf) / 250.lf);
    let coastal = cubic((base - 1.lf) / 13.lf) * (1.lf - cubic((base - 22.lf) / 56.lf));
    if mountain + coastal == 0.lf {
        return 0.lf;
    }
    let spine = noise((x + 290.lf * strata) / 780.lf, (z + 440.lf * rain) / 890.lf, seed ^ 0x51b0lu);
    let fork = noise((x - 92.lf * spine) / 360.lf, (z + 111.lf * spine) / 310.lf, seed ^ 0x51b1lu);
    let arid = cubic((-rain - 0.025lf) / 0.31lf);
    let rib = cubic((0.31lf - abs(spine + 0.39lf * fork)) / 0.30lf);
    let bowl = cubic((fork - 0.08lf) / 0.52lf) * cubic((-spine + 0.02lf) / 0.57lf);
    let mountain_relief = (1.lf - arid) * (66.lf * (rib - 0.43lf) - 42.lf * bowl)
        + arid * (70.lf * (cubic((spine + 0.12lf) / 0.26lf) - 0.5lf) + 16.lf * fork);
    let coast_relief = (13.lf * spine + 5.lf * fork) * fma(arid, 0.35lf, 0.65lf);
    return clamp(mountain * mountain_relief + coastal * coast_relief, f64(-86.), 86.lf);
}

fn bedrock(seed: u64, x: f64, z: f64, base: f64, uplift: f64, strata: f64, moisture: f64) -> f64 {
    if base <= 4.lf {
        return 0.lf;
    }
    let exposure = noise(x / 930.lf, z / 960.lf, seed ^ 0x5171lu);
    let fractures = noise(x / 93.lf + z / 190.lf, z / 350.lf - x / 270.lf, seed ^ 0x5172lu);
    let tilt = select(f64(-1.), 1.lf, (seed & 1lu) == 0lu);
    let thickness = 39.lf + 12.lf * strata + 4.5lf * exposure;
    let along = tilt * 0.81lf * x + 0.59lf * z;
    let across = f64(-0.59) * x + tilt * 0.81lf * z;
    let fan = noise((along + 119.lf * strata) / 180.lf, across / 480.lf, seed ^ 0x5175lu);
    let branch = noise(along / 72.lf - 0.8lf * exposure, across / 173.lf + 0.66lf * fan, seed ^ 0x5176lu);
    let phase = (base + tilt * (0.039lf * x + 0.020lf * z) + 62.lf * strata + 12.lf * fractures + 17.lf * fan)
        / thickness;
    let index = floor(phase);
    let t = phase - index;
    let width = 0.17lf + 0.21lf * cubic((fractures + 0.38lf) / 0.78lf);
    let riser = clamp((t - 0.5lf) / width + 0.5lf, 0.lf, 1.lf);
    let step = 0.23lf * t + 0.77lf * cubic(riser);
    let mountain = uplift * uplift * cubic((exposure + 0.42lf) / 0.88lf);
    let mesa = cubic((-moisture - 0.06lf) / 0.34lf) * cubic((strata + 0.16lf) / 0.58lf);
    let joint_mask = 0.32lf + 0.68lf * cubic((fractures + 0.43lf + 0.30lf * fan) / 0.98lf);
    let strength = max(mountain, 0.82lf * mesa) * cubic((base - 18.lf) / 45.lf);
    let erosion = 7.5lf * fractures * cubic((exposure + 0.18lf) / 0.65lf);
    var resistance = 0.lf;
    if strength > 0.lf {
        let cement = noise(along / 470.lf + 1.57lf * index, across / 605.lf - 0.88lf * index, seed ^ 0x51b2lu);
        resistance = 0.19lf + 0.81lf * cubic((cement + 0.35lf) / 0.73lf);
    }
    let hard_face = strength * joint_mask * resistance * 4.lf * riser * (1.lf - riser);
    var joint = 0.lf;
    if hard_face > 0.lf {
        let shape = noise(along / 15.lf + 0.8lf * branch, across / 31.lf, seed ^ 0x5177lu);
        joint = hard_face * (4.8lf * shape + 1.9lf * cubic((shape + 0.28lf) / 0.42lf));
    }
    let ledges = strength * joint_mask * (resistance * (step - t) * thickness + erosion) + joint;
    let ribs = cubic((0.20lf - abs(fan + 0.30lf * branch)) / 0.25lf);
    let relief = strength * (38.lf * (fan + 0.10lf) + 21.lf * (ribs - 0.35lf) + 9.lf * branch);
    let lowland = (1.lf - uplift * uplift) * cubic((base - 4.lf) / 26.lf) * (1.lf - cubic((base - 155.lf) / 185.lf));
    return clamp(ledges + relief + lowland * (13.lf * fan + 4.lf * branch), f64(-61.), 61.lf);
}

fn land(xm: f64, zm: f64) -> f64 {
    let seed = params[0];
    let scale = bitcast<f64>(params[1]);
    let amplitude = bitcast<f64>(params[2]);
    let roughness = bitcast<f64>(params[3]);
    let octaves = u32(params[4]);
    let x = xm * 1800.lf / scale;
    let z = zm * 1800.lf / scale;
    let continent = fbm(x, z, 48000.lf, 3u, 0.45lf, 0x5101lu, seed);
    let province = fbm(x, z, 22000.lf, 3u, 0.45lf, 0x5102lu, seed);
    let mountain = cubic((province + 0.08lf) / 0.48lf);
    let ridge = 1.lf - abs(fbm(x, z, 3600.lf, 4u, 0.48lf, 0x5105lu, seed));
    let hills = fbm(x, z, 1800.lf, 3u, 0.45lf, 0x5106lu, seed);
    let fine = fbm(x, z, 90.lf, octaves, 0.65lf * roughness, 0x5107lu, seed);
    let strata = fbm(x, z, 1050.lf, 3u, 0.45lf, 0x5121lu, seed);
    let spine = ridge * ridge * ridge * 1520.lf;
    let upland = mountain * mountain;
    let base = 440.lf * continent + 90.lf + upland * (spine + 145.lf * strata)
        + hills * (50.lf + 30.lf * strata) + 5.lf * fine;
    let moisture = fbm(x, z, 27000.lf, 3u, 0.5lf, 0x5104lu, seed);
    let ribs = fbm(x, z, 620.lf, 2u, 0.47lf, 0x5130lu, seed);
    let cuts = fbm(x, z, 180.lf, 2u, 0.40lf, 0x5131lu, seed);
    let shoulders = cubic((ridge - 0.52lf) * (2.7777777777777777lf));
    let ravine = cubic((ribs + 0.30lf * cuts - 0.13lf) * (2.564102564102564lf));
    let crest = cubic((ribs + 0.32lf * strata - 0.03lf) * (2.272727272727273lf));
    let dry = cubic((-moisture - 0.06lf) * (3.3333333333333335lf));
    let platform = strata + 0.22lf * ribs;
    let bench = 37.lf * cubic((platform + 0.12lf) * (6.25lf))
        + 27.lf * cubic((platform - 0.26lf) * (7.142857142857142lf)) - 14.lf * platform;
    let banks = cubic((cuts + 0.26lf * strata - 0.10lf) * (2.857142857142857lf));
    let landscape = base + upland * (shoulders * (94.lf * crest - 108.lf * ravine))
        + dry * bench + (1.lf - upland) * banks * f64(-10.);
    let joints = noise(x / 23.lf + z / 30.lf, z / 8.5lf - x / 11.lf, seed ^ 0x5140lu);
    let debris = noise(x / 3.1lf, z / 3.1lf, seed ^ 0x5141lu);
    let exposure = cubic((ridge - 0.45lf) * (2.6315789473684212lf)) * cubic((strata + 0.38lf) * (1.5873015873015872lf));
    let fracture = joints + 0.24lf * debris;
    let ledges = 2.8lf * cubic((fracture + 0.23lf) * (5.0lf))
        + 4.2lf * cubic((fracture - 0.21lf) * (5.2631578947368425lf)) - 3.5lf;
    let strength = exposure * (0.80lf * upland + 0.20lf);
    let rock = landscape + (strength * (ledges + debris * debris * 0.85lf) + 0.24lf * debris);
    let foundation = rock + province_relief(seed, x, z, rock, mountain, strata, moisture);
    return amplitude / 1000.lf * (foundation + bedrock(seed, x, z, foundation, mountain, strata, moisture));
}

// Each region and bin uses the coordinate transforms of the CPU graph.
fn drainage_bin(x: f64, z: f64, root: u32) -> vec2u {
    let width = drainage[root + 4u];
    let ix = i32(floor(x / width) - drainage[root]);
    let iz = i32(floor(z / width) - drainage[root + 1u]);
    let nx = i32(drainage[root + 2u]);
    let nz = i32(drainage[root + 3u]);
    if ix < 0 || iz < 0 || ix >= nx || iz >= nz {
        return vec2u(0u);
    }
    let region = u32(drainage[root + 6u]) + 4u * u32(iz * nx + ix);
    let spacing = drainage[region + 2u];
    let maximum = drainage[root + 5u] - 1.lf;
    let bx = u32(clamp(floor((x - drainage[region]) / spacing), 0.lf, maximum));
    let bz = u32(clamp(floor((z - drainage[region + 1u]) / spacing), 0.lf, maximum));
    let bin = u32(drainage[region + 3u]) + 2u * (bz * u32(drainage[root + 5u]) + bx);
    return vec2u(u32(drainage[bin]), u32(drainage[bin + 1u]));
}

fn incision(x: f64, z: f64) -> f64 {
    let bin = drainage_bin(x, z, 7u);
    var cut = 0.lf;
    for (var i = 0u; i < bin.y; i += 1u) {
        let r = bin.x + 12u * i;
        let dx = x - drainage[r];
        let dz = z - drainage[r + 1u];
        let vx = drainage[r + 2u];
        let vz = drainage[r + 3u];
        let t = clamp((dx * vx + dz * vz) * drainage[r + 4u], 0.lf, 1.lf);
        let nx = dx - t * vx;
        let nz = dz - t * vz;
        let distance2 = nx * nx + nz * nz;
        let radius = drainage[r + 5u];
        if distance2 >= radius * radius {
            continue;
        }
        let cross_section = sqrt(distance2) / radius;
        let shoulder = 1.lf - cubic(cross_section);
        let bed = 1.lf - cubic((cross_section - 0.06lf) / 0.49lf);
        let along = drainage[r + 7u] + (drainage[r + 8u] - drainage[r + 7u]) * t;
        let longitudinal = cubic(along / 0.22lf) * (0.76lf + 0.24lf * cubic((1.lf - along) / 0.18lf));
        cut = max(cut, drainage[r + 6u] * longitudinal * (0.39lf * shoulder + 0.61lf * bed));
    }
    return cut;
}

fn ground_height(x: f64, z: f64) -> f64 {
    let uncut = land(x, z);
    let relief = bitcast<f64>(params[2]) / 1000.lf;
    if relief <= 0.lf {
        return uncut;
    }
    let scale = bitcast<f64>(params[1]);
    let bin = drainage_bin(x, z, 0u);
    var weighted_ground = uncut;
    var total_weight = 1.lf;
    var bank_distance = 1.lf;
    var water_stage = f64(-1e30);
    for (var i = 0u; i < bin.y; i += 1u) {
        let r = bin.x + 12u * i;
        let dx = drainage[r + 2u] - drainage[r];
        let dz = drainage[r + 3u] - drainage[r + 1u];
        let denominator = dx * dx + dz * dz;
        var t = 0.lf;
        let from_x = x - drainage[r];
        let from_z = z - drainage[r + 1u];
        if denominator > 0.lf {
            t = clamp((from_x * dx + from_z * dz) / denominator, 0.lf, 1.lf);
        }
        let u = from_x - t * dx;
        let v = from_z - t * dz;
        let distance = sqrt(u * u + v * v);
        let stage = drainage[r + 4u] + t * (drainage[r + 5u] - drainage[r + 4u]);
        let width = drainage[r + 6u] + t * (drainage[r + 7u] - drainage[r + 6u]);
        let outer = drainage[r + 9u];
        if distance >= outer || stage < 0.lf {
            continue;
        }
        let depth = drainage[r + 8u] * relief;
        let bank_height = 0.7lf * relief;
        var profile = 0.lf;
        if distance <= width {
            profile = stage - depth + (depth + bank_height) * cubic(distance / width);
        } else {
            let side = (distance - width) / (outer - width);
            let shoulder = 0.56lf * cubic(side)
                + 0.19lf * cubic((side - 0.10lf) / 0.14lf)
                + 0.25lf * cubic((side - 0.57lf) / 0.36lf);
            profile = stage + bank_height + (uncut - stage - bank_height) * shoulder;
        }
        let taper = 1.lf - cubic(distance / outer);
        let weight = outer * outer / max(distance * distance, width * width * 1e-8lf) * taper * taper;
        weighted_ground += weight * min(profile, uncut);
        total_weight += weight;
        if drainage[r + 10u] != 0.lf {
            if stage > 0.lf {
                bank_distance = min(bank_distance, distance / max(width * 4.5lf, 28.lf * scale / 1800.lf));
            }
            if distance < width {
                water_stage = max(water_stage, stage);
            }
        }
    }
    let ground = weighted_ground / total_weight;
    if uncut < 0.lf || water_stage > ground {
        return ground;
    }
    let floodplain = cubic((bank_distance - 0.30lf) / 0.65lf);
    let exposure = cubic((ground / relief - 3.lf) / 14.lf);
    if floodplain * exposure <= 0.lf {
        return ground;
    }
    return ground - floodplain * exposure * incision(x, z);
}

@compute @workgroup_size(64)
fn generate(@builtin(global_invocation_id) id: vec3u) {
    if id.x >= arrayLength(&positions) {
        return;
    }
    let p = positions[id.x];
    let x = (f64(p.x) + 0.5lf) * 0.0625lf;
    let z = (f64(p.y) + 0.5lf) * 0.0625lf;
    if FULL_SOURCE {
        heights[id.x] = ground_height(x, z);
    } else {
        heights[id.x] = land(x, z);
    }
}
