// The hierarchy is made exclusively from the rendered surface's own columns.
// It certifies empty space for a bundle. Every remaining ray uses the same
// exact leaf intersection as the single-ray control.
override FOOTPRINT_SAMPLES: u32 = 2u; // samples per axis
override ADAPTIVE_FOOTPRINT: bool = false;
override TILE_PROOF: bool = false;
override SILHOUETTE_ONLY: bool = false;
override FILTER_FACES: bool = false;
var<workgroup> tile_prefix: f32;
var<workgroup> tile_empty: bool;

fn empty_slab(directions: array<vec3f,4>, start: f32, end: f32) -> bool {
    let grid = outer_grid();
    let origin = (view.eye.xz - grid.extent.xy) * grid.extent.z;
    var lower = vec2f(INF);
    var upper = vec2f(-INF);
    var lowest_y = INF;
    for (var i = 0u; i < 4u; i += 1u) {
        let d = directions[i];
        let a = fma(vec2f(start), d.xz * grid.extent.z, origin);
        let b = fma(vec2f(end), d.xz * grid.extent.z, origin);
        lower = min(lower, min(a, b));
        upper = max(upper, max(a, b));
        lowest_y = min(lowest_y, min(fma(start, d.y, view.eye.y), fma(end, d.y, view.eye.y)));
    }
    // Outward padding for the finite experimental grid and its f32 camera.
    // A slab outside the grid is empty in the finite fixture.
    let pad = vec2f(.125);
    lower -= pad;
    upper += pad;
    if any(upper < vec2f(0.)) || any(lower >= vec2f(grid.extent.w)) { return true; }
    let last = vec2f(grid.extent.w - 1.);
    let lo = vec2i(clamp(floor(lower), vec2f(0.), last));
    let hi = vec2i(clamp(floor(upper), vec2f(0.), last));
    // Pick a mip where the rectangle overlaps at most 2x2 cells.
    let span = hi - lo;
    let span_cells = u32(max(span.x, span.y));
    let mip = select(0u, firstLeadingBit(span_cells) + 1u, span_cells != 0u);
    let a = vec2i(lo >> vec2u(mip));
    let b = vec2i(hi >> vec2u(mip));
    let h = max(
        max(bounds_at(grid, a, mip).x, bounds_at(grid, vec2i(a.x,b.y), mip).x),
        max(bounds_at(grid, vec2i(b.x,a.y), mip).x, bounds_at(grid, b, mip).x)
    );
    return lowest_y > h + .002;
}

fn prove_tile(pixel: vec2u) {
    let lo = vec2f(pixel);
    let hi = vec2f(min(pixel + vec2u(8), view.size.xy));
    let corners = array(
        screen_ray_at(lo), screen_ray_at(vec2f(lo.x, hi.y)),
        screen_ray_at(vec2f(hi.x, lo.y)), screen_ray_at(hi)
    );
    // The unnormalized camera rays all reach the far plane by eye.w, also
    // when the actual ray is clipped radially. Start at zero for the proof.
    tile_prefix = 0.;
    tile_empty = false;
    let far = view.eye.w * select(1., 1.00001, RADIAL_CLIP);
    var span = 4.;
    for (var step = 0u; step < 24u; step += 1u) {
        let end = min(tile_prefix + span, far);
        if empty_slab(corners, tile_prefix, end) {
            tile_prefix = end;
            if end >= far {
                tile_empty = true;
                return;
            }
            span *= 2.;
        } else {
            span *= .5;
            if span < .125 { return; }
        }
    }
}

fn needs_subsamples(pixel: vec2u, hit: vec2f, d: vec3f) -> bool {
    // This is a selection heuristic for terrain pixels. Only the empty
    // tile test is a certificate for the whole footprint.
    if hit.x == -2. { return true; }
    if !SILHOUETTE_ONLY {
        if hit.x < 0. { return true; }
        let width = hit.x * 2. * length(view.up.xyz) / f32(view.size.y);
        if abs(d.y) < .10 && width > .018 { return true; }
    }
    let offsets = array(vec2i(-1,0), vec2i(1,0), vec2i(0,-1), vec2i(0,1));
    for (var i = 0u; i < 4u; i += 1u) {
        let coord = vec2u(clamp(vec2i(pixel) + offsets[i], vec2i(0), vec2i(view.size.xy)-1));
        let neighbor = hits[coord.y * view.size.x + coord.x];
        if (neighbor.x >= 0.) != (hit.x >= 0.) { return true; }
        if hit.x < 0. { continue; }
        let depth_gap = select(max(.12, .03 * hit.x), max(.5, .12 * hit.x), SILHOUETTE_ONLY);
        if abs(neighbor.x - hit.x) > depth_gap || (!SILHOUETTE_ONLY && neighbor.y != hit.y) {
            return true;
        }
    }
    return false;
}

fn footprint_offset(i: u32) -> vec2f {
    if FOOTPRINT_SAMPLES == 2u {
        let rotated = array(vec2f(.375,.125), vec2f(.875,.375), vec2f(.125,.625), vec2f(.625,.875));
        return rotated[i];
    }
    let row = vec2u(i % FOOTPRINT_SAMPLES, i / FOOTPRINT_SAMPLES);
    return (vec2f(row) + .5) / f32(FOOTPRINT_SAMPLES);
}

fn column_height(grid:Grid, cell:vec2i) -> f32 {
    return bounds_at(grid,clamp(cell,vec2i(0),vec2i(i32(grid.extent.w)-1)),0u).x;
}

fn footprint_color(hit:vec2f,d:vec3f) -> vec3f {
    let actual = terrain_color(hit,d);
    if !FILTER_FACES || hit.x < 0. { return actual; }
    let grid = outer_grid();
    let width = hit.x * 2. * length(view.up.xyz) / f32(view.size.y);
    let view_ray = normalize(d);
    let radius = clamp(width * grid.extent.z / max(.15,abs(view_ray.y)), 2., 48.);
    let p = fma(vec3f(hit.x),d,view.eye.xyz);
    let cell = vec2i(floor((p.xz-grid.extent.xy)*grid.extent.z));
    let step = i32(round(radius));
    let dx = vec2i(step,0);
    let dz = vec2i(0,step);
    let inv_span = grid.extent.z / f32(2 * step);
    let slope = inv_span * vec2f(
        column_height(grid,cell+dx)-column_height(grid,cell-dx),
        column_height(grid,cell+dz)-column_height(grid,cell-dz)
    );
    let mean_normal = normalize(vec3f(-slope.x,1.,-slope.y));
    let footprint_cells = width * grid.extent.z / max(.12,abs(dot(view_ray,mean_normal)));
    let blend = smoothstep(.7,2.6,footprint_cells);
    if blend == 0. { return actual; }
    // Mean projected areas of the three voxel face orientations for a local
    // slope. Use this only for lighting and material, after exact visibility.
    let nx = vec3f(sign(-slope.x),0.,0.);
    let nz = vec3f(0.,0.,sign(-slope.y));
    let y_weight = max(0.,-view_ray.y);
    let x_weight = abs(slope.x)*max(0.,-dot(view_ray,nx));
    let z_weight = abs(slope.y)*max(0.,-dot(view_ray,nz));
    let area = x_weight + y_weight + z_weight;
    if area < .0001 { return actual; }
    let mean = (
        x_weight * ground_color(p,nx,hit.x) +
        y_weight * ground_color(p,vec3f(0.,1.,0.),hit.x) +
        z_weight * ground_color(p,nz,hit.x)
    ) / area;
    return mix(actual,mean,blend);
}

@compute @workgroup_size(8,8)
fn shade_footprint(
    @builtin(global_invocation_id) id: vec3u,
    @builtin(local_invocation_index) lane: u32,
) {
    if TILE_PROOF {
        if lane == 0u { prove_tile(id.xy); }
        workgroupBarrier();
    }
    if any(id.xy >= view.size.xy) { return; }
    let index = id.y * view.size.x + id.x;
    let center = hits[index];
    let d = screen_ray(id.xy);
    let certified_sky = TILE_PROOF && tile_empty && center.x == -1.;
    if certified_sky || (ADAPTIVE_FOOTPRINT && !needs_subsamples(id.xy, center, d)) {
        colors[index] = pack_pixel(footprint_color(center,d), select(0., 1., center.x >= 0.));
        if INSTRUMENT && tile_empty { counters[index].w = 1u; }
        return;
    }
    let count = FOOTPRINT_SAMPLES * FOOTPRINT_SAMPLES;
    var rgb = vec3f(0.);
    var covered = 0u;
    var stats = vec4u(0);
    var failed = false;
    let grid = outer_grid();
    for (var i = 0u; i < count; i += 1u) {
        let ray = screen_ray_at(vec2f(id.xy) + footprint_offset(i));
        let scale = clip_scale(ray);
        var near = NEAR * scale;
        if TILE_PROOF { near = max(near,tile_prefix); }
        let hit = trace(ray, ray_grid(ray.xz, grid, near, view.eye.w * scale), &stats);
        rgb += footprint_color(hit,ray);
        covered += u32(hit.x >= 0.);
        failed |= hit.x == -2.;
    }
    if INSTRUMENT {
        let base = counters[index];
        counters[index] = vec4u(base.xy + stats.xy, count, 0u);
    }
    colors[index] = select(
        pack_pixel(rgb / f32(count), f32(covered) / f32(count)),
        0x00ff00ffu, // unresolved sentinel in the diagnostic alpha channel
        failed
    );
}
