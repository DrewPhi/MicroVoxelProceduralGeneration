// Shared exact hierarchy for primary rays and occlusion queries. Generated
// with group0 for the ray pass and the shared near-shadow group for mesh receivers.
struct NaadfCamera {matrix:mat4x4<f32>,inverse:mat4x4<f32>,eye:vec4<f32>,extent:vec4<u32>,viewport:vec4<f32>,sun:vec4<f32>,sky:vec4<f32>,far_matrix:mat4x4<f32>,shadow:vec4<f32>,forward:vec4<f32>,right:vec4<f32>,up:vec4<f32>,projection:vec4<f32>,ring:vec4<u32>};
struct Pigment {rgb:vec3<f32>,material:u32};
@group(NAADF_GROUP) @binding(NAADF_BASE) var<uniform> naadf:NaadfCamera;
@group(NAADF_GROUP) @binding(NAADF_BASE+1u) var chunks:texture_3d<u32>;
@group(NAADF_GROUP) @binding(NAADF_BASE+2u) var<storage,read> blocks:array<u32>;
@group(NAADF_GROUP) @binding(NAADF_BASE+3u) var<storage,read> voxels:array<u32>;
@group(NAADF_GROUP) @binding(NAADF_BASE+4u) var<storage,read> palette:array<Pigment>;
@group(NAADF_GROUP) @binding(NAADF_BASE+5u) var root_distance:texture_3d<u32>;
fn root_tile_rank(coordinate:vec3<u32>,side:u32)->u32 {
    let center=vec3(side/2u);
    let rank=select(2u*(coordinate-center),2u*(center-coordinate)-1u,coordinate<center);
    return rank.x+side*(rank.y+side*rank.z);
}
fn cached_root_word(cp:vec3<u32>,fallback:u32)->u32 {
    let ready=naadf.ring.w>>2u;
    if ready==0u || (fallback&0xc0000000u)!=0u {return fallback;}
    let n=naadf.extent.w;
    let side=(min(n,128u)+31u)/32u;
    let offset=(n-min(n,128u))/2u;
    if any(cp<vec3(offset)) || any(cp>=vec3(offset+side*32u)) {return fallback;}
    let p=cp-vec3(offset);
    if root_tile_rank(p/32u,side)>=ready {return fallback;}
    return textureLoad(root_distance,vec3<i32>(p),0).x;
}
struct VoxelHit {found:bool,t:f32,normal:vec3<f32>,pigment:u32,visits:vec3<u32>};
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
fn trace_counted(o:vec3<f32>,d:vec3<f32>,minimum:f32,occlusion:bool,measure:bool)->VoxelHit {
    var visits=vec3<u32>(0);
    if naadf.extent.x==0u{return VoxelHit(false,0.,vec3(0.),0u,visits);}
    let interval=voxel_interval(o,d);var t=max(minimum,max(0.,interval.x));let exit=interval.y;
    if t>=exit{return VoxelHit(false,t,vec3(0.),0u,visits);}
    var cell=clamp(forward_cell(o+d*t,d),vec3(0),vec3<i32>(naadf.extent.xyz)-vec3(1));
    var faces=0u;var was_solid=false;
    let extent=vec3<i32>(naadf.extent.xyz);
    let sparse=(naadf.ring.w&1u)!=0u;
    let page_side=(naadf.extent.w+3u)/4u;
    var atlas_shape=vec3(1u);
    if sparse { atlas_shape=textureDimensions(chunks,0); }
    // A monotone ray crosses at most sum(extent) native grid planes. The bound
    // comes from the volume, never a heuristic step cap that can erase hits.
    let budget=naadf.extent.x+naadf.extent.y+naadf.extent.z+8u;
    for(var iteration=0u;iteration<budget;iteration++) {
        if any(cell<vec3(0))||any(cell>=extent){break;}
        let c=vec3<u32>(cell);let cp=c/16u;
        var scale=16u;var bits=5u;var parent_lo=vec3(0);var parent_hi=extent;
        var node=0u;
        if measure {visits.x+=1u;}
        if sparse {
            let page=cp/4u;
            let entry=sparse_root_load(page.y+page_side*(page.x+page_side*page.z),atlas_shape);
            if (entry&0x80000000u)==0u {
                // The entire page has the same occupancy and pigment.
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
        if (node&0x80000000u)!=0u {
            if measure {visits.y+=1u;}
            let bp=(c%16u)/4u;node=blocks[(node&0x3fffffffu)+bp.x+4u*(bp.y+4u*bp.z)];
            scale=4u;bits=2u;parent_lo=vec3<i32>(cp*16u);parent_hi=parent_lo+vec3(16);
            if (node&0x80000000u)!=0u {
                if measure {visits.z+=1u;}
                let vp=c%4u;let vi=vp.x+4u*(vp.y+4u*vp.z);
                node=naadf_leaf_word((node&0x3fffffffu)+vi);
                scale=1u;parent_lo=vec3<i32>((c/4u)*4u);parent_hi=parent_lo+vec3(4);
            }
        }
        let solid=(node&0x40000000u)!=0u;
        if solid && occlusion {return VoxelHit(true,t,vec3(0.),0u,visits);}
        if solid && !was_solid && faces!=0u {
            var face=0u;
            for(var k=0u;k<6u;k++){if (faces&(1u<<k))!=0u{face=k;break;}}
            var normal=vec3(0.); normal[face/2u]=select(-1.,1.,(face&1u)!=0u);
            return VoxelHit(true,t,normal,node&0x3fffffffu,visits);
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
        if crossing<=t || crossing>exit {return VoxelHit(false,t,vec3(0.),0u,visits);}
        cell=clamp(forward_cell(o+d*crossing,d),lo,hi-vec3(1));faces=0u;
        for(var a=0u;a<3u;a++){if next[a]==crossing{cell[a]=select(lo[a]-1,hi[a],d[a]>0.);faces|=1u<<(a*2u+select(1u,0u,d[a]>0.));}}
        t=crossing;
    }
    return VoxelHit(false,t,vec3(0.),0u,visits);
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
