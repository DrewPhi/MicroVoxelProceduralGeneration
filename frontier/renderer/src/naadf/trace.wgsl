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
// Pipeline specialization keeps cache state and branches out of the control.
override traversal_cache_nodes:bool=false;
override traversal_cache_nodes_compact:bool=false;
override traversal_height_bounds:bool=false;
override traversal_height_tile:u32=8u;
override traversal_height_mips:bool=false;
override traversal_height_gate:bool=false;
fn distance_byte3(word:u32)->vec3<u32> {
    return (vec3(word)>>vec3(0u,8u,16u))&vec3(255u);
}
fn cached_root_word(cp:vec3<u32>,fallback:u32)->u32 {
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
    if (naadf.ring.w&2u)==0u { return voxels[address]; }
    let packed=voxels[address>>1u];
    let word=(packed>>((address&1u)*16u))&0xffffu;
    return select(word,0x40000000u|(word&0x7fffu),(word&0x8000u)!=0u);
}
fn sparse_root_load(address:u32,shape:vec3<u32>)->u32 {
    let row=address/shape.x;
    let texel=vec3(address%shape.x,row%shape.y,row/shape.y);
    return textureLoad(chunks,vec3<i32>(texel),0).x;
}
struct HeightResume {cell:vec3<i32>,t:f32,blocked_below:u32};
fn height_resume(o:vec3<f32>,d:vec3<f32>,cell:vec3<i32>,t:f32,exit:f32,exact_crossing:f32)->HeightResume {
    let extent=vec3<f32>(naadf.extent.xyz);
    var level=0u;
    if traversal_height_mips {level=textureNumLevels(heights)-1u;}
    loop {
        let side=traversal_height_tile*16u*(1u<<level);
        let tile=vec2<u32>(cell.xz)/side;
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
                    return HeightResume(resumed,safe,0u);
                }
            }
        } else if level==0u {
            return HeightResume(cell,t,highest);
        }
        if level==0u {break;}
        level-=1u;
    }
    return HeightResume(cell,t,0u);
}
fn trace_impl(o:vec3<f32>,d:vec3<f32>,minimum:f32,occlusion:bool,measure:bool,allow_height:bool)->VoxelHit {
    var visits=vec3<u32>(0);var flags=0u;
    if naadf.extent.x==0u{return VoxelHit(false,0.,vec3(0.),0u,visits,flags);}
    let interval=voxel_interval(o,d);var t=max(minimum,max(0.,interval.x));let exit=interval.y;
    if t>=exit{return VoxelHit(false,t,vec3(0.),0u,visits,flags);}
    var cell=clamp(forward_cell(o+d*t,d),vec3(0),vec3<i32>(naadf.extent.xyz)-vec3(1));
    var faces=0u;var was_solid=false;
    let extent=vec3<i32>(naadf.extent.xyz);
    let sparse=(naadf.ring.w&1u)!=0u;
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
    // A monotone ray crosses at most sum(extent) native grid planes. The bound
    // comes from the volume, never a heuristic step cap that can erase hits.
    let budget=naadf.extent.x+naadf.extent.y+naadf.extent.z+8u;
    for(var iteration=0u;iteration<budget;iteration++) {
        if any(cell<vec3(0))||any(cell>=extent){break;}
        if use_height && !traversal_height_gate {
            let advance=height_resume(o,d,cell,t,exit,t);
            if advance.t>t {
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
        if traversal_cache_nodes && all(cp==last_root) {
            node=root_word;
            scale=root_scale;
        } else if traversal_cache_nodes_compact && root_key==last_mixed_root_key {
            node=root_word;
        } else {
            if sparse {
                let page=cp/4u;
                let page_address=page.y+page_side*(page.x+page_side*page.z);
                var entry=page_word;
                var page_reuse=false;
                if traversal_cache_nodes {page_reuse=all(page==last_page);}
                if traversal_cache_nodes_compact {page_reuse=page_address==last_page_address;}
                if !page_reuse {
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
                    node=sparse_root_load(address,atlas_shape);
                }
            } else {
                let address=cp+naadf.ring.xyz;
                let size=vec3(naadf.extent.w);
                let slot=select(address,address-size,address>=size);
                node=cached_root_word(cp,textureLoad(chunks,vec3<i32>(slot.yxz),0).x);
            }
            if traversal_cache_nodes {
                last_root=cp;
                root_word=node;
                root_scale=scale;
            }
            if traversal_cache_nodes_compact && (node&0x80000000u)!=0u {
                last_mixed_root_key=root_key;
                root_word=node;
            }
        }
        let root_empty=(node&0xc0000000u)==0u;
        if (node&0x80000000u)!=0u {
            if measure {visits.y+=1u;}
            let bp=(c%16u)/4u;
            let b=c/4u;
            let block_address=(node&0x3fffffffu)+bp.x+4u*(bp.y+4u*bp.z);
            var block_reuse=false;
            if traversal_cache_nodes {block_reuse=all(b==last_block);}
            if traversal_cache_nodes_compact {block_reuse=block_address==last_block_address;}
            if block_reuse {
                node=block_word;
            } else {
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
                if measure {visits.z+=1u;}
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
        for(var a=0u;a<3u;a++){if d[a]!=0.{next[a]=(f32(select(lo[a],hi[a],d[a]>0.))-o[a])/d[a];}}
        let crossing=min(next.x,min(next.y,next.z));
        if crossing<=t {return VoxelHit(false,t,vec3(0.),0u,visits,flags|TRACE_STALLED);}
        if crossing>exit {return VoxelHit(false,t,vec3(0.),0u,visits,flags);}
        if use_height && traversal_height_gate && root_empty && crossing+2.<exit {
            let tile=c.xz/(traversal_height_tile*16u);
            // Every ancestor's max is at least its base tile's max. A previous
            // rejection proves there is no new useful bound until Y rises
            // above that max, or the ray crosses out of the XZ tile.
            if any(tile!=rejected_tile) || c.y>=rejected_below {
                let advance=height_resume(o,d,cell,t,exit,crossing);
                if advance.t>t && any(vec3<u32>(advance.cell)/16u!=cp) {
                    cell=advance.cell;t=advance.t;faces=0u;was_solid=false;
                    flags|=TRACE_SKIPPED;
                    // Reload only for the new cell. No-skip iterations use the
                    // already loaded node and its exact crossing below.
                    continue;
                }
                if advance.blocked_below!=0u {
                    rejected_tile=tile;
                    rejected_below=advance.blocked_below;
                }
            }
        }
        cell=clamp(forward_cell(o+d*crossing,d),lo,hi-vec3(1));faces=0u;
        for(var a=0u;a<3u;a++){if next[a]==crossing{cell[a]=select(lo[a]-1,hi[a],d[a]>0.);faces|=1u<<(a*2u+select(1u,0u,d[a]>0.));}}
        t=crossing;
    }
    if all(cell>=vec3(0)) && all(cell<extent) {flags|=TRACE_STALLED;}
    return VoxelHit(false,t,vec3(0.),0u,visits,flags);
}
fn trace_prefixed(o:vec3<f32>,d:vec3<f32>,minimum:f32,prefix:f32,occlusion:bool,measure:bool)->VoxelHit {
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
            let exact=trace_impl(o,d,minimum,occlusion,measure,false);
            h=VoxelHit(exact.found,exact.t,exact.normal,exact.pigment,h.visits+exact.visits,exact.flags);
        }
    }
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

fn naadf_mesh_sun_visibility(receiver_relative:vec3<f32>,normal:vec3<f32>,sun:vec3<f32>)->f32 {
    if naadf.shadow.z>0.5{return 1.;}
    return naadf_sun_visibility(receiver_relative,normal,sun);
}
