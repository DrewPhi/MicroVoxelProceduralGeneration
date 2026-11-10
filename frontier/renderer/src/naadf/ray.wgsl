// Three exact levels. Empty cuboids are acceleration only: no geometry LOD.
struct Camera {matrix:mat4x4<f32>,inverse:mat4x4<f32>,eye:vec4<f32>,extent:vec4<u32>,viewport:vec4<f32>,sun:vec4<f32>,sky:vec4<f32>};
struct Pigment {rgb:vec3<f32>,material:u32};
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var chunks:texture_3d<u32>;
@group(0) @binding(2) var<storage,read> blocks:array<u32>;
@group(0) @binding(3) var<storage,read> voxels:array<u32>;
@group(0) @binding(4) var<storage,read> palette:array<Pigment>;
@vertex fn vs_main(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{let p=vec2(f32((i<<1u)&2u),f32(i&2u));return vec4(p*2.-1.,0.,1.);}
struct Fragment {@location(0) color:vec4<f32>,@builtin(frag_depth) depth:f32};
fn forward_cell(p:vec3<f32>,d:vec3<f32>)->vec3<i32>{let f=floor(p);return vec3<i32>(f)-vec3<i32>(select(vec3<u32>(0),vec3<u32>(1),(d<vec3(0.))&(p==f)));}
fn light(normal:vec3<f32>)->vec3<f32>{
    var sun=normalize(vec3(-0.5,0.8,-0.3));if camera.sun.w>0.5{sun=camera.sun.xyz;}
    let sky=0.5+0.5*normal.y;
    let ambient=mix(vec3(0.12,0.145,0.075),vec3(0.25,0.33,0.43),sky)*(1.+camera.sky.x*0.3);
    let direct=vec3(2.,1.7,1.2)*max(dot(normal,sun),0.)*(1.-0.72*camera.sky.x);
    if camera.sun.w>0.5{return ambient+direct;}
    return mix(vec3(0.12,0.105,0.085),vec3(0.28,0.33,0.40),sky)+vec3(0.78,0.70,0.58)*max(dot(normal,sun),0.);
}
@fragment fn fs_main(@builtin(position) pixel:vec4<f32>,@builtin(sample_index) sample:u32)->Fragment {
    let sample_offsets=array<vec2<f32>,4>(vec2(0.375,0.125),vec2(0.875,0.375),vec2(0.125,0.625),vec2(0.625,0.875));
    var point=pixel.xy;
    if camera.viewport.z>1.5 {point=floor(pixel.xy)+sample_offsets[min(sample,3u)];}
    let ndc=vec2(point.x/camera.viewport.x*2.-1.,1.-point.y/camera.viewport.y*2.);
    let near_h=camera.inverse*vec4(ndc,0.,1.);let middle_h=camera.inverse*vec4(ndc,0.5,1.);
    let o=camera.eye.xyz*16.;let d=normalize(middle_h.xyz/middle_h.w-camera.eye.xyz);
    var t=length(near_h.xyz/near_h.w-camera.eye.xyz)*16.;
    var cell=forward_cell(o+d*t,d);var faces=0u;var was_solid=false;
    let extent=vec3<i32>(camera.extent.xyz);
    // A monotone ray crosses at most sum(extent) native grid planes. The bound
    // comes from the volume, never a heuristic step cap that can erase hits.
    let budget=camera.extent.x+camera.extent.y+camera.extent.z+8u;
    for(var iteration=0u;iteration<budget;iteration++) {
        if any(cell<vec3(0))||any(cell>=extent){break;}
        let c=vec3<u32>(cell);let cp=c/16u;
        var node=textureLoad(chunks,vec3<i32>(cp.yxz),0).x;
        var scale=16u;var bits=5u;var parent_lo=vec3(0);var parent_hi=extent;
        if (node&0x80000000u)!=0u {
            let bp=(c%16u)/4u;node=blocks[(node&0x3fffffffu)+bp.x+4u*(bp.y+4u*bp.z)];
            scale=4u;bits=2u;parent_lo=vec3<i32>(cp*16u);parent_hi=parent_lo+vec3(16);
            if (node&0x80000000u)!=0u {
                let vp=c%4u;let vi=vp.x+4u*(vp.y+4u*vp.z);
                node=voxels[(node&0x3fffffffu)+vi];
                scale=1u;parent_lo=vec3<i32>((c/4u)*4u);parent_hi=parent_lo+vec3(4);
            }
        }
        let solid=(node&0x40000000u)!=0u;
        if solid&&!was_solid&&faces!=0u {
            var face=0u;for(var k=0u;k<6u;k++){if (faces&(1u<<k))!=0u{face=k;break;}}
            var normal=vec3(0.);normal[face/2u]=select(-1.,1.,(face&1u)!=0u);
            let hit=(o+d*t)*0.0625;let clip=camera.matrix*vec4(hit,1.);let depth=clip.z/clip.w;
            if depth<0.||depth>1.{discard;}
            let ty=node&0x3fffffffu;let pigment=palette[ty];
            if camera.viewport.w>0.5{return Fragment(vec4(f32(pigment.material)/255.,0.,0.,1.),depth);}
            return Fragment(vec4(pigment.rgb*light(normal),1.),depth);
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
        if crossing<=t {discard;}
        cell=clamp(forward_cell(o+d*crossing,d),lo,hi-vec3(1));faces=0u;
        for(var a=0u;a<3u;a++){if next[a]==crossing{cell[a]=select(lo[a]-1,hi[a],d[a]>0.);faces|=1u<<(a*2u+select(1u,0u,d[a]>0.));}}
        t=crossing;
    }
    discard;
}
