// A perspective ray in forward-depth coordinates is affine in pixel x/y.
// Over a depth slab the four corner rays at both endpoints therefore enclose
// every ray in the packet. Bounds prove a contiguous empty prefix only.
const PACKET_EDGE: u32 = 8u;
@group(0) @binding(5) var<storage, read_write> packets: array<vec4f>;
override USE_PACKETS: bool = false;

fn packet_upper(lo: vec2f, hi: vec2f) -> f32 {
    // Pick the finest level for which the rectangle can span at most 2x2 cells.
    for (var level = 0u; level < 4u; level += 1u) {
        let cell = array<f32,4>(20., 80., 320., 1280.)[level];
        if level == 0u && !FINE_BOUNDS { continue; }
        let subdivisions = array<i32,4>(4, 1, 1, 1)[level];
        let divisor = array<i32,4>(1, 1, 4, 16)[level];
        let size = i32(SIDE) * subdivisions / divisor;
        let padding = vec2(0.005);
        let origin = vec2f(view.grid.xy) * BLOCK;
        let first = vec2i(floor((lo - padding - origin) / cell));
        let last = vec2i(floor((hi + padding - origin) / cell));
        if any(first < vec2i(0)) || any(last >= vec2i(size)) { return INF; }
        if any(last - first > vec2i(1)) { continue; }
        let offset = array<u32,4>(FINE_OFFSET, 0u, MEDIUM, COARSE)[level];
        let a = u32(first.x + first.y * size);
        let b = u32(last.x + first.y * size);
        let c = u32(first.x + last.y * size);
        let d = u32(last.x + last.y * size);
        return max(max(bounds[offset+a], bounds[offset+b]),
                   max(bounds[offset+c], bounds[offset+d]));
    }
    return INF;
}

fn packet_index(packet: vec2u) -> u32 {
    return packet.x + packet.y * ((view.image.x + PACKET_EDGE - 1u) / PACKET_EDGE);
}

@compute @workgroup_size(8,8)
fn packet_prefix(@builtin(global_invocation_id) id: vec3u) {
    let first = id.xy * PACKET_EDGE + view.image.zw;
    if first.x >= view.image.x || first.y >= view.image.y
        || id.y * PACKET_EDGE >= u32(view.grid.z)
        || id.x * PACKET_EDGE >= u32(view.controls.w) { return; }
    let last = min(first + PACKET_EDGE - 1u, view.image.xy - 1u);
    let a = (2. * (vec2f(first) + .5)) / vec2f(view.image.xy) - 1.;
    let b = (2. * (vec2f(last) + .5)) / vec2f(view.image.xy) - 1.;
    let q = view.forward.xyz + .5 * (a.x + b.x) * view.right.xyz
                            - .5 * (a.y + b.y) * view.up.xyz;
    let radius = .5 * ((b.x-a.x) * abs(view.right.xyz) + (b.y-a.y) * abs(view.up.xyz));
    let fast = q + radius;
    let slow = q - radius;
    let eye = view.eye.xyz;
    let far = view.eye.w;
    var depth = .025;
    var step = 320.;
    var tests = 0.;
    var analytic_tests = 0.;
    var sky = 0.;
    for (var i=0u; i<96u; i+=1u) {
        let next = min(depth + step, far);
        // Spatial guard covers the f32 projection and forward conversion.
        let guard = .005 + next * .000003;
        let lo = min(fma(vec3(depth),slow,eye), fma(vec3(next),slow,eye)) - guard;
        let hi = max(fma(vec3(depth),fast,eye), fma(vec3(next),fast,eye)) + guard;
        var top = packet_upper(lo.xz, hi.xz);
        tests += 1.;
        if lo.y <= top + PAD && step <= 40. {
            top = min(top, rect_upper((lo.xz+hi.xz)*.5, (hi.xz-lo.xz)*.5));
            analytic_tests += 1.;
        }
        if lo.y > top + PAD {
            depth = next;
            if depth >= far {
                // Certify the tiny projection guard beyond the rendered clip
                // within the enclosure above. The GPU output stays sky.
                sky = 1.;
                break;
            }
            step = min(step*2., 640.);
        } else {
            if step <= 5. { break; }
            step *= .5;
        }
    }
    packets[packet_index(first/PACKET_EDGE)] = vec4(max(.025,depth-.01), tests, analytic_tests, sky);
}

// Keep the first independent leaf's original ray interval. Beginning inside
// the leaf changes the subdivisions used by bounded numerical repair and can
// miss a narrow crossing that the independent ray sampled.
fn aligned_packet_start(eye:vec3f, d:vec3f, prefix:f32, near:f32) -> f32 {
    let cell=select(BLOCK,BLOCK/4.,FINE_BOUNDS);
    let pos=fma(d.xz,vec2(prefix),eye.xz);
    let corner=floor(pos/cell);
    let entry_edge=(corner+select(vec2(1.),vec2(0.),d.xz>vec2(0.)))*cell;
    let entry=select((entry_edge-eye.xz)/d.xz,vec2(-INF),abs(d.xz)<vec2(1.e-10));
    // Start just before the boundary so the same DDA edge update handles
    // round-off and ties. The extra segment remains inside the empty prefix.
    return max(near, min(prefix,max(entry.x,entry.y)-.02));
}

// Preview mode. Four primary rays per packet supply reciprocal forward depths,
// which vary linearly on a projected plane. A local true-height sign bracket
// verifies a surface crossing. Only the empty prefix can certify first-hit
// visibility; a different earlier crossing can exist for other predictions.
struct Anchor {
    depth: vec4f,
    work: vec4u,
}
@group(0) @binding(6) var<storage, read_write> anchors: array<Anchor>;
override SPARSE_MODE: u32 = 0u;

fn packet_corners(first: vec2u, last: vec2u, corner: u32) -> vec2u {
    return select(first, last, (vec2u(corner)&vec2u(1u,2u))!=vec2u(0u));
}
fn pixel_ray(pixel: vec2u) -> vec3f {
    let uv=2.*(vec2f(pixel)+.5)/vec2f(view.image.xy)-1.;
    return normalize(view.forward.xyz+uv.x*view.right.xyz-uv.y*view.up.xyz);
}

@compute @workgroup_size(8,8)
fn trace_anchors(@builtin(global_invocation_id) id: vec3u) {
    let first = vec2u(id.x/4u,id.y) * PACKET_EDGE + view.image.zw;
    if first.x >= view.image.x || first.y >= view.image.y
        || id.y * PACKET_EDGE >= u32(view.grid.z)
        || (id.x/4u) * PACKET_EDGE >= u32(view.controls.w) { return; }
    let packet = packet_index(first/PACKET_EDGE);
    let last = min(first + PACKET_EDGE - 1u, view.image.xy - 1u);
    let d = pixel_ray(packet_corners(first,last,id.x%4u));
    let forward = dot(d,view.forward.xyz);
    var work=vec4u(0);
    var t=-1.;
    let prefix=packets[packet];
    if prefix.w!=1. {
        let start=aligned_packet_start(view.eye.xyz,d,prefix.x/forward,.025/forward);
        t=trace(view.eye.xyz,d,start,view.eye.w/forward,&work);
    }
    anchors[packet*4u+id.x%4u] = Anchor(vec4(t*forward,0.,0.,0.),work);
}

fn preview_intersection(ij:vec2u, d:vec3f, forward:f32, prefix:f32,
                        counts:ptr<function,vec4u>) -> vec2f {
    let packet=packet_index(ij/PACKET_EDGE)*4u;
    let depth=vec4(anchors[packet].depth.x,anchors[packet+1u].depth.x,
                   anchors[packet+2u].depth.x,anchors[packet+3u].depth.x);
    let near=min(min(depth.x,depth.y),min(depth.z,depth.w));
    let far=max(max(depth.x,depth.y),max(depth.z,depth.w));
    if near<=0. || far-near>2.+.085*near { return vec2(-1.,0.); }
    let first=ij/PACKET_EDGE*PACKET_EDGE;
    let last=min(first+PACKET_EDGE-1u,view.image.xy-1u);
    let uv=vec2f(ij-first)/vec2f(max(vec2u(1),last-first));
    let reciprocal=1./depth;
    let t=1./(forward*mix(mix(reciprocal.x,reciprocal.y,uv.x),
                           mix(reciprocal.z,reciprocal.w,uv.x),uv.y));
    let tolerance=clamp(t*view.controls.x,.008,4.);
    let exact_start=prefix/forward;
    let exact_end=view.eye.w/forward;
    var left=max(exact_start,t-.5*tolerance);
    var right=min(exact_end,t+.5*tolerance);
    if right<=left {return vec2(-1.,0.);}
    let a=gap_at(view.eye.xyz,d,left,counts);
    let b=gap_at(view.eye.xyz,d,right,counts);
    if a>PAD && b < -PAD {
        return vec2(right,select(1.,2.,left<=exact_start));
    }
    // A guarded secant corrects the plane prediction. Propagate no crossing
    // unless the canonical full-height function changes sign locally.
    let decline=(b-a)/(right-left);
    if decline >= -.035 { return vec2(-1.,0.); }
    let estimated=left-a/decline;
    if abs(estimated-t)>max(.15,tolerance*2.) {return vec2(-1.,0.);}
    left=max(exact_start,estimated-.5*tolerance);
    right=min(exact_end,estimated+.5*tolerance);
    if right<=left {return vec2(-1.,0.);}
    if gap_at(view.eye.xyz,d,left,counts)>PAD && gap_at(view.eye.xyz,d,right,counts)<-PAD {
        return vec2(right,select(1.,2.,left<=exact_start));
    }
    return vec2(-1.,0.);
}

override COMPACT: bool = false;
struct FallbackQueue {
    length: atomic<u32>,
    indices: array<u32>,
}
@group(0) @binding(7) var<storage, read_write> fallback: FallbackQueue;
var<workgroup> pending_pixels: array<u32,64>;
var<workgroup> pending_count: atomic<u32>;
var<workgroup> pending_offset: u32;

fn begin_pending(lane:u32) {
    if lane==0u { atomicStore(&pending_count,0u); }
    workgroupBarrier();
}
fn push_pending(index:u32) {
    pending_pixels[atomicAdd(&pending_count,1u)]=index;
}
fn flush_pending(lane:u32) {
    workgroupBarrier();
    let count=atomicLoad(&pending_count);
    if lane==0u && count>0u { pending_offset=atomicAdd(&fallback.length,count); }
    workgroupBarrier();
    if lane<count { fallback.indices[pending_offset+lane]=pending_pixels[lane]; }
}
fn valid_tile_pixel(id:vec2u, ij:vec2u) -> bool {
    return id.x<u32(view.controls.w) && ij.x<view.image.x && ij.y<view.image.y;
}

// Separate the cheap predictions from the expensive solver so every wave in
// the latter contains only pending rays. Reserve one span per workgroup.
@compute @workgroup_size(8,8)
fn queue_preview(@builtin(global_invocation_id) id:vec3u,
                 @builtin(local_invocation_index) lane:u32) {
    begin_pending(lane);
    let ij=id.xy+view.image.zw;
    if valid_tile_pixel(id.xy,ij) {
        let index=ij.y*view.image.x+ij.x;
        let d=pixel_ray(ij);
        let packet=packets[packet_index(ij/PACKET_EDGE)];
        var counts=vec4u(0);
        if packet.w==1. {
            store_pixel(index,d,-1.,0.);
        } else {
            let preview=preview_intersection(ij,d,dot(d,view.forward.xyz),packet.x,&counts);
            if preview.y==2. || (SPARSE_MODE==2u && preview.y==1.) {
                store_pixel(index,d,preview.x,preview.y);
            } else {
                push_pending(index);
            }
        }
        if INSTRUMENT { telemetry[index]=counts; }
    }
    flush_pending(lane);
}

@compute @workgroup_size(8,8)
fn queue_unresolved(@builtin(global_invocation_id) id:vec3u,
                    @builtin(local_invocation_index) lane:u32) {
    begin_pending(lane);
    let ij=id.xy+view.image.zw;
    if valid_tile_pixel(id.xy,ij) {
        let index=ij.y*view.image.x+ij.x;
        if pixels[index].w==-2. {push_pending(index);}
    }
    flush_pending(lane);
}

@compute @workgroup_size(64)
fn resolve_fallback(@builtin(global_invocation_id) id:vec3u) {
    if id.x >= atomicLoad(&fallback.length) { return; }
    let ix=fallback.indices[id.x];
    resolve_pixel(vec2u(ix%view.image.x,ix/view.image.x));
}
