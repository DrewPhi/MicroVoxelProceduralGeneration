// Shared linear-light response for native faces, the collar and terrain pages.
// Warm directional sunlight keeps voxel faces legible; cool sky and restrained
// ground bounce provide hemispherical fill without flattening the shaded faces.
// No visibility/shadow approximation or camera-dependent exposure is applied.
fn terrain_lighting(normal:vec3<f32>) -> vec3<f32> {
    let n=normalize(normal);
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let sky=0.5+0.5*n.y;
    let ambient=mix(vec3<f32>(0.12,0.105,0.085),vec3<f32>(0.28,0.33,0.40),sky);
    return ambient+vec3<f32>(0.78,0.70,0.58)*max(dot(n,sun),0.);
}
// Exact first exposed front face, matching native mesh's local-meter depth and
// flat material/Lambert shading. No artificial fog or reduced internal resolution.
struct Camera {
    matrix:mat4x4<f32>, inverse:mat4x4<f32>, eye:vec4<f32>,
    extent:vec4<u32>, bricks:vec4<u32>, viewport:vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var<storage,read> headers:array<u32>;
@group(0) @binding(2) var<storage,read> dense:array<u32>;
@group(0) @binding(3) var<storage,read> directional:array<u32>;
@vertex fn vs_main(@builtin(vertex_index) index:u32)->@builtin(position) vec4<f32> {
    let p=vec2<f32>(f32((index<<1u)&2u),f32(index&2u));
    return vec4<f32>(p*2.-1.,0.,1.);
}
struct Fragment { @location(0) color:vec4<f32>, @builtin(frag_depth) depth:f32 };
fn color(id:u32)->vec3<f32> {
    switch id {
        case 1u:{return vec3<f32>(0.18,0.34,0.10);}
        case 2u:{return vec3<f32>(0.32,0.22,0.13);}
        case 3u:{return vec3<f32>(0.35,0.37,0.39);}
        case 4u:{return vec3<f32>(0.41,0.25,0.13);}
        default:{return vec3<f32>(0.);}
    }
}
fn forward_cell(p:vec3<f32>,d:vec3<f32>)->vec3<i32> {
    let f=floor(p);
    return vec3<i32>(f)-vec3<i32>(select(vec3<u32>(0u),vec3<u32>(1u),(d<vec3<f32>(0.))&(p==f)));
}
@fragment fn fs_main(@builtin(position) pixel:vec4<f32>)->Fragment {
    let ndc=vec2<f32>(pixel.x/camera.viewport.x*2.-1.,1.-pixel.y/camera.viewport.y*2.);
    let near_h=camera.inverse*vec4<f32>(ndc,0.,1.);
    let mid_h=camera.inverse*vec4<f32>(ndc,0.5,1.);
    let o=camera.eye.xyz*16.;
    let d=normalize(mid_h.xyz/mid_h.w-camera.eye.xyz);
    let near_t=length(near_h.xyz/near_h.w-camera.eye.xyz)*16.;
    let extent=vec3<f32>(camera.extent.xyz);
    var entry=vec3<f32>(-1e30); var exit=vec3<f32>(1e30);
    for(var a=0u;a<3u;a++) {
        if d[a]==0. {if o[a]<0. || o[a]>=extent[a] {discard;}}
        else { let t0=-o[a]/d[a];let t1=(extent[a]-o[a])/d[a];entry[a]=min(t0,t1);exit[a]=max(t0,t1); }
    }
    let enter=max(entry.x,max(entry.y,entry.z));
    let end=min(exit.x,min(exit.y,exit.z));
    var t=max(near_t,enter);
    if t>=end {discard;}
    var cell=forward_cell(o+d*t,d);
    var entered_faces=0u;
    if enter>=near_t {
        // Retain every simultaneously entered face. A solid neighbor may hide
        // the lowest-axis face while another tied face is genuinely exposed.
        for(var a=0u;a<3u;a++) { if entry[a]==enter {
            cell[a]=select(i32(camera.extent[a])-1,0,d[a]>0.);
            let face=a*2u+select(1u,0u,d[a]>0.);
            entered_faces|=1u<<face;
        }}
    }
    // At most sum(extent)+sum(bricks) crossings. Bound derives from the actual
    // finite volume rather than a heuristic ray length that can erase geometry.
    let budget=camera.extent.x+camera.extent.y+camera.extent.z+8u;
    for(var iteration=0u;iteration<budget;iteration++) {
        if any(cell<vec3<i32>(0)) || any(cell>=vec3<i32>(camera.extent.xyz)) {break;}
        let b=vec3<u32>(cell)/8u;
        let bi=(b.z*camera.bricks.y+b.y)*camera.bricks.x+b.x;
        let header=headers[bi*2u];
        let is_dense=(header&0x80000000u)!=0u;
        var word=header;
        if is_dense {
            let p=vec3<u32>(cell)%8u;
            let i=(p.z*8u+p.y)*8u+p.x;
            word=(dense[(header&0x7fffffffu)+i/2u]>>((i%2u)*16u))&65535u;
        }
        let exposed_entered=entered_faces&((word>>8u)&63u);
        if exposed_entered!=0u {
            // Lowest exposed entered axis is deterministic. On a zero-area tie
            // this may differ from canonical DDA's lowest-axis normal, but the
            // occupied cell must never disappear because that face is hidden.
            var face=0u;
            for(var candidate=0u;candidate<6u;candidate++) {
                if (exposed_entered&(1u<<candidate))!=0u {face=candidate;break;}
            }
            let hit=(o+d*t)*0.0625;
            let clip=camera.matrix*vec4<f32>(hit,1.);
            let depth=clip.z/clip.w;
            if depth<0. || depth>1. {discard;}
            var normal=vec3<f32>(0.);
            normal[face/2u]=select(-1.,1.,(face&1u)!=0u);
            var result:Fragment;
            result.color=vec4<f32>(color(word&255u)*terrain_lighting(normal),1.);
            result.depth=depth;
            return result;
        }
        var lo=cell;var hi=cell+vec3<i32>(1);
        // Uniform bricks have identical material AND exposed-face masks. After
        // testing the entry face, their interior cannot introduce a new surface.
        if !is_dense {
            var radius=0i;
            if header==0u && camera.extent.w==1u {radius=i32(headers[bi*2u+1u]);}
            lo=max((vec3<i32>(b)-vec3<i32>(radius))*8,vec3<i32>(0));
            hi=min((vec3<i32>(b)+vec3<i32>(radius+1))*8,vec3<i32>(camera.extent.xyz));
            if header==0u && camera.extent.w==2u {
                let positive=d>=vec3<f32>(0.);
                let octant=select(0u,1u,positive.x)+select(0u,2u,positive.y)+select(0u,4u,positive.z);
                let packed=directional[bi*8u+octant];
                let length=vec3<i32>(i32(packed&1023u),i32((packed>>10u)&1023u),i32((packed>>20u)&1023u));
                // Zero is invalid/unavailable: retain ordinary brick DDA.
                if all(length>vec3<i32>(0)) {
                    let base=vec3<i32>(b);
                    lo=max(select(base-length+vec3<i32>(1),base,positive)*8,vec3<i32>(0));
                    hi=min(select(base+vec3<i32>(1),base+length,positive)*8,vec3<i32>(camera.extent.xyz));
                }
            }
        }
        var next=vec3<f32>(1e30);
        for(var a=0u;a<3u;a++) {if d[a]!=0. {next[a]=(f32(select(lo[a],hi[a],d[a]>0.))-o[a])/d[a];}}
        let crossing=min(next.x,min(next.y,next.z));
        if crossing>=end {break;}
        // Keep non-crossed coordinates inside the region just traversed.
        // Rounded multiply/add at a different axis plane must not jump a cell.
        cell=clamp(forward_cell(o+d*crossing,d),lo,hi-vec3<i32>(1));
        entered_faces=0u;
        for(var a=0u;a<3u;a++) {if next[a]==crossing {
            cell[a]=select(lo[a]-1,hi[a],d[a]>0.);
            let face=a*2u+select(1u,0u,d[a]>0.);
            entered_faces|=1u<<face;
        }}
        t=crossing;
    }
    discard;
}
