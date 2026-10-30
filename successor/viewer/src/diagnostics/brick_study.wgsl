// Experimental canonical-cell DDA over 32^3 bricks; no filtered surfaces.
struct Ray { origin: vec4<f32>, direction: vec4<f32>, controls: vec4<u32> }
struct Hit { cell_status: vec4<i32>, value_work: vec4<u32> }
struct Params { minimum: vec4<i32>, maximum: vec4<i32>, dimensions: vec4<u32> }
@group(0) @binding(0) var<storage, read> cells: array<u32>;
@group(0) @binding(1) var<storage, read> rays: array<Ray>;
@group(0) @binding(2) var<storage, read_write> hits: array<Hit>;
@group(0) @binding(3) var<uniform> params: Params;

// Compare positive crossing fractions without repeated tMax accumulation.
// Products that round to the same f32 are ordered by their FMA residuals.
// This matters when adjacent high-entropy cells have different exact colors.
fn crossing_le(a: u32, b: u32, numerator: vec3<f32>, denominator: vec3<f32>) -> bool {
    if denominator[b] == 0.0 { return true; }
    if denominator[a] == 0.0 { return false; }
    let lhs = numerator[a] * denominator[b];
    let rhs = numerator[b] * denominator[a];
    if lhs != rhs { return lhs < rhs; }
    return fma(numerator[a], denominator[b], -lhs) <= fma(numerator[b], denominator[a], -rhs);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if index >= params.dimensions.w { return; }
    let ray = rays[index];
    // status: 0 distance-limited miss; 1 exact hit; 2 unknown; 3 incomplete.
    var result = Hit(vec4<i32>(0, 0, 0, 3), vec4<u32>(0));
    let norm = length(ray.direction.xyz);
    if norm == 0.0 || ray.origin.w < 0.0 || ray.controls.x == 0u {
        hits[index] = result; return;
    }
    let direction = ray.direction.xyz;
    let denominator = abs(direction);
    var cell = vec3<i32>(floor(ray.origin.xyz));
    var step = vec3<i32>(0);
    var numerator = vec3<f32>(0.0);
    for (var a = 0u; a < 3u; a++) {
        if direction[a] > 0.0 {
            step[a] = 1;
            numerator[a] = f32(cell[a]) + 1.0 - ray.origin[a];
        } else if direction[a] < 0.0 {
            step[a] = -1;
            numerator[a] = ray.origin[a] - f32(cell[a]);
        }
    }
    for (var visit = 0u; visit < ray.controls.x; visit++) {
        result.value_work.y = visit + 1u;
        if any(cell < params.minimum.xyz) || any(cell >= params.maximum.xyz) { break; }
        // Relative coordinates are nonnegative, preserving signed world cells.
        let relative = vec3<u32>(cell - params.minimum.xyz);
        let brick = relative / vec3<u32>(32u);
        let local = relative % vec3<u32>(32u);
        let brick_index = (brick.z * params.dimensions.y + brick.y) * params.dimensions.x + brick.x;
        let local_index = (local.z * 32u + local.x) * 32u + local.y;
        let value = cells[brick_index * 32768u + local_index];
        if value == 0xffffffffu {
            result.cell_status = vec4<i32>(cell, 2); break;
        }
        if (value & 65535u) != 0u {
            result.cell_status = vec4<i32>(cell, 1);
            result.value_work.x = value; break;
        }
        var axis = 2u;
        if crossing_le(0u, 1u, numerator, denominator) && crossing_le(0u, 2u, numerator, denominator) {
            axis = 0u;
        } else if crossing_le(1u, 2u, numerator, denominator) {
            axis = 1u;
        }
        let distance = (numerator[axis] / denominator[axis]) * norm;
        if distance > ray.origin.w {
            result.cell_status.w = 0; break;
        }
        // Simultaneous crossings enter only the diagonal cell, matching CPU.
        let crossings = vec3<bool>(
            crossing_le(0u, axis, numerator, denominator),
            crossing_le(1u, axis, numerator, denominator),
            crossing_le(2u, axis, numerator, denominator));
        for (var a = 0u; a < 3u; a++) {
            if crossings[a] && denominator[a] != 0.0 {
                cell[a] += step[a];
                if step[a] > 0 { numerator[a] = f32(cell[a]) + 1.0 - ray.origin[a]; }
                else { numerator[a] = ray.origin[a] - f32(cell[a]); }
            }
        }
    }
    hits[index] = result;
}
