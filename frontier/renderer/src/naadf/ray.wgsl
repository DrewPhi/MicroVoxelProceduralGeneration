override traversal_diagnostic:bool = false;
override traversal_beam:bool = false;
@group(2) @binding(0) var beam_prefix:texture_2d<u32>;
@group(1) @binding(0) var far_depth:texture_depth_2d;
@group(1) @binding(1) var far_sampler:sampler_comparison;
@vertex fn vs_main(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{let p=vec2(f32((i<<1u)&2u),f32(i&2u));return vec4(p*2.-1.,0.,1.);}
struct Fragment {@location(0) color:vec4<f32>,@builtin(frag_depth) depth:f32};
fn distant_sun_visibility(hit:vec3<f32>,sun:vec3<f32>)->f32 {
    if naadf.shadow.x<0.5{return 1.;}
    // Compare at the ray's exit, so shadow geometry inside the NAADF box
    // cannot substitute for, or double-shadow, our exact voxel query.
    let interval=voxel_interval(hit*16.,sun);
    let receiver=hit+sun*max(0.,interval.y*0.0625+0.002)-naadf.eye.xyz;
    let clip=naadf.far_matrix*vec4(receiver,1.);let q=clip.xyz/clip.w;
    let uv=q.xy*vec2(0.5,-0.5)+0.5;
    if any(uv<vec2(0.002))||any(uv>vec2(0.998))||q.z<=0.||q.z>=1.{return 1.;}
    return textureSampleCompareLevel(far_depth,far_sampler,uv,q.z-0.0002);
}
fn light(normal:vec3<f32>,visibility:f32)->vec3<f32>{
    var sun=normalize(vec3(-0.5,0.8,-0.3));if naadf.sun.w>0.5{sun=naadf.sun.xyz;}
    let sky=0.5+0.5*normal.y;
    let ambient=mix(vec3(0.12,0.145,0.075),vec3(0.25,0.33,0.43),sky)*(1.+naadf.sky.x*0.3);
    let direct=vec3(2.,1.7,1.2)*max(dot(normal,sun),0.)*(1.-0.72*naadf.sky.x);
    if naadf.sun.w>0.5{return ambient+direct*visibility;}
    return mix(vec3(0.12,0.105,0.085),vec3(0.28,0.33,0.40),sky)+vec3(0.78,0.70,0.58)*max(dot(normal,sun),0.);
}
@fragment fn fs_main(@builtin(position) pixel:vec4<f32>,@builtin(sample_index) sample:u32)->Fragment {
    let offsets=array<vec2<f32>,4>(vec2(0.375,0.125),vec2(0.875,0.375),vec2(0.125,0.625),vec2(0.625,0.875));
    var point=pixel.xy;if naadf.viewport.z>1.5{point=floor(point)+offsets[min(sample,3u)];}
    let ndc=vec2(point.x/naadf.viewport.x*2.-1.,1.-point.y/naadf.viewport.y*2.);
    // Work in camera direction space: subtracting the eye after inverse-VP
    // unprojection loses angular precision as the snapshot origin moves away.
    let o=naadf.eye.xyz*16.;
    let d=normalize(naadf.forward.xyz+ndc.x*naadf.right.xyz*naadf.right.w+ndc.y*naadf.up.xyz*naadf.up.w);
    let cosine=dot(d,naadf.forward.xyz);
    let minimum=naadf.projection.x/cosine*16.;
    var prefix=minimum;
    if traversal_beam {
        let steps=textureLoad(beam_prefix,vec2<i32>(vec2<u32>(point)/8u),0).x;
        prefix=max(minimum,f32(steps)/cosine);
        if traversal_metrics {
            let interval=voxel_interval(o,d);
            let ordinary=max(minimum,max(0.,interval.x));
            trace_metric(2u,10u,1u);
            if prefix>ordinary && ordinary<interval.y {
                trace_metric(2u,9u,1u);
                if prefix>=interval.y {trace_metric(2u,11u,1u);}
                trace_distance(2u,17u,max(0.,min(prefix,interval.y)-ordinary));
            }
        }
    }
    let h=trace_prefixed(o,d,minimum,prefix,false,traversal_diagnostic);
    if traversal_diagnostic {
        return Fragment(vec4(vec3<f32>(h.visits),1.),0.);
    }
    if !h.found{discard;}
    let hit=(o+d*h.t)*0.0625;
    // Storage has a native-cell apron; raster owns the outermost layer.
    let center=(floor(hit*16.-h.normal*0.01)+0.5)*0.0625;
    if naadf.viewport.w<0.5 && (any(center<vec3(0.0625))||any(center>=vec3<f32>(naadf.extent.xyz)*0.0625-vec3(0.0625))){discard;}
    let depth=naadf.projection.z+naadf.projection.w/(h.t*0.0625*cosine);
    if depth<0.||depth>1.{discard;}
    let pigment=palette[h.pigment];
    if naadf.viewport.w>1.5 {
        let blocked=trace_voxels((hit+h.normal*0.001)*16.,normalize(naadf.sun.xyz),0.,true).found;
        return Fragment(vec4(select(0.,1.,blocked),0.,0.,1.),depth);
    }
    if naadf.viewport.w>0.5{return Fragment(vec4(f32(pigment.material)/255.,0.,0.,1.),depth);}
    if naadf.sky.w>0.5{return Fragment(vec4(vec3(0.8)*(0.3+0.7*max(dot(h.normal,normalize(vec3(-0.5,0.8,-0.3))),0.)),1.),depth);}
    var visibility=1.;
    if naadf.sun.w>0.5 && dot(h.normal,naadf.sun.xyz)>0. {
        if naadf.shadow.y<0.5{visibility=naadf_sun_visibility(hit-naadf.eye.xyz,h.normal,naadf.sun.xyz);}
        if visibility>0.5{visibility*=distant_sun_visibility(hit,naadf.sun.xyz);}
    }
    return Fragment(vec4(pigment.rgb*light(h.normal,visibility),1.),depth);
}
