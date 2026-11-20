// Interior column runs for the adaptive terrain proxy. The existing raster
// path owns boundary strips and publishes the same certified tile bounds.
fn coarse_triangle_corner(vertex: u32) -> u32 {
    let local = vertex % 6u;
    return select(select(local, local - 2u, local > 3u), 0u, local == 3u);
}

@vertex
fn vs_coarse_generated(@builtin(vertex_index) vertex: u32,
    @location(0) xz: vec4<f32>, @location(1) levels: vec2<f32>,
    @location(2) floors: vec4<f32>, @location(3) style: vec3<u32>) -> VertexOut {
    camera = draw_camera;
    let face = vertex / 6u;
    let corner = coarse_triangle_corner(vertex);
    if face == 0u {
        return native_face(corner, vec3<f32>(xz.x, levels.x, xz.y),
                           vec2<f32>(xz.w - xz.y, xz.z - xz.x), 1u, style.x, style.z);
    }

    let side = (face - 1u) / 2u;
    let deep = (face & 1u) != 0u;
    let positive = (side & 1u) != 0u;
    let along_x = side >= 2u;
    let floor = select(select(floors.x, floors.y, positive), select(floors.z, floors.w, positive), along_x);
    let split = clamp(levels.y, floor, levels.x);
    let y0 = select(split, floor, deep);
    let y1 = select(levels.x, split, deep);
    if y1 <= y0 {
        return VertexOut(vec4<f32>(2., 2., 0., 1.), vec3<f32>(0.), vec3<f32>(0.), vec3<f32>(0.), 0u, 0u);
    }
    let anchor = select(vec3<f32>(select(xz.x, xz.z, positive), y0, xz.y),
                        vec3<f32>(xz.x, y0, select(xz.y, xz.w, positive)), along_x);
    let extent = select(vec2<f32>(y1 - y0, xz.w - xz.y), vec2<f32>(xz.z - xz.x, y1 - y0), along_x);
    let axis_sign = select(0u, 2u, along_x) | select(4u, 0u, positive);
    return native_face(corner, anchor, extent, axis_sign, select(style.x, style.y, deep), style.z);
}

struct CoarseSplatVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) xz: vec4<f32>,
    @location(1) @interpolate(flat) levels: vec2<f32>,
    @location(2) @interpolate(flat) floors: vec4<f32>,
    @location(3) @interpolate(flat) style: vec3<u32>,
};

fn coarse_box_bottom(levels: vec2<f32>, floors: vec4<f32>) -> f32 {
    // Roof-only runs still need a nonempty slab. The added depth can never
    // produce a side hit because all four side floors coincide with the roof.
    return min(levels.x - 0.0625, min(min(floors.x, floors.y), min(floors.z, floors.w)));
}

fn coarse_ndc(point: vec4<f32>) -> vec2<f32> {
    return point.xy / point.w;
}

struct CoarseProjectionBounds {
    lo: vec2<f32>,
    hi: vec2<f32>,
};

fn coarse_projected_bounds(origin: vec4<f32>, dx: vec4<f32>, dy: vec4<f32>, dz: vec4<f32>,
                           clip_lo: vec4<f32>, clip_hi: vec4<f32>, near_w: f32) -> CoarseProjectionBounds {
    if clip_lo.z < 0. || clip_lo.w <= 0. {
        if near_w <= 0. {
            // A nearly singular matrix still has exact visibility in the slab
            // test. Use full screen bounds when projection cannot bound it.
            return CoarseProjectionBounds(vec2<f32>(-1.), vec2<f32>(1.));
        }
        // Intervals ignore x/y/w correlations. Sign-aware division bounds
        // every point left after the near plane clips this box.
        let min_w = max(near_w, clip_lo.w) * 0.9999;
        let max_w = max(near_w, clip_hi.w) * 1.0001;
        let lo = clip_lo.xy / select(vec2<f32>(max_w), vec2<f32>(min_w), clip_lo.xy < vec2<f32>(0.));
        let hi = clip_hi.xy / select(vec2<f32>(max_w), vec2<f32>(min_w), clip_hi.xy > vec2<f32>(0.));
        return CoarseProjectionBounds(lo, hi);
    }

    let a = coarse_ndc(origin);
    let b = coarse_ndc(origin + dx);
    let c = coarse_ndc(origin + dy);
    let d = coarse_ndc(origin + dx + dy);
    let e = coarse_ndc(origin + dz);
    let f = coarse_ndc(origin + dx + dz);
    let g = coarse_ndc(origin + dy + dz);
    let h = coarse_ndc(origin + dx + dy + dz);
    return CoarseProjectionBounds(
        min(min(min(a, b), min(c, d)), min(min(e, f), min(g, h))),
        max(max(max(a, b), max(c, d)), max(max(e, f), max(g, h))),
    );
}

@vertex
fn vs_coarse_splat(@builtin(vertex_index) vertex: u32,
    @location(0) xz: vec4<f32>, @location(1) levels: vec2<f32>,
    @location(2) floors: vec4<f32>, @location(3) style: vec3<u32>) -> CoarseSplatVertex {
    let eye = draw_camera.eye.xyz;
    let top = levels.x;
    let eye_facing = eye.y > top
        || (eye.x < xz.x && floors.x < top) || (eye.x > xz.z && floors.y < top)
        || (eye.z < xz.y && floors.z < top) || (eye.z > xz.w && floors.w < top);
    let rejected = CoarseSplatVertex(vec4<f32>(2., 2., 0., 1.), xz, levels, floors, style);
    if !eye_facing {
        return rejected;
    }

    let bottom = coarse_box_bottom(levels, floors);
    let matrix = draw_camera.view_projection;
    let origin = matrix * vec4<f32>(xz.x, bottom, xz.y, 1.);
    let dx = matrix[0] * (xz.z - xz.x);
    let dy = matrix[1] * (top - bottom);
    let dz = matrix[2] * (xz.w - xz.y);
    let clip_lo = origin + min(dx, vec4<f32>(0.)) + min(dy, vec4<f32>(0.)) + min(dz, vec4<f32>(0.));
    let clip_hi = origin + max(dx, vec4<f32>(0.)) + max(dy, vec4<f32>(0.)) + max(dz, vec4<f32>(0.));
    if clip_hi.z < 0. || clip_hi.w <= 0. {
        return rejected;
    }
    var near_w = 0.;
    if clip_lo.z < 0. || clip_lo.w <= 0. {
        // Camera::matrix uses a 0.025 m near plane. A wide margin keeps this
        // denominator conservative when the tile origin is far from the eye.
        near_w = 0.0125;
    }
    let bounds = coarse_projected_bounds(origin, dx, dy, dz, clip_lo, clip_hi, near_w);
    // Candidate rectangle padding absorbs rasterizer subpixel snapping. The
    // slab still decides coverage and emits the true face's depth.
    let padding = vec2<f32>(0.00025);
    let lo = clamp(bounds.lo - padding, vec2<f32>(-1.), vec2<f32>(1.));
    let hi = clamp(bounds.hi + padding, vec2<f32>(-1.), vec2<f32>(1.));
    let corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let ndc = mix(lo, hi, corner);
    return CoarseSplatVertex(vec4<f32>(ndc, 0., 1.), xz, levels, floors, style);
}

struct CoarseSplatFragment {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

fn coarse_camera_ray(ndc: vec2<f32>, matrix: mat4x4<f32>) -> vec3<f32> {
    // The perspective camera has mutually orthogonal scaled basis rows x/y/w.
    // Undo horizontal/vertical scale to obtain a direction with clip w rate 1.
    let rows = transpose(matrix);
    let x = rows[0].xyz;
    let y = rows[1].xyz;
    let w = rows[3].xyz;
    return x * (ndc.x / dot(x, x)) + y * (ndc.y / dot(y, y)) + w / dot(w, w);
}

@fragment
fn fs_coarse_splat(in: CoarseSplatVertex) -> CoarseSplatFragment {
    camera = draw_camera;
    // Every candidate uses the same ray at this pixel. Interpolating its
    // rectangle's NDC through subpixel-snapped vertices can round differently
    // on opposite sides of a shared terrain edge.
    let unit_pixel = in.position.xy / camera.viewport.xy;
    let ndc = vec2<f32>(2. * unit_pixel.x - 1., 1. - 2. * unit_pixel.y);
    let direction = coarse_camera_ray(ndc, camera.view_projection);
    let eye = camera.eye.xyz;
    let lo = vec3<f32>(in.xz.x, coarse_box_bottom(in.levels, in.floors), in.xz.y);
    let hi = vec3<f32>(in.xz.z, in.levels.x, in.xz.w);
    let parallel = direction == vec3<f32>(0.);
    if any(parallel & ((eye < lo) | (eye >= hi))) {
        discard;
    }
    let forward = direction > vec3<f32>(0.);
    let enter_plane = select(hi, lo, forward);
    let exit_plane = select(lo, hi, forward);
    let divisor = select(direction, vec3<f32>(1.), parallel);
    let entry = select((enter_plane - eye) / divisor, vec3<f32>(-1e30), parallel);
    let exit = select((exit_plane - eye) / divisor, vec3<f32>(1e30), parallel);
    let t = max(entry.x, max(entry.y, entry.z));
    if t <= 0. || t >= min(exit.x, min(exit.y, exit.z)) {
        discard;
    }

    var hit = eye + direction * t;
    let floor_x = select(in.floors.y, in.floors.x, forward.x);
    let floor_z = select(in.floors.w, in.floors.z, forward.z);
    // Shared slab edges may enter more than one face at the same depth. A
    // missing/covered face must not mask a tied exposed face.
    let side_x = entry.x == t && floor_x < hi.y && hit.y >= floor_x;
    let roof = entry.y == t && !forward.y;
    let side_z = entry.z == t && floor_z < hi.y && hit.y >= floor_z;
    if !(side_x || roof || side_z) {
        discard;
    }
    let chosen = vec3<bool>(side_x, !side_x && roof, !side_x && !roof);
    let normal = select(vec3<f32>(0.), select(vec3<f32>(1.), vec3<f32>(-1.), forward), chosen);
    hit = select(hit, enter_plane, chosen);
    let clip = camera.view_projection * vec4<f32>(hit, 1.);
    if clip.z < 0. || clip.z > clip.w || clip.w <= 0. {
        discard;
    }
    let material = select(in.style.x, in.style.y, !chosen.y && hit.y < in.levels.y);
    let shaded = VertexOut(in.position, MATERIAL_COLORS[min(material, 9u)], normal, hit, in.style.z, 0u);
    return CoarseSplatFragment(shade(shaded), clip.z / clip.w);
}
