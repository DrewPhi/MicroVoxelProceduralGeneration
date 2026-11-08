// Greenfield bounded single scattering, inspired by Wronski SIGGRAPH 2014:
// visibility is evaluated INSIDE the medium, rather than blurring lit pixels.
// Source: https://bartwronski.com/wp-content/uploads/2014/08/bwronski_volumetric_fog_siggraph2014.pdf
// This is a reduced-resolution bounded ray integral, not a full froxel grid.
// Scattering/transmittance separation also follows Hillaire, EGSR 2020:
// https://sebh.github.io/publications/egsr2020.pdf
// Half-resolution, 16 deterministic world-ray samples, 56 m maximum extent.
struct AtmosphereUniform {
    inverse_view_projection:mat4x4<f32>, relative_eye:vec4<f32>,world_eye:vec4<f32>,
    sun:vec4<f32>,fog:vec4<f32>,weather:vec4<f32>,beauty:vec4<f32>,
};
@group(0) @binding(0) var<uniform> atmosphere:AtmosphereUniform;
// DEPTH_BINDING
struct NearShadow {projection:mat4x4<f32>,parameters:vec4<f32>};
@group(1) @binding(0) var shadow_depth:texture_depth_2d;
@group(1) @binding(1) var shadow_sampler:sampler_comparison;
@group(1) @binding(2) var<uniform> shadow:NearShadow;
struct VertexOutput {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>};
@vertex fn vs_main(@builtin(vertex_index) i:u32)->VertexOutput {
    var p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    var o:VertexOutput;o.position=vec4(p[i],0.,1.);o.uv=vec2(p[i].x*.5+.5,.5-p[i].y*.5);return o;
}
fn reconstruct(uv:vec2<f32>,d:f32)->vec3<f32> {
    let p=atmosphere.inverse_view_projection*vec4(uv.x*2.-1.,1.-uv.y*2.,d,1.);
    return p.xyz/p.w-atmosphere.relative_eye.xyz;
}
fn visibility(p:vec3<f32>)->f32 {
    let q=(shadow.projection*vec4(p,1.)).xyz;
    let uv=q.xy*vec2(.5,-.5)+.5;
    if any(uv<vec2(.005)) || any(uv>vec2(.995)) || q.z<0. || q.z>1. {return 1.;}
    return textureSampleCompareLevel(shadow_depth,shadow_sampler,uv,q.z-.0003);
}
@fragment fn fs_main(i:VertexOutput)->@location(0) vec4<f32> {
    let size=vec2<i32>(textureDimensions(depth));
    let pixel=clamp(vec2<i32>(i.uv*vec2<f32>(size)),vec2(0),size-vec2(1));
    let d=DEPTH_LOAD;
    let offset=reconstruct(i.uv,d);
    let endpoint=select(length(offset),4000.,d>=1.);
    if atmosphere.beauty.y<.5 || shadow.parameters.x<.5 || atmosphere.fog.x<=0. || atmosphere.sun.y<=0. {
        return vec4(0.,0.,0.,endpoint);
    }
    let ray=normalize(offset);
    let distance=min(endpoint,56.);
    // Thin meadow pollen/mist envelope rests on the local terrain plane, with a
    // smooth 56 m radius. Density and maximum optical depth stay bounded; it
    // cannot conceal distant terrain or depend on streaming coverage.
    let density_scale=min(atmosphere.fog.x*60.,.0045);
    let cosine=dot(ray,atmosphere.sun.xyz);
    let g=.55;
    // Henyey-Greenstein shape normalized to unit mean over the sphere.
    let phase=(1.-g*g)/pow(max(1.+g*g-2.*g*cosine,.08),1.5);
    var transmittance=1.;var scattering=0.;var unshadowed=0.;
    // Quadratic partitions concentrate precision at the nearby canopy rather
    // than spending most steps on empty distant air. Integrate each segment
    // with its actual length so energy stays independent of sample spacing.
    for(var k=0u;k<16u;k+=1u) {
        let a=f32(k)/16.;let b=(f32(k)+1.)/16.;
        let start=a*a*distance;let end=b*b*distance;
        let step=end-start;let t=(start+end)*.5;let p=ray*t;
        let height=atmosphere.world_eye.y+p.y-atmosphere.world_eye.w;
        let vertical=exp(-max(height,0.)*.065);
        let local=1.-smoothstep(18.,56.,length(p));
        let extinction=density_scale*vertical*local;
        let segment=1.-exp(-extinction*step);
        scattering+=transmittance*segment*visibility(p);
        unshadowed+=transmittance*segment;
        transmittance*=1.-segment;
    }
    // Same medium weights and same actual shadow comparisons as presentation.
    // This ratio separates shadow-mask structure from sun hue/phase/gain.
    if atmosphere.beauty.z>1.5 && atmosphere.beauty.z<2.5 {
        return vec4(vec3(scattering/max(unshadowed,.000001)),endpoint);
    }
    let warm=mix(vec3(1.,.93,.78),vec3(1.,.84,.64),1.-smoothstep(.04,.4,atmosphere.sun.y));
    // Keep broad-angle air clear while allowing the real shadow map to
    // carve strong shafts when looking toward the sun through the crown.
    // Canopy shafts extend tens of degrees below the solar disc. A broad
    // Mie lobe admits those rays; the measured canopy contrast below, rather
    // than a narrow angular cutoff, limits the open-meadow wash.
    let forward_focus=.04+.96*smoothstep(.60,.96,cosine);
    // Artistic canopy-pollen contrast: retain the light/shadow corridors proved
    // by the actual integral, while reducing the uniform open-meadow veil.
    // This is not an extra physical scattering order or an occlusion query.
    let lit_fraction=scattering/max(unshadowed,.000001);
    let canopy_contrast=.12+.88*smoothstep(.02,.18,1.-lit_fraction);
    let illumination=warm*min(phase,10.)*.95*forward_focus*(1.-atmosphere.weather.z*.8);
    return vec4(illumination*scattering*canopy_contrast,endpoint);
}
