// Bounded directional single scattering. Density is anchored to the sampled
// terrain surface. The sun's shadow maps carve beams out of that medium.
struct AtmosphereUniform {
    inverse_view_projection:mat4x4<f32>, relative_eye:vec4<f32>,world_eye:vec4<f32>,
    sun:vec4<f32>,fog:vec4<f32>,weather:vec4<f32>,beauty:vec4<f32>,
    lighting: vec4<f32>, // shafts, streaming haze, region count, reserved
    far_shadow: mat4x4<f32>,
    haze_regions: array<vec4<f32>,8>,
};
@group(0) @binding(0) var<uniform> atmosphere:AtmosphereUniform;
// DEPTH_BINDING
struct NearShadow {projection:mat4x4<f32>,parameters:vec4<f32>};
@group(1) @binding(0) var shadow_depth:texture_depth_2d;
@group(1) @binding(1) var shadow_sampler:sampler_comparison;
@group(1) @binding(2) var<uniform> shadow:NearShadow;
@group(2) @binding(0) var far_depth:texture_depth_2d;
@group(2) @binding(1) var far_sampler:sampler_comparison;
struct VertexOutput {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>};
@vertex fn vs_main(@builtin(vertex_index) i:u32)->VertexOutput {
    var p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    var o:VertexOutput;o.position=vec4(p[i],0.,1.);o.uv=vec2(p[i].x*.5+.5,.5-p[i].y*.5);return o;
}
fn reconstruct(uv:vec2<f32>,d:f32)->vec3<f32> {
    let p=atmosphere.inverse_view_projection*vec4(uv.x*2.-1.,1.-uv.y*2.,d,1.);
    return p.xyz/p.w-atmosphere.relative_eye.xyz;
}
fn visibility(p:vec3<f32>,distance:f32,forward:f32)->f32 {
    let distant=(atmosphere.far_shadow*vec4(p,1.)).xyz;
    let far_uv=distant.xy*vec2(0.5,-0.5)+0.5;
    var visible=1.;
    if all(far_uv>=vec2(.005)) && all(far_uv<=vec2(.995)) &&
        distant.z>=0. && distant.z<=1. {
        visible=textureSampleCompareLevel(far_depth,far_sampler,far_uv,distant.z-.00024);
    }
    let q=(shadow.projection*vec4(p,1.)).xyz;
    let uv=q.xy*vec2(.5,-.5)+0.5;
    let edge=min(min(uv.x,uv.y),min(1.-uv.x,1.-uv.y));
    if edge>.005 && q.z>=0. && q.z<=1. {
        let near=textureSampleCompareLevel(shadow_depth,shadow_sampler,uv,q.z-.0003);
        visible=min(visible,mix(1.,near,smoothstep(.005,.09,edge)));
    }
    // Local voxel edits and canopy gaps can be too small to reach the
    // raster maps. Query only nearby scattering facing the sun, after the
    // maps have already rejected any known blocker. A short bounded ray
    // covers the trees and close geometry that can give the sharpest shafts.
    let exact_weight=(1.-smoothstep(15.,26.,distance))*smoothstep(.72,.88,forward);
    if visible>0.01 && exact_weight>0.01 && naadf.extent.x>0u {
        let origin=(p+naadf.eye.xyz)*VOXELS_PER_METER;
        let extent=vec3<f32>(naadf.extent.xyz);
        if all(origin>=vec3(1.)) && all(origin<extent-vec3(1.)) {
            let ray=trace_impl_mode(origin,atmosphere.sun.xyz,0.,true,false,false,true,256.);
            if ray.found {visible*=1.-exact_weight;}
        }
    }
    return visible;
}
@fragment fn fs_main(i:VertexOutput)->@location(0) vec4<f32> {
    let size=vec2<i32>(textureDimensions(depth));
    let pixel=clamp(vec2<i32>(i.uv*vec2<f32>(size)),vec2(0),size-vec2(1));
    let d=DEPTH_LOAD;
    let offset=reconstruct(i.uv,d);
    let endpoint=select(length(offset),4000.,d<=0.);
    if atmosphere.beauty.y<.5 || shadow.parameters.x<.5 || atmosphere.fog.x<=0. || atmosphere.sun.y<=0. {
        return vec4(0.,0.,0.,endpoint);
    }
    let ray=normalize(offset);
    let ray_end=min(endpoint,54.);
    let cosine=dot(ray,atmosphere.sun.xyz);
    let g=0.58;
    let phase=(1.-g*g)/pow(max(1.+g*g-2.*g*cosine,0.08),1.5);
    // Small extinction keeps open air clear and permits the sun's visibility
    // to sculpt the bright paths. Segment lengths compensate the nonuniform steps.
    let base_density=min(atmosphere.fog.x*64.,0.004);
    var transmittance=1.;
    var lit=0.;
    var integrated=0.;
    for(var k=0u;k<12u;k+=1u) {
        let a=f32(k)/12.;
        let b=(f32(k)+1.)/12.;
        let start=mix(a,a*a,.55)*ray_end;
        let end=mix(b,b*b,.55)*ray_end;
        let distance=(start+end)*.5;
        let point=ray*distance;
        var ground=atmosphere.world_eye.w-atmosphere.world_eye.y;
        if ground_available() {ground=ground_sample(point).a;}
        let height=point.y-ground;
        let vertical=exp(-max(height,0.)*.12)*smoothstep(-2.5,0.15,height);
        let distance_fade=1.-smoothstep(30.,54.,distance);
        let depth=base_density*vertical*distance_fade*(end-start);
        let weight=transmittance*(1.-exp(-depth));
        if weight>0.000004 {lit+=weight*visibility(point,distance,cosine);}
        integrated+=weight;
        transmittance*=exp(-depth);
    }
    if atmosphere.beauty.z>1.5 && atmosphere.beauty.z<2.5 {
        return vec4(vec3(lit/max(integrated,.000001)),endpoint);
    }
    let warm=outdoor_sun_color(atmosphere.sun.y)*.5;
    let light=warm*(min(phase,9.)*atmosphere.lighting.x*(1.-atmosphere.weather.z*.8));
    return vec4(light*lit*.65,endpoint);
}
