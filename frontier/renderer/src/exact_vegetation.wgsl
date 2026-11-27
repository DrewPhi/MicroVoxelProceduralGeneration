struct View {
    eye: vec4f,
    forward: vec4f,
    right: vec4f,
    up: vec4f,
    sun: vec4f,
    clip: vec4f,
    counts: vec4u,
}
struct Node {
    min: vec3i,
    right: u32,
    max: vec3i,
    end: u32,
}
struct Brick {
    min: vec3i,
    pigment: u32,
    mask: vec2u,
    bounds: u32,
    padding: u32,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> nodes: array<Node>;
@group(0) @binding(2) var<storage, read> bricks: array<Brick>;
@group(0) @binding(3) var<storage, read> pigments: array<u32>;
@group(0) @binding(4) var<storage, read> edits: array<vec4i>;
@group(1) @binding(0) var<storage, read> terrain_hits: array<vec2u>;

struct VertexOutput {
    @builtin(position) attachment_position: vec4f,
    @location(0) @interpolate(linear, sample) screen: vec2f,
}

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let corners = array<vec2f, 3>(vec2f(-1., -1.), vec2f(3., -1.), vec2f(-1., 3.));
    let p = corners[vertex_index];
    return VertexOutput(vec4f(p, 0., 1.), p);
}

const INF: f32 = 1e30;
const LEAF: u32 = 0x80000000u;

struct Ray {
    eye: vec3f,
    direction: vec3f,
    inverse: vec3f,
    stationary: vec3<bool>,
}

struct Slab {
    enter: f32,
    exit: f32,
    axis: u32,
}

fn slab(ray: Ray, lower: vec3i, upper: vec3i) -> Slab {
    let lo = vec3f(lower);
    let hi = vec3f(upper);
    let inside = (ray.eye >= lo) & (ray.eye < hi);
    if any( !inside & ray.stationary) {
        return Slab(INF, -INF, 0u);
    }
    let a = (lo - ray.eye) * ray.inverse;
    let b = (hi - ray.eye) * ray.inverse;
    let near = select(min(a, b), vec3f(-INF), ray.stationary);
    let far = select(max(a, b), vec3f(INF), ray.stationary);
    let axis = select(select(2u, 1u, near.y >= near.z), 0u, near.x >= max(near.y, near.z));
    return Slab(near[axis], min(min(far.x, far.y), far.z), axis);
}

fn is_edited(cell: vec3i) -> bool {
    var low = 0u;
    var high = view.counts.y;
    while low < high {
        let mid = (low + high) / 2u;
        let p = edits[mid].xyz;
        if all(p == cell) {
            return true;
        }
        if p.x < cell.x || (p.x == cell.x && (p.y < cell.y || (p.y == cell.y && p.z < cell.z))) {
            low = mid + 1u;
        } else {
            high = mid;
        }
    }
    return false;
}

struct Hit {
    t: f32,
    color: u32,
    normal: vec3f,
}

fn in_brick(ray: Ray, brick: Brick, near: f32, far: f32) -> Hit {
    let axes = vec3u(0, 2, 4);
    let lo = vec3i((vec3u(brick.bounds) >> axes) & vec3u(3));
    let hi = vec3i((vec3u(brick.bounds) >> (axes + vec3u(6))) & vec3u(3)) + vec3i(1);
    let s = slab(ray, brick.min + lo, brick.min + hi);
    let start = max(s.enter, near);
    let stop = min(s.exit, far);
    let miss = Hit(far, 0u, vec3f(0.));
    if start >= stop {
        return miss;
    }

    let origin = ray.eye - vec3f(brick.min);
    let entry = fma(ray.direction, vec3f(start), origin);
    let rounded = floor(entry);
    let step = select(vec3i(1), vec3i(-1), ray.direction < vec3f(0.));
    let from_upper = (step < vec3i(0)) & (entry == rounded);
    var cell = clamp(vec3i(rounded) - select(vec3i(0), vec3i(1), from_upper), lo, hi - vec3i(1));
    let next_face = cell + select(vec3i(0), vec3i(1), step > vec3i(0));
    var next = select((vec3f(next_face) - origin) * ray.inverse, vec3f(INF), ray.stationary);
    let delta = abs(ray.inverse);
    var t = start;
    var hit_axis = s.axis;
    for (var i = 0u; i < 12u; i++) {
        let next_t = min(next.x, min(next.y, next.z));
        // Cells touched only at a line or corner have zero intersection length.
        if t < next_t {
            let bit = u32(cell.x + 4 * cell.y + 16 * cell.z);
            let word = bit / 32u;
            let flag = 1u << (bit % 32u);
            if (brick.mask[word] & flag) != 0u && !is_edited(brick.min + cell) {
                var color = brick.pigment;
                if (color & LEAF) == 0u {
                    let prior_word = select(0u, countOneBits(brick.mask.x), word != 0u);
                    let prior_bits = countOneBits(brick.mask[word] & (flag - 1u));
                    let index = color + prior_word + prior_bits;
                    let palette = pigments[0];
                    if palette == 0u {
                        color = pigments[1u + index];
                    } else {
                        let pair = pigments[1u + index / 2u];
                        color = pigments[palette + ((pair >> (16u * (index % 2u))) & 65535u)];
                    }
                }
                var normal = vec3f(0.);
                normal[hit_axis] = -f32(step[hit_axis]);
                return Hit(t, color, normal);
            }
        }
        if next_t >= stop {
            break;
        }
        let crossed = next <= vec3f(next_t);
        hit_axis = select(select(2u, 1u, crossed.y), 0u, crossed.x);
        cell += select(vec3i(0), step, crossed);
        if any(cell < lo) || any(cell >= hi) {
            break;
        }
        next += select(vec3f(0.), delta, crossed);
        t = max(t, next_t);
    }
    return miss;
}

fn lower_bound(ray: Ray, index: u32, near: f32, far: f32) -> f32 {
    let node = nodes[index];
    let s = slab(ray, node.min, node.max);
    let entry = max(s.enter, near);
    return select(INF, entry, entry < min(far, s.exit));
}

fn trace_ordered(ray: Ray, near: f32, far: f32) -> Hit {
    var closest = Hit(far, 0u, vec3f(0.));
    if lower_bound(ray, 0u, near, far) >= far {
        return closest;
    }
    // The CPU splits by median. With the resident cap, binary tree depth is
    // well under 32, even if every occupied brick needs its own branch.
    var deferred: array<u32, 32>;
    var entries: array<f32, 32>;
    var top = 0u;
    var index = 0u;

    loop {
        let node = nodes[index];
        if (node.end & LEAF) != 0u {
            for (var i = node.right; i < (node.end & ~LEAF); i++) {
                let hit = in_brick(ray, bricks[i], near, closest.t);
                if hit.color != 0u {
                    closest = hit;
                }
            }
        } else {
            let left = index + 1u;
            let right = node.right;
            let a = lower_bound(ray, left, near, closest.t);
            let b = lower_bound(ray, right, near, closest.t);
            if min(a, b) < closest.t {
                if max(a, b) < closest.t {
                    deferred[top] = select(left, right, a <= b);
                    entries[top] = max(a, b);
                    top += 1u;
                }
                index = select(right, left, a <= b);
                continue;
            }
        }
        loop {
            if top == 0u {
                return closest;
            }
            top -= 1u;
            if entries[top] < closest.t {
                index = deferred[top];
                break;
            }
        }
    }
    return closest;
}

// Fixed depth-first, stackless walk. An inner-node miss jumps over its
// whole subtree. Identical leaf tests supply the same nearest intersection.
fn trace_threaded(ray: Ray, near: f32, far: f32) -> Hit {
    var closest = Hit(far, 0u, vec3f(0.));
    var index = 0u;
    while index < view.counts.x {
        let node = nodes[index];
        let leaf = (node.end & LEAF) != 0u;
        if lower_bound(ray, index, near, closest.t) < closest.t {
            if leaf {
                for (var i = node.right; i < (node.end & ~LEAF); i++) {
                    let hit = in_brick(ray, bricks[i], near, closest.t);
                    if hit.color != 0u {
                        closest = hit;
                    }
                }
            }
            index += 1u;
        } else {
            index = select(node.end, index + 1u, leaf);
        }
    }
    return closest;
}

override THREADED: bool = false;

struct FragmentOutput {
    @location(0) color: vec4f,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fragment(input: VertexOutput) -> FragmentOutput {
    let size = vec2f(view.counts.zw);
    // A pixel centered on the viewing axis must produce an exactly zero
    // horizontal offset. Interpolated fullscreen coordinates can perturb it
    // onto the wrong side of a native cell boundary.
    let ndc = (2. * input.attachment_position.xy - size) / size * vec2f(1., -1.);
    let screen = select(ndc, input.screen, view.right.w != 0.);
    let direction = view.forward.xyz + view.right.xyz * screen.x + view.up.xyz * screen.y;
    let stationary = direction == vec3f(0.);
    let inverse = 1. / select(direction, vec3f(1.), stationary);
    let ray = Ray(view.eye.xyz, direction, inverse, stationary);
    let inverse_length = inverseSqrt(dot(direction, direction));
    var far = view.clip.y * inverse_length;
    if view.eye.w != 0. {
        let pixel = vec2u(input.attachment_position.xy);
        let ground = terrain_hits[pixel.y * view.counts.z + pixel.x];
        let meters = bitcast<f32>(ground.x);
        let material = (ground.y >> 4u) & 15u;
        let opaque = view.forward.w == 0. || meters * view.up.w < 0.73 * view.clip.y;
        // The existing hit buffer stores distance along a normalized center
        // ray. Keep a forward error margin and let the scene depth decide ties.
        if meters > 0. && material != 7u && opaque {
            far = min(far, (meters + max(.002, meters * .000002)) * view.up.w * inverse_length);
        }
    }
    var hit: Hit;
    if THREADED {
        hit = trace_threaded(ray, view.clip.x, far);
    } else {
        hit = trace_ordered(ray, view.clip.x, far);
    }
    if hit.color == 0u {
        discard;
    }
    let encoded = unpack4x8unorm(hit.color).rgb;
    let albedo = encoded * encoded;
    let facing_sun = max(0., dot(hit.normal, view.sun.xyz));
    let sky_visibility = clamp(0.5 + 0.5 * hit.normal.y, 0., 1.);
    let light = (0.30 + 0.31 * sky_visibility) * vec3f(0.86, 0.97, 1.12)
        + 0.65 * facing_sun * vec3f(1.07, 1.00, 0.91);
    return FragmentOutput(vec4f(albedo * light, 1.), view.clip.z - view.clip.w / hit.t);
}
