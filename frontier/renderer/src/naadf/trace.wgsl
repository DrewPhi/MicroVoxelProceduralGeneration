// Shared exact hierarchy for primary rays and occlusion queries. Generated
// with group0 for the ray pass and the shared near-shadow group for mesh receivers.
struct NaadfCamera {matrix:mat4x4<f32>,inverse:mat4x4<f32>,eye:vec4<f32>,extent:vec4<u32>,viewport:vec4<f32>,sun:vec4<f32>,sky:vec4<f32>,far_matrix:mat4x4<f32>,shadow:vec4<f32>,forward:vec4<f32>,right:vec4<f32>,up:vec4<f32>,projection:vec4<f32>,ring:vec4<u32>,cache:vec4<u32>};
struct Pigment {rgb:vec3<f32>,material:u32};
@group(NAADF_GROUP) @binding(NAADF_BASE) var<uniform> naadf:NaadfCamera;
@group(NAADF_GROUP) @binding(NAADF_BASE+1u) var chunks:texture_3d<u32>;
@group(NAADF_GROUP) @binding(NAADF_BASE+2u) var<storage,read> blocks:array<u32>;
@group(NAADF_GROUP) @binding(NAADF_BASE+3u) var<storage,read> voxels:array<u32>;
@group(NAADF_GROUP) @binding(NAADF_BASE+4u) var<storage,read> palette:array<Pigment>;
@group(NAADF_GROUP) @binding(NAADF_BASE+5u) var root_distance:texture_3d<u32>;
@group(NAADF_GROUP) @binding(NAADF_BASE+6u) var heights:texture_2d<u32>;
@group(NAADF_GROUP) @binding(NAADF_BASE+7u) var<storage,read_write> trace_metrics:array<atomic<u32>>;
override traversal_metrics:bool=false;
// Approximate lighting-only experiment. Primary hits always remain exact.
override traversal_shadow_coarse_after_m:f32=0.;
fn trace_metric(lane:u32,index:u32,value:u32) {
    if traversal_metrics {atomicAdd(&trace_metrics[lane*32u+index],value);}
}
fn trace_distance(lane:u32,first:u32,amount:f32) {
    if traversal_metrics {
        let value=u32(clamp(floor(amount),0.,1.e9));
        let old=atomicAdd(&trace_metrics[lane*32u+first],value);
        if old>0xffffffffu-value {atomicAdd(&trace_metrics[lane*32u+first+1u],1u);}
    }
}

// Pipeline specialization keeps cache state and branches out of the control.
override traversal_cache_nodes:bool=false;
// 0 retains the general runtime format. Known immutable volume formats can
// specialize primary rays without specializing shared raster receiver shaders.
override traversal_leaf_format:u32=0u;
override traversal_root_format:u32=0u;
override traversal_cpu_roots:bool=false;
fn naadf_sparse()->bool {
    if traversal_root_format!=0u{return traversal_root_format==2u;}
    return (naadf.ring.w&1u)!=0u;
}
override traversal_cache_nodes_compact:bool=false;
override traversal_height_bounds:bool=false;
override traversal_height_tile:u32=8u;
override traversal_height_mips:bool=false;
override traversal_height_gate:bool=false;
override traversal_height_probe:u32=0xffffffffu;
fn distance_byte3(word:u32)->vec3<u32> {
    return (vec3(word)>>vec3(0u,8u,16u))&vec3(255u);
}
override traversal_direct_roots:bool=false;
override traversal_local_exit_repair:bool=false;
// The otherwise-invalid CHILD|FULL root tag identifies a GPU-grown air proof.
// It travels with physical root slots; publishing fresh CPU strips removes it.
fn root_repaired(word:u32)->bool {
    return traversal_direct_roots && (word&0xc0000000u)==0xc0000000u;
}
fn root_decode(word:u32)->u32 {
    return select(word,word&0x3fffffffu,root_repaired(word));
}

fn cached_root_word(cp:vec3<u32>,fallback:u32,lane:u32,record:bool)->u32 {
    if traversal_direct_roots || traversal_cpu_roots {return fallback;}
    let masks=naadf.cache.xy;
    if (masks.x|masks.y)==0u || (fallback&0xc0000000u)!=0u {return fallback;}
    let side=naadf.cache.w>>24u;
    let n=naadf.extent.w;
    let center=(n-min(n,128u))/2u;
    let bytes=vec3<i32>(distance_byte3(naadf.cache.z));
    let base=vec3(i32(center))+select(bytes,bytes-vec3(256),bytes>=vec3(128));
    let p=vec3<i32>(cp)-base;
    if any(p<vec3(0)) || any(p>=vec3(i32(side*32u))) {return fallback;}
    let cell=vec3<u32>(p);
    let slot=(cell/32u+distance_byte3(naadf.cache.w))%vec3(side);
    let index=slot.x+side*(slot.y+side*slot.z);
    if (masks[index/32u] & (1u<<(index%32u)))==0u {return fallback;}
    if record {trace_metric(lane,29u,1u);}
    let proof=textureLoad(root_distance,vec3<i32>(slot*32u+cell%32u),0).x;
    if (naadf.ring.w&4u)!=0u && fallback!=0u {
        // Either proof is safe independently. Mixing face extents from
        // different cuboids is unsafe; only use a proof containing fallback.
        for(var shift=0u;shift<30u;shift+=5u) {
            if ((proof>>shift)&31u)<((fallback>>shift)&31u) {return fallback;}
        }
    }
    return proof;
}
const TRACE_SKIPPED:u32=1u;
const TRACE_STALLED:u32=2u;
const TRACE_ROOT_CORNER:u32=4u;
const TRACE_REPAIRED_PATH:u32=8u;
const TRACE_LOCAL_UNRESOLVED:u32=16u;
struct VoxelHit {found:bool,t:f32,normal:vec3<f32>,pigment:u32,visits:vec3<u32>,flags:u32};
fn forward_cell(p:vec3<f32>,d:vec3<f32>)->vec3<i32>{let f=floor(p);return vec3<i32>(f)-vec3<i32>(select(vec3<u32>(0),vec3<u32>(1),(d<vec3(0.))&(p==f)));}
fn voxel_interval(o:vec3<f32>,d:vec3<f32>)->vec2<f32>{
    var lo=-1e30;var hi=1e30;
    for(var a=0u;a<3u;a++){
        let upper=f32(naadf.extent[a]);
        if d[a]==0. {if o[a]<0.||o[a]>=upper{return vec2(1.,-1.);}}
        else {let a0=-o[a]/d[a];let a1=(upper-o[a])/d[a];lo=max(lo,min(a0,a1));hi=min(hi,max(a0,a1));}
    }
    return vec2(lo,hi);
}
fn naadf_leaf_word(address:u32)->u32 {
    if traversal_leaf_format==1u || (traversal_leaf_format==0u && (naadf.ring.w&2u)==0u) { return voxels[address]; }
    let packed=voxels[address>>1u];
    let word=(packed>>((address&1u)*16u))&0xffffu;
    return select(word,0x40000000u|(word&0x7fffu),(word&0x8000u)!=0u);
}
fn sparse_root_load(address:u32,shape:vec3<u32>)->u32 {
    let row=address/shape.x;
    let texel=vec3(address%shape.x,row%shape.y,row/shape.y);
    return textureLoad(chunks,vec3<i32>(texel),0).x;
}
fn trace_diagnostic_crossing(o:vec3<f32>,d:vec3<f32>,cell:vec3<i32>,t:f32)->f32 {
    let c=vec3<u32>(cell);let cp=c/16u;let extent=vec3<i32>(naadf.extent.xyz);
    var node=0u;var scale=16u;var bits=5u;var parent_lo=vec3(0);var parent_hi=extent;
    if naadf_sparse() {
        let side=(naadf.extent.w+3u)/4u;let page=cp/4u;let shape=textureDimensions(chunks,0);
        let entry=sparse_root_load(page.y+side*(page.x+side*page.z),shape);
        if (entry&0x80000000u)==0u {node=entry;scale=64u;}
        else {
            let local=cp%4u;
            node=sparse_root_load(side*side*side+(entry&0x3fffffffu)+local.y+4u*(local.x+4u*local.z),shape);
        }
    } else {
        let address=cp+naadf.ring.xyz;let size=vec3(naadf.extent.w);
        let slot=select(address,address-size,address>=size);
        node=cached_root_word(cp,textureLoad(chunks,vec3<i32>(slot.yxz),0).x,0u,false);
    }
    node=root_decode(node);
    if (node&0x80000000u)!=0u {
        let bp=(c%16u)/4u;node=blocks[(node&0x3fffffffu)+bp.x+4u*(bp.y+4u*bp.z)];
        scale=4u;bits=2u;parent_lo=vec3<i32>(cp*16u);parent_hi=parent_lo+vec3(16);
        if (node&0x80000000u)!=0u {
            let vp=c%4u;node=naadf_leaf_word((node&0x3fffffffu)+vp.x+4u*(vp.y+4u*vp.z));
            scale=1u;parent_lo=vec3<i32>((c/4u)*4u);parent_hi=parent_lo+vec3(4);
        }
    }
    if (node&0x40000000u)!=0u {return t;}
    let base=vec3<i32>((c/scale)*scale);
    var lo=base;var hi=base+vec3<i32>(i32(scale));let mask=(1u<<bits)-1u;
    for(var a=0u;a<3u;a++){
        lo[a]-=i32((node>>(a*2u*bits))&mask)*i32(scale);
        hi[a]+=i32((node>>((a*2u+1u)*bits))&mask)*i32(scale);
    }
    lo=max(lo,parent_lo);hi=min(hi,parent_hi);
    var crossing=1e30;
    for(var a=0u;a<3u;a++){
        if d[a]!=0. {crossing=min(crossing,(f32(select(lo[a],hi[a],d[a]>0.))-o[a])/d[a]);}
    }
    return crossing;
}
struct HeightResume {cell:vec3<i32>,t:f32,blocked_below:u32};
fn height_resume(o:vec3<f32>,d:vec3<f32>,cell:vec3<i32>,t:f32,exit:f32,exact_crossing:f32,shadow:bool)->HeightResume {
    let lane=u32(shadow);
    trace_metric(lane,7u,1u);
    let extent=vec3<f32>(naadf.extent.xyz);
    var level=0u;
    if traversal_height_mips {level=textureNumLevels(heights)-1u;}
    let single_probe=traversal_height_probe!=0xffffffffu;
    if single_probe {level=min(level,traversal_height_probe);}
    loop {
        let side=traversal_height_tile*16u*(1u<<level);
        let tile=vec2<u32>(cell.xz)/side;
        trace_metric(lane,8u,1u);
        let highest=textureLoad(heights,vec2<i32>(tile),i32(level)).x;
        if u32(cell.y)>=highest {
            let base=tile*side;
            let lo=vec3<f32>(f32(base.x),f32(highest),f32(base.y));
            let hi=min(vec3<f32>(f32(base.x+side),extent.y,f32(base.y+side)),extent);
            var limit=exit;
            for(var a=0u;a<3u;a++){
                if d[a]!=0. {limit=min(limit,(select(lo[a],hi[a],d[a]>0.)-o[a])/d[a]);}
            }
            // Resume inside proven air; the large tile supplies no hit face.
            let safe=limit-2.;
            if safe-t>=32. && safe>exact_crossing {
                let resumed=forward_cell(o+d*safe,d);
                if all(vec3<f32>(resumed)>=lo) && all(vec3<f32>(resumed)<hi) {
                    trace_metric(lane,9u,1u);
                    return HeightResume(resumed,safe,0u);
                }
            }
        } else if level==0u || single_probe {
            trace_metric(lane,11u,1u);trace_metric(lane,12u,1u);
            return HeightResume(cell,t,highest);
        }
        if level==0u || single_probe {break;}
        level-=1u;
    }
    trace_metric(lane,11u,1u);trace_metric(lane,13u,1u);
    return HeightResume(cell,t,0u);
}
// Identical per-plane arithmetic for native DDA and the local handoff. This
// function defines the prototype's f32 event-time contract, not an exact-real
// intersection predicate under arbitrary WGSL reassociation/flush behavior.
fn native_plane_time(plane:i32,origin:f32,direction:f32)->f32 {
    return (f32(plane)-origin)/direction;
}
struct LocalExit {valid:bool,cell:vec3<i32>,t:f32,faces:u32};
fn local_exit(o:vec3<f32>,d:vec3<f32>,lo:vec3<i32>,hi:vec3<i32>,current_cell:vec3<i32>,current_t:f32,coarse_next:vec3<f32>)->LocalExit {
    let tau=min(coarse_next.x,min(coarse_next.y,coarse_next.z));
    let invalid=LocalExit(false,current_cell,current_t,0u);
    if !(tau>current_t && abs(tau)<1.e29) {return invalid;}
    if any(current_cell<lo)||any(current_cell>=hi) {return invalid;}
    if any(abs(o)>vec3<f32>(naadf.extent.xyz)*16.) {return invalid;}
    if any((d!=vec3(0.))&(abs(d)<vec3(1.17549435e-38))) {return invalid;}
    var axis=0u;
    if coarse_next.y<coarse_next.x {axis=1u;}
    if coarse_next.z<coarse_next[axis] {axis=2u;}
    if d[axis]==0. {return invalid;}
    let plane=select(lo[axis],hi[axis],d[axis]>0.);
    let projection=d*tau;
    let coordinate_scale=abs(o)+abs(vec3<f32>(lo))+abs(vec3<f32>(hi))+vec3(1.);
    for(var a=0u;a<3u;a++) {
        // Bound the separately divided terms too; cancellation must not hide
        // an overflowing intermediate in an algebraically equivalent form.
        if d[a]!=0. && !(coordinate_scale[a]<1.e29*abs(d[a])) {return invalid;}
    }
    // Engineering filter, deliberately wider than the current driver's usual
    // error: reject large cancellation/amplification and coordinates whose bin
    // could be more than one native cell away. Runtime event-interval checks
    // below decide acceptance; this filter is not a portable WGSL error proof.
    let amplification=abs(d/d[axis])*(abs(f32(plane))+abs(o[axis]));
    let envelope=32.*1.1920928955078125e-7*(abs(o)+abs(projection)
        +abs(vec3<f32>(lo))+abs(vec3<f32>(hi))+amplification+vec3(1.));
    if !all(envelope<vec3(0.25)) {return invalid;}
    let position=o+projection;
    if !all(abs(position)<vec3(8388608.)) {return invalid;}
    var cell=vec3<i32>(floor(position));
    var faces=0u;
    for(var a=0u;a<3u;a++) {
        if d[a]==0. {
            // A constant coordinate does not generate an entry face.
            if cell[a]!=current_cell[a] {return invalid;}
            continue;
        }
        let step=select(-1,1,d[a]>0.);
        if coarse_next[a]==tau {
            cell[a]=select(lo[a]-1,hi[a],d[a]>0.);
        }
        var entry=native_plane_time(cell[a]+select(1,0,d[a]>0.),o[a],d[a]);
        var leave=native_plane_time(cell[a]+select(0,1,d[a]>0.),o[a],d[a]);
        if tau<entry {
            cell[a]-=step;
            leave=entry;
            entry=native_plane_time(cell[a]+select(1,0,d[a]>0.),o[a],d[a]);
        } else if tau>=leave {
            cell[a]+=step;
            entry=leave;
            leave=native_plane_time(cell[a]+select(0,1,d[a]>0.),o[a],d[a]);
        }
        // Strictly ordered native boundaries, and the selected event belongs
        // to this post-crossing cell. One correction is the entire local work.
        if !(entry<=tau && tau<leave && entry<leave
            && abs(entry)<1.e29 && abs(leave)<1.e29) {return invalid;}
        if cell[a]<lo[a] || cell[a]>=hi[a] {
            if coarse_next[a]!=tau {return invalid;}
        }
        if entry==tau {faces|=1u<<(a*2u+select(1u,0u,d[a]>0.));}
    }
    if faces==0u {return invalid;}
    return LocalExit(true,cell,tau,faces);
}

// Rare direct-root replay ignores all proof fields. Only crossed axes change
// cell identity; o+d*t never reclassifies a transverse voxel at a corner.
fn trace_native(o:vec3<f32>,d:vec3<f32>,minimum:f32,occlusion:bool,measure:bool)->VoxelHit {
    var visits=vec3<u32>(0);
    if naadf.extent.x==0u {return VoxelHit(false,0.,vec3(0.),0u,visits,0u);}
    let interval=voxel_interval(o,d);
    var t=max(minimum,max(0.,interval.x));
    let exit=interval.y;
    if t>=exit {return VoxelHit(false,t,vec3(0.),0u,visits,0u);}
    let extent=vec3<i32>(naadf.extent.xyz);
    var cell=clamp(forward_cell(o+d*t,d),vec3(0),extent-vec3(1));
    var faces=0u;var was_solid=false;
    let sparse=naadf_sparse();
    let page_side=(naadf.extent.w+3u)/4u;
    var atlas_shape=vec3(1u);
    if sparse {atlas_shape=textureDimensions(chunks,0);}
    let budget=naadf.extent.x+naadf.extent.y+naadf.extent.z+8u;
    for(var iteration=0u;iteration<budget;iteration++) {
        if any(cell<vec3(0)) || any(cell>=extent) {break;}
        trace_metric(u32(occlusion),31u,1u);
        let c=vec3<u32>(cell);let cp=c/16u;
        var node=0u;
        if measure {visits.x+=1u;}
        trace_metric(u32(occlusion),1u,1u);trace_metric(u32(occlusion),4u,1u);
        if sparse {
            let page=cp/4u;
            let entry=sparse_root_load(page.y+page_side*(page.x+page_side*page.z),atlas_shape);
            node=entry;
            if (entry&0x80000000u)!=0u {
                let local=cp%4u;
                trace_metric(u32(occlusion),4u,1u);
                node=sparse_root_load(page_side*page_side*page_side+(entry&0x3fffffffu)
                    +local.y+4u*(local.x+4u*local.z),atlas_shape);
            }
        } else {
            let slot=(cp+naadf.ring.xyz)%vec3(naadf.extent.w);
            node=textureLoad(chunks,vec3<i32>(slot.yxz),0).x;
        }
        node=root_decode(node);
        if (node&0x80000000u)!=0u {
            if measure {visits.y+=1u;}
            trace_metric(u32(occlusion),2u,1u);trace_metric(u32(occlusion),5u,1u);
            let bp=(c%16u)/4u;
            node=blocks[(node&0x3fffffffu)+bp.x+4u*(bp.y+4u*bp.z)];
            if (node&0x80000000u)!=0u {
                if measure {visits.z+=1u;}
                trace_metric(u32(occlusion),3u,1u);trace_metric(u32(occlusion),6u,1u);
                let vp=c%4u;
                node=naadf_leaf_word((node&0x3fffffffu)+vp.x+4u*(vp.y+4u*vp.z));
            }
        }
        let solid=(node&0x40000000u)!=0u;
        if solid && occlusion {return VoxelHit(true,t,vec3(0.),0u,visits,0u);}
        if solid && !was_solid && faces!=0u {
            var face=0u;
            for(var k=0u;k<6u;k++) {if (faces&(1u<<k))!=0u {face=k;break;}}
            var normal=vec3(0.);normal[face/2u]=select(-1.,1.,(face&1u)!=0u);
            return VoxelHit(true,t,normal,node&0x3fffffffu,visits,0u);
        }
        was_solid=solid;
        var next=vec3(1e30);
        for(var a=0u;a<3u;a++) {
            if d[a]!=0. {next[a]=native_plane_time(cell[a]+select(0,1,d[a]>0.),o[a],d[a]);}
        }
        let crossing=min(next.x,min(next.y,next.z));
        if crossing>exit {break;}
        // At least one integer coordinate advances every iteration, including
        // sub-ulp coincident times. The volume-derived iteration bound holds.
        faces=0u;
        for(var a=0u;a<3u;a++) {
            if next[a]==crossing {
                cell[a]+=select(-1,1,d[a]>0.);
                faces|=1u<<(a*2u+select(1u,0u,d[a]>0.));
            }
        }
        t=max(t,crossing);
    }
    return VoxelHit(false,t,vec3(0.),0u,visits,0u);
}

fn trace_near_corner(o:vec3<f32>,d:vec3<f32>,t:f32)->vec3<bool> {
    let p=o+d*t;
    let error=max(vec3(0.001),4.e-7*(abs(o)+abs(d*t)));
    return (abs(p-round(p))<=error)&(d!=vec3(0.));
}

fn trace_impl(o:vec3<f32>,d:vec3<f32>,minimum:f32,occlusion:bool,measure:bool,allow_height:bool)->VoxelHit {
    var visits=vec3<u32>(0);var flags=0u;
    let lane=u32(occlusion);
    if naadf.extent.x==0u{return VoxelHit(false,0.,vec3(0.),0u,visits,flags);}
    let interval=voxel_interval(o,d);var t=max(minimum,max(0.,interval.x));let exit=interval.y;
    if t>=exit{return VoxelHit(false,t,vec3(0.),0u,visits,flags);}
    var cell=clamp(forward_cell(o+d*t,d),vec3(0),vec3<i32>(naadf.extent.xyz)-vec3(1));
    var faces=0u;var was_solid=false;
    let extent=vec3<i32>(naadf.extent.xyz);
    let sparse=naadf_sparse();
    // Very distant camera origins can lose native-cell precision in o+d*t.
    let use_height=traversal_height_bounds && allow_height
        && all(abs(o)<=vec3<f32>(extent)*16.);
    let page_side=(naadf.extent.w+3u)/4u;
    var atlas_shape=vec3(1u);
    if sparse { atlas_shape=textureDimensions(chunks,0); }
    // Logical coordinates key each read, independently of the physical ring
    // or repeated child pointers. Entries live only for this immutable ray.
    var last_root=vec3(0xffffffffu);var root_word=0u;var root_scale=16u;
    var last_page=vec3(0xffffffffu);var page_word=0u;
    var last_block=vec3(0xffffffffu);var block_word=0u;
    var last_mixed_root_key=0xffffffffu;
    var last_page_address=0xffffffffu;
    var last_block_address=0xffffffffu;
    var rejected_tile=vec2(0xffffffffu);var rejected_below=0u;
    var rejection_side=traversal_height_tile*16u;
    if traversal_height_probe!=0xffffffffu {
        rejection_side*=1u<<min(traversal_height_probe,textureNumLevels(heights)-1u);
    }
    // A monotone ray crosses at most sum(extent) native grid planes. The bound
    // comes from the volume, never a heuristic step cap that can erase hits.
    let budget=naadf.extent.x+naadf.extent.y+naadf.extent.z+8u;
    for(var iteration=0u;iteration<budget;iteration++) {
        if any(cell<vec3(0))||any(cell>=extent){break;}
        if use_height && !traversal_height_gate {
            let advance=height_resume(o,d,cell,t,exit,t,occlusion);
            if advance.t>t {
                if traversal_metrics {
                    let original=trace_diagnostic_crossing(o,d,cell,t);
                    trace_metric(lane,28u,1u);
                    trace_distance(lane,15u,max(0.,advance.t-original));
                    trace_distance(lane,17u,max(0.,original-advance.t));
                    trace_metric(lane,10u,1u);
                }
                if traversal_local_exit_repair && (flags&TRACE_REPAIRED_PATH)!=0u {
                    flags|=TRACE_LOCAL_UNRESOLVED;
                }
                cell=advance.cell;t=advance.t;faces=0u;was_solid=false;
            }
        }
        let c=vec3<u32>(cell);let cp=c/16u;
        var scale=16u;var bits=5u;var parent_lo=vec3(0);var parent_hi=extent;
        var node=0u;
        // Snapshot construction limits the root edge to 512, hence each
        // coordinate fits nine bits. Compact mode tracks only mixed roots.
        var root_key=0u;
        if traversal_cache_nodes_compact {root_key=cp.x|(cp.y<<9u)|(cp.z<<18u);}
        if measure {visits.x+=1u;}
        trace_metric(lane,1u,1u);
        if traversal_cache_nodes && all(cp==last_root) {
            trace_metric(lane,25u,1u);
            node=root_word;
            scale=root_scale;
        } else if traversal_cache_nodes_compact && root_key==last_mixed_root_key {
            trace_metric(lane,25u,1u);
            node=root_word;
        } else {
            if sparse {
                let page=cp/4u;
                let page_address=page.y+page_side*(page.x+page_side*page.z);
                var entry=page_word;
                var page_reuse=false;
                if traversal_cache_nodes {page_reuse=all(page==last_page);}
                if traversal_cache_nodes_compact {page_reuse=page_address==last_page_address;}
                if page_reuse {trace_metric(lane,27u,1u);}
                if !page_reuse {
                    trace_metric(lane,4u,1u);
                    entry=sparse_root_load(page_address,atlas_shape);
                    if traversal_cache_nodes {
                        last_page=page;
                        page_word=entry;
                    }
                    if traversal_cache_nodes_compact {
                        last_page_address=page_address;
                        page_word=entry;
                    }
                }
                if (entry&0x80000000u)==0u {
                    // Partial air pages are clipped at the published bounds.
                    node=entry;
                    scale=64u;
                } else {
                    let local=cp%4u;
                    let address=page_side*page_side*page_side+(entry&0x3fffffffu)
                        +local.y+4u*(local.x+4u*local.z);
                    trace_metric(lane,4u,1u);
                    node=sparse_root_load(address,atlas_shape);
                }
            } else {
                let address=cp+naadf.ring.xyz;
                let size=vec3(naadf.extent.w);
                let slot=select(address,address-size,address>=size);
                trace_metric(lane,4u,1u);
                node=cached_root_word(cp,textureLoad(chunks,vec3<i32>(slot.yxz),0).x,lane,true);
            }
            if traversal_cache_nodes {
                last_root=cp;
                root_word=node;
                root_scale=scale;
            }
            if traversal_cache_nodes_compact && (node&0xc0000000u)==0x80000000u {
                last_mixed_root_key=root_key;
                root_word=node;
            }
        }
        let repaired_here=root_repaired(node);
        if repaired_here {flags|=TRACE_REPAIRED_PATH;}
        node=root_decode(node);
        let root_empty=(node&0xc0000000u)==0u;
        if (node&0x80000000u)!=0u {
            if measure {visits.y+=1u;}
            trace_metric(lane,2u,1u);
            let bp=(c%16u)/4u;
            let b=c/4u;
            let block_address=(node&0x3fffffffu)+bp.x+4u*(bp.y+4u*bp.z);
            var block_reuse=false;
            if traversal_cache_nodes {block_reuse=all(b==last_block);}
            if traversal_cache_nodes_compact {block_reuse=block_address==last_block_address;}
            if block_reuse {
                trace_metric(lane,26u,1u);
                node=block_word;
            } else {
                trace_metric(lane,5u,1u);
                node=blocks[block_address];
                if traversal_cache_nodes {
                    last_block=b;
                    block_word=node;
                }
                if traversal_cache_nodes_compact {
                    last_block_address=block_address;
                    block_word=node;
                }
            }
            scale=4u;bits=2u;parent_lo=vec3<i32>(cp*16u);parent_hi=parent_lo+vec3(16);
            if (node&0x80000000u)!=0u {
                if occlusion && traversal_shadow_coarse_after_m>0. && t>=traversal_shadow_coarse_after_m*16. {
                    return VoxelHit(true,t,vec3(0.),0u,visits,flags);
                }
                if measure {visits.z+=1u;}
                trace_metric(lane,3u,1u);trace_metric(lane,6u,1u);
                let vp=c%4u;let vi=vp.x+4u*(vp.y+4u*vp.z);
                node=naadf_leaf_word((node&0x3fffffffu)+vi);
                scale=1u;parent_lo=vec3<i32>((c/4u)*4u);parent_hi=parent_lo+vec3(4);
            }
        }
        let solid=(node&0x40000000u)!=0u;
        if solid && occlusion {return VoxelHit(true,t,vec3(0.),0u,visits,flags);}
        if solid && !was_solid && faces!=0u {
            var face=0u;
            for(var k=0u;k<6u;k++){if (faces&(1u<<k))!=0u{face=k;break;}}
            var normal=vec3(0.); normal[face/2u]=select(-1.,1.,(face&1u)!=0u);
            return VoxelHit(true,t,normal,node&0x3fffffffu,visits,flags);
        }
        was_solid=solid;
        let base=vec3<i32>((c/scale)*scale);
        var lo=base;var hi=base+vec3<i32>(i32(scale));
        if !solid {
            let mask=(1u<<bits)-1u;
            for(var a=0u;a<3u;a++){
                lo[a]-=i32((node>>(a*2u*bits))&mask)*i32(scale);
                hi[a]+=i32((node>>((a*2u+1u)*bits))&mask)*i32(scale);
            }
            lo=max(lo,parent_lo);hi=min(hi,parent_hi);
        }
        var next=vec3(1e30);
        for(var a=0u;a<3u;a++){if d[a]!=0.{next[a]=native_plane_time(select(lo[a],hi[a],d[a]>0.),o[a],d[a]);}}
        let crossing=min(next.x,min(next.y,next.z));
        if crossing<=t {trace_metric(lane,20u,1u);return VoxelHit(false,t,vec3(0.),0u,visits,flags|TRACE_STALLED);}
        if crossing>exit {return VoxelHit(false,t,vec3(0.),0u,visits,flags);}
        var ambiguous_local=false;
        let local_path=traversal_direct_roots && traversal_local_exit_repair
            && (flags&TRACE_REPAIRED_PATH)!=0u;
        if local_path && !solid {
            let at_plane=trace_near_corner(o,d,crossing);
            ambiguous_local=(at_plane.x&&at_plane.y)||(at_plane.y&&at_plane.z)||(at_plane.z&&at_plane.x);
        } else if repaired_here && root_empty && node!=0u {
            let at_plane=trace_near_corner(o,d,crossing);
            if (at_plane.x&&at_plane.y)||(at_plane.y&&at_plane.z)||(at_plane.z&&at_plane.x) {
                // Keep the flag even if subsequent traversal misses: an
                // altered empty cuboid can otherwise hide a corner hit.
                flags|=TRACE_ROOT_CORNER;
            }
        }
        if use_height && traversal_height_gate && root_empty && crossing+2.<exit {
            let tile=c.xz/rejection_side;
            // A full walk caches the rejected base max; a single probe caches
            // its selected tile's max. The same search cannot succeed until
            // Y rises above that max or the ray leaves that XZ tile.
            if any(tile!=rejected_tile) || c.y>=rejected_below {
                let advance=height_resume(o,d,cell,t,exit,crossing,occlusion);
                if advance.t>t && any(vec3<u32>(advance.cell)/16u!=cp) {
                    if local_path {flags|=TRACE_LOCAL_UNRESOLVED;}
                    cell=advance.cell;t=advance.t;faces=0u;was_solid=false;
                    flags|=TRACE_SKIPPED;
                    trace_metric(lane,10u,1u);
                    trace_distance(lane,15u,advance.t-crossing);
                    // Reload only for the new cell. No-skip iterations use the
                    // already loaded node and its exact crossing below.
                    continue;
                }
                if advance.blocked_below!=0u {
                    rejected_tile=tile;
                    rejected_below=advance.blocked_below;
                }
            } else {
                trace_metric(lane,14u,1u);
            }
        }
        if ambiguous_local {
            let repaired=local_exit(o,d,lo,hi,cell,t,next);
            if repaired.valid {
                cell=repaired.cell;t=repaired.t;faces=repaired.faces;
                trace_metric(lane,19u,1u);
                continue;
            }
            // Rejection is sticky, including misses; never silence the v2
            // global safety path when a local state could not be certified.
            flags|=TRACE_LOCAL_UNRESOLVED|TRACE_ROOT_CORNER;
        }
        cell=clamp(forward_cell(o+d*crossing,d),lo,hi-vec3(1));faces=0u;
        for(var a=0u;a<3u;a++){if next[a]==crossing{cell[a]=select(lo[a]-1,hi[a],d[a]>0.);faces|=1u<<(a*2u+select(1u,0u,d[a]>0.));}}
        t=crossing;
    }
    if all(cell>=vec3(0)) && all(cell<extent) {flags|=TRACE_STALLED;}
    return VoxelHit(false,t,vec3(0.),0u,visits,flags);
}
fn trace_prefixed(o:vec3<f32>,d:vec3<f32>,minimum:f32,prefix:f32,occlusion:bool,measure:bool)->VoxelHit {
    let lane=u32(occlusion);
    trace_metric(lane,0u,1u);
    var h=trace_impl(o,d,max(minimum,prefix),occlusion,measure,true);
    // A different preceding empty cuboid can change face priority at a
    // simultaneous plane crossing, or round into the previous cell and stall
    // before a hit is returned. Replay those cases with the original start.
    let height_may_have_skipped=traversal_height_bounds
        && (!traversal_height_gate || (h.flags&TRACE_SKIPPED)!=0u);
    let stalled=(h.flags&TRACE_STALLED)!=0u;
    if (height_may_have_skipped || prefix>minimum) && (h.found || stalled) {
        var replay=stalled;
        if h.found {
            let p=o+d*h.t;
            let error=max(vec3(0.001),4.e-7*(abs(o)+abs(d*h.t)));
            let at_plane=(abs(p-round(p))<=error)&(d!=vec3(0.));
            if occlusion {
                replay=(at_plane.x&&at_plane.y)||(at_plane.y&&at_plane.z)||(at_plane.z&&at_plane.x);
            } else {
                replay=any(at_plane&(h.normal==vec3(0.)));
            }
        }
        if replay {
            trace_metric(lane,select(22u,21u,stalled),1u);
            let exact=trace_impl(o,d,minimum,occlusion,measure,false);
            h=VoxelHit(exact.found,exact.t,exact.normal,exact.pigment,h.visits+exact.visits,exact.flags);
        }
    }
    if traversal_direct_roots {
        let repaired_path=(h.flags&TRACE_REPAIRED_PATH)!=0u;
        var native_replay=(h.flags&TRACE_ROOT_CORNER)!=0u
            || (repaired_path && (h.flags&TRACE_STALLED)!=0u);
        let locally_resolved=traversal_local_exit_repair
            && (h.flags&TRACE_LOCAL_UNRESOLVED)==0u;
        if h.found && repaired_path && !locally_resolved {
            let at_plane=trace_near_corner(o,d,h.t);
            if occlusion {
                native_replay=native_replay||(at_plane.x&&at_plane.y)||(at_plane.y&&at_plane.z)||(at_plane.z&&at_plane.x);
            } else {
                native_replay=native_replay||any(at_plane&(h.normal==vec3(0.)));
            }
        }
        if native_replay {
            trace_metric(lane,30u,1u);
            let exact=trace_native(o,d,minimum,occlusion,measure);
            h=VoxelHit(exact.found,exact.t,exact.normal,exact.pigment,h.visits+exact.visits,exact.flags);
        }
    }
    trace_metric(lane,select(23u,24u,h.found),1u);
    return h;
}
fn trace_counted(o:vec3<f32>,d:vec3<f32>,minimum:f32,occlusion:bool,measure:bool)->VoxelHit {
    return trace_prefixed(o,d,minimum,minimum,occlusion,measure);
}
fn trace_voxels(o:vec3<f32>,d:vec3<f32>,minimum:f32,occlusion:bool)->VoxelHit {
    return trace_counted(o,d,minimum,occlusion,false);
}
fn naadf_sun_visibility(receiver_relative:vec3<f32>,normal:vec3<f32>,sun:vec3<f32>)->f32 {
    if naadf.shadow.x<0.5 || dot(normal,sun)<=0. {return 1.;}
    let o=(naadf.eye.xyz+receiver_relative+normal*0.001)*16.;
    return select(1.,0.,trace_voxels(o,sun,0.,true).found);
}

override traversal_mesh_sun:bool=true;
fn naadf_mesh_sun_visibility(receiver_relative:vec3<f32>,normal:vec3<f32>,sun:vec3<f32>)->f32 {
    if !traversal_mesh_sun || naadf.shadow.z>0.5{return 1.;}
    return naadf_sun_visibility(receiver_relative,normal,sun);
}
