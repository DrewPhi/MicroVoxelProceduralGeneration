struct CompactionJob {
    source: u32,
    count: u32,
    draw: u32,
    output: u32,
    regions: u32,
    padding: array<u32, 3>,
};

struct DrawIndirect {
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
};

@group(0) @binding(0) var<storage, read> source_words: array<u32>;
@group(0) @binding(1) var<storage, read> camera_words: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> jobs: array<CompactionJob>;
@group(0) @binding(3) var<storage, read_write> compact: array<u32>;
@group(0) @binding(4) var<storage, read_write> commands: array<DrawIndirect>;

const CAMERA_STRIDE: u32 = 16u;
const COLUMN_STRIDE: u32 = 13u;
const REGION_STRIDE: u32 = 6u;
const REGION_COLUMNS: u32 = 16u;
const MAX_COLUMNS: u32 = 256u;

var<workgroup> region_visible: array<u32, 16>;
var<workgroup> lane_masks: array<u32, 64>;
var<workgroup> region_masks: array<u32, 16>;
var<workgroup> region_prefix: array<u32, 16>;

struct Frustum {
    left: vec4<f32>,
    right: vec4<f32>,
    bottom: vec4<f32>,
    top: vec4<f32>,
    near: vec4<f32>,
    far: vec4<f32>,
    error_scale: vec4<f32>,
};

fn make_frustum(matrix: mat4x4<f32>) -> Frustum {
    let rows = transpose(matrix);
    let x = rows[0]; let y = rows[1]; let z = rows[2]; let w = rows[3];
    return Frustum(w + x, w - x, w + y, w - y, z, w - z,
                   abs(x) + abs(y) + abs(z) + abs(w));
}

fn outside_plane(plane: vec4<f32>, lo: vec3<f32>, hi: vec3<f32>, margin: f32) -> bool {
    let corner = select(lo, hi, plane.xyz >= vec3<f32>(0.));
    return dot(plane, vec4<f32>(corner, 1.)) < -margin;
}

fn frustum_intersects(frustum: Frustum, lo: vec3<f32>, hi: vec3<f32>) -> bool {
    // Account for position/transform roundoff before rejecting even a whole
    // box lying close to a plane. The uncombined row magnitudes also bound
    // cancellation in the far-plane row w-z.
    let scale = dot(frustum.error_scale, vec4<f32>(max(abs(lo), abs(hi)), 1.));
    let margin = 0.0000625 + scale / 65536.;
    return !(outside_plane(frustum.left, lo, hi, margin)
        || outside_plane(frustum.right, lo, hi, margin)
        || outside_plane(frustum.bottom, lo, hi, margin)
        || outside_plane(frustum.top, lo, hi, margin)
        || outside_plane(frustum.near, lo, hi, margin)
        || outside_plane(frustum.far, lo, hi, margin));
}

fn source_f32(index: u32) -> f32 {
    return bitcast<f32>(source_words[index]);
}

fn source_vec3(index: u32) -> vec3<f32> {
    return vec3<f32>(source_f32(index), source_f32(index + 1u), source_f32(index + 2u));
}

fn visible_column(word: u32, eye: vec3<f32>, frustum: Frustum) -> bool {
    let xz = vec4<f32>(source_f32(word), source_f32(word + 1u),
                       source_f32(word + 2u), source_f32(word + 3u));
    let top = source_f32(word + 4u);
    let floors = vec4<f32>(source_f32(word + 6u), source_f32(word + 7u),
                           source_f32(word + 8u), source_f32(word + 9u));
    let bottom = min(top - 0.0625, min(min(floors.x, floors.y), min(floors.z, floors.w)));
    let lo = vec3<f32>(xz.x, bottom, xz.y);
    let hi = vec3<f32>(xz.z, top, xz.w);
    let scale = max(max(abs(lo), abs(hi)), abs(eye));
    let margin = 0.0625 / 1024. + max(scale.x, max(scale.y, scale.z)) / 65536.;
    let eye_facing = eye.y >= top - margin
        || (eye.x <= xz.x + margin && floors.x < top) || (eye.x >= xz.z - margin && floors.y < top)
        || (eye.z <= xz.y + margin && floors.z < top) || (eye.z >= xz.w - margin && floors.w < top);
    return eye_facing && frustum_intersects(frustum, lo, hi);
}

fn compact_tile(group: vec3<u32>, lane: u32, vertex_count: u32) {
    if group.x >= arrayLength(&jobs) {
        return;
    }
    let job = jobs[group.x];
    let words = arrayLength(&source_words);
    let destination = arrayLength(&compact);
    let cameras = arrayLength(&camera_words) / CAMERA_STRIDE;
    let valid_draw = job.draw < arrayLength(&commands);
    let valid_camera = job.draw < cameras;
    let valid_source = job.source <= words && job.count <= (words - job.source) / COLUMN_STRIDE;
    let valid_output = job.output <= destination && job.count <= destination - job.output;
    let count = select(0u, job.count, valid_draw && valid_camera && valid_source && valid_output && job.count <= MAX_COLUMNS);
    let region_count = (count + REGION_COLUMNS - 1u) / REGION_COLUMNS;
    let have_regions = job.regions <= words && region_count <= (words - job.regions) / REGION_STRIDE;

    var frustum = make_frustum(mat4x4<f32>());
    var eye = vec3<f32>(0.);
    if valid_camera {
        let base = job.draw * CAMERA_STRIDE;
        frustum = make_frustum(mat4x4<f32>(camera_words[base], camera_words[base + 1u],
                                            camera_words[base + 2u], camera_words[base + 3u]));
        eye = camera_words[base + 4u].xyz;
    }
    if lane < 16u {
        var keep = lane < region_count;
        if keep && have_regions {
            let address = job.regions + lane * REGION_STRIDE;
            let lo = source_vec3(address);
            let hi = source_vec3(address + 3u);
            // A malformed bound cannot justify omitting the enclosed columns.
            keep = any(lo > hi) || frustum_intersects(frustum, lo, hi);
        }
        region_visible[lane] = u32(keep);
    }
    workgroupBarrier();

    // Four lanes share a 16-column region. Each lane owns four ordered bits.
    let region = lane / 4u;
    let first_bit = (lane % 4u) * 4u;
    var seen = 0u;
    if region_visible[region] != 0u {
        for (var bit = first_bit; bit < first_bit + 4u; bit++) {
            let index = region * REGION_COLUMNS + bit;
            if index < count && visible_column(job.source + index * COLUMN_STRIDE, eye, frustum) {
                seen |= 1u << bit;
            }
        }
    }
    lane_masks[lane] = seen;
    workgroupBarrier();

    if lane == 0u {
        var total = 0u;
        for (var block = 0u; block < region_count; block++) {
            let first = block * 4u;
            let mask = lane_masks[first] | lane_masks[first + 1u] | lane_masks[first + 2u] | lane_masks[first + 3u];
            region_masks[block] = mask;
            region_prefix[block] = total;
            total += countOneBits(mask);
        }
        if valid_draw {
            // The render pass binds a compact-buffer slice at job.output.
            commands[job.draw] = DrawIndirect(vertex_count, total, 0u, 0u);
        }
    }
    workgroupBarrier();

    if seen != 0u {
        for (var bit = first_bit; bit < first_bit + 4u; bit++) {
            let mask = 1u << bit;
            if (seen & mask) != 0u {
                let slot = region_prefix[region] + countOneBits(region_masks[region] & (mask - 1u));
                let index = region * REGION_COLUMNS + bit;
                compact[job.output + slot] = job.source + index * COLUMN_STRIDE;
            }
        }
    }
}

@compute @workgroup_size(64)
fn compact_boxes(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    compact_tile(group, lane, 4u);
}

@compute @workgroup_size(64)
fn compact_quads(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    compact_tile(group, lane, 54u);
}
