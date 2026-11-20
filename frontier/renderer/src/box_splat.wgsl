// The bounded box-splat and brick-ray probes use the same local-meter camera.
struct Camera {
    matrix: mat4x4<f32>,
    inverse: mat4x4<f32>,
    eye: vec4<f32>,
    extent: vec4<u32>,
    bricks: vec4<u32>,
    viewport: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
// xyz is the local integer cell, w packs material and its six exposed faces.
@group(0) @binding(1) var<storage, read> cells: array<vec4<u32>>;

const CELL_METERS: f32 = 0.0625;
struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) cell: vec4<u32>,
};

fn project(point: vec4<f32>) -> vec2<f32> {
    return point.xy / point.w;
}

// Eye-facing exposed planes are necessary for an accepted slab entry.
fn can_enter_exposed_face(cell: vec4<u32>) -> bool {
    let lower = vec3<f32>(cell.xyz);
    let eye = camera.eye.xyz / CELL_METERS;
    let low_bits = select(vec3<u32>(0u), vec3<u32>(1u, 4u, 16u), eye < lower);
    let high_bits = select(vec3<u32>(0u), vec3<u32>(2u, 8u, 32u), eye > lower + vec3<f32>(1.));
    return any(((low_bits | high_bits) & vec3<u32>(cell.w >> 8u)) != vec3<u32>(0u));
}

struct ProjectedBounds {
    lower: vec2<f32>,
    upper: vec2<f32>,
};

fn projected_bounds(origin: vec4<f32>, dx: vec4<f32>, dy: vec4<f32>, dz: vec4<f32>,
                    clip_lo: vec4<f32>, clip_hi: vec4<f32>) -> ProjectedBounds {
    if clip_lo.z < 0. {
        // Clipped perspective geometry has w >= near_w (near plane z=0).
        // Interval division ignores x/y/w correlations and therefore encloses
        // every point in the clipped hull, including off-screen crossing boxes.
        let near_w = 1. / camera.inverse[3].w;
        let w_min = max(near_w, clip_lo.w) * 0.9999;
        let w_max = max(near_w, clip_hi.w) * 1.0001;
        let lower = clip_lo.xy / select(vec2<f32>(w_max), vec2<f32>(w_min), clip_lo.xy < vec2<f32>(0.));
        let upper = clip_hi.xy / select(vec2<f32>(w_max), vec2<f32>(w_min), clip_hi.xy > vec2<f32>(0.));
        return ProjectedBounds(lower, upper);
    }
    let a = project(origin);
    let b = project(origin + dx);
    let c = project(origin + dy);
    let d = project(origin + dx + dy);
    let e = project(origin + dz);
    let f = project(origin + dx + dz);
    let g = project(origin + dy + dz);
    let h = project(origin + dx + dy + dz);
    return ProjectedBounds(
        min(min(min(a, b), min(c, d)), min(min(e, f), min(g, h))),
        max(max(max(a, b), max(c, d)), max(max(e, f), max(g, h))),
    );
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Vertex {
    let cell = cells[instance];
    if !can_enter_exposed_face(cell) {
        return Vertex(vec4<f32>(2., 2., 0., 1.), cell);
    }
    let rect_corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let origin = camera.matrix * vec4<f32>(vec3<f32>(cell.xyz) * CELL_METERS, 1.);
    let dx = camera.matrix[0] * CELL_METERS;
    let dy = camera.matrix[1] * CELL_METERS;
    let dz = camera.matrix[2] * CELL_METERS;
    let clip_lo = origin + min(dx, vec4<f32>(0.)) + min(dy, vec4<f32>(0.)) + min(dz, vec4<f32>(0.));
    let clip_hi = origin + max(dx, vec4<f32>(0.)) + max(dy, vec4<f32>(0.)) + max(dz, vec4<f32>(0.));
    let far_low = origin.z - origin.w + min(dx.z - dx.w, 0.) + min(dy.z - dy.w, 0.) + min(dz.z - dz.w, 0.);
    if clip_hi.z < 0. || far_low > 0. {
        return Vertex(vec4<f32>(2., 2., 0., 1.), cell);
    }
    let bounds = projected_bounds(origin, dx, dy, dz, clip_lo, clip_hi);
    // Round out the candidate bounds before rasterizer subpixel snapping. A
    // wider rectangle cannot change the geometry accepted by the slab test.
    let pad = 0.25 / camera.viewport.xy;
    let lo = clamp(bounds.lower - pad, vec2<f32>(-1.), vec2<f32>(1.));
    let hi = clamp(bounds.upper + pad, vec2<f32>(-1.), vec2<f32>(1.));
    let ndc = mix(lo, hi, rect_corner);
    return Vertex(vec4<f32>(ndc, 0., 1.), cell);
}

struct Fragment {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// Same linear material colors and face lighting as the bounded brick-ray probe.
fn material_color(id: u32) -> vec3<f32> {
    switch id {
        case 1u: { return vec3<f32>(0.18, 0.34, 0.10); }
        case 2u: { return vec3<f32>(0.32, 0.22, 0.13); }
        case 3u: { return vec3<f32>(0.35, 0.37, 0.39); }
        case 4u: { return vec3<f32>(0.41, 0.25, 0.13); }
        case 5u: { return vec3<f32>(0.40, 0.53, 0.25); }
        case 6u: { return vec3<f32>(0.93, 0.92, 0.84); }
        case 7u: { return vec3<f32>(0.035, 0.19, 0.29); }
        case 8u: { return vec3<f32>(0.62, 0.52, 0.32); }
        case 9u: { return vec3<f32>(0.88, 0.92, 0.96); }
        default: { return vec3<f32>(0.); }
    }
}

fn terrain_lighting(normal: vec3<f32>) -> vec3<f32> {
    let sun = normalize(vec3<f32>(-0.5, 0.8, -0.3));
    let sky = 0.5 + 0.5 * normal.y;
    let ambient = mix(vec3<f32>(0.12, 0.105, 0.085), vec3<f32>(0.28, 0.33, 0.40), sky);
    return ambient + vec3<f32>(0.78, 0.70, 0.58) * max(dot(normal, sun), 0.);
}

@fragment
fn fs_main(vertex: Vertex) -> Fragment {
    let ndc = vec2<f32>(vertex.position.x / camera.viewport.x * 2. - 1.,
                        1. - vertex.position.y / camera.viewport.y * 2.);
    let mid = camera.inverse * vec4<f32>(ndc, 0.5, 1.);
    let direction = normalize(mid.xyz / mid.w - camera.eye.xyz);
    let eye = camera.eye.xyz / CELL_METERS;
    let lower = vec3<f32>(vertex.cell.xyz);
    let upper = lower + vec3<f32>(1.);
    let parallel = direction == vec3<f32>(0.);
    // Use the half-open cell convention for axis-parallel rays on a grid plane.
    if any(parallel & ((eye < lower) | (eye >= upper))) {
        discard;
    }
    let divisor = select(direction, vec3<f32>(1.), parallel);
    let first = (lower - eye) / divisor;
    let second = (upper - eye) / divisor;
    let entry = select(min(first, second), vec3<f32>(-1e30), parallel);
    let exit = select(max(first, second), vec3<f32>(1e30), parallel);
    let near = max(entry.x, max(entry.y, entry.z));
    let far = min(exit.x, min(exit.y, exit.z));
    if near >= far || near <= 0. {
        discard;
    }

    let entered_faces = select(vec3<u32>(1u, 3u, 5u), vec3<u32>(0u, 2u, 4u), direction > vec3<f32>(0.));
    let exposed = ((vec3<u32>(vertex.cell.w >> 8u) >> entered_faces) & vec3<u32>(1u)) != vec3<u32>(0u);
    let hits = (entry == vec3<f32>(near)) & exposed;
    if !any(hits) {
        discard;
    }
    // At a shared edge, pick the lowest exposed entering axis uniformly with
    // brick-ray. An unexposed face must never cap a cropped volume or a cave.
    let axis = select(select(vec3<bool>(false, false, true), vec3<bool>(false, true, false), hits.y),
                      vec3<bool>(true, false, false), hits.x);
    let normal = select(vec3<f32>(0.), select(vec3<f32>(1.), vec3<f32>(-1.), direction > vec3<f32>(0.)), axis);
    let hit = (eye + direction * near) * CELL_METERS;
    let clip = camera.matrix * vec4<f32>(hit, 1.);
    let depth = clip.z / clip.w;
    if depth < 0. || depth > 1. {
        discard;
    }
    return Fragment(vec4<f32>(material_color(vertex.cell.w & 255u) * terrain_lighting(normal), 1.), depth);
}
