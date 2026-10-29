// Full opaque images are composited, so a LOD transition cannot punch holes
// through terrain. Work on both images exists only during a brief replacement.
struct Params { size:vec2<f32>, progress:f32, antialias:f32 }
@group(0) @binding(0) var current_image:texture_2d<f32>;
@group(0) @binding(1) var previous_image:texture_2d<f32>;
@group(0) @binding(2) var<uniform> params:Params;
@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    let p=array<vec2<f32>,3>(vec2<f32>(-1,-1),vec2<f32>(3,-1),vec2<f32>(-1,3));return vec4<f32>(p[i],0.,1.);
}
fn load(p:vec2<i32>,old:bool)->vec3<f32> {
    let q=clamp(p,vec2<i32>(0),vec2<i32>(params.size)-vec2<i32>(1));
    if old {return textureLoad(previous_image,q,0).rgb;}
    return textureLoad(current_image,q,0).rgb;
}
fn luma(c:vec3<f32>)->f32 {return dot(c,vec3<f32>(0.2126,0.7152,0.0722));}
fn filtered(p:vec2<i32>,old:bool)->vec3<f32> {
    let c=load(p,old);if params.antialias<0.5 {return c;}
    let n=load(p+vec2<i32>(0,-1),old);let s=load(p+vec2<i32>(0,1),old);
    let e=load(p+vec2<i32>(1,0),old);let w=load(p+vec2<i32>(-1,0),old);
    let lo=min(luma(c),min(min(luma(n),luma(s)),min(luma(e),luma(w))));
    let hi=max(luma(c),max(max(luma(n),luma(s)),max(luma(e),luma(w))));
    if hi-lo<max(0.035,hi*0.18) {return c;}
    // Bounded edge-aware filter, inspired by FXAA's contrast early-out. This
    // is not a port of FXAA and does not claim its full edge-search quality.
    let horizontal=abs(luma(n)+luma(s)-2.*luma(c));
    let vertical=abs(luma(e)+luma(w)-2.*luma(c));
    let pair=select((e+w)*0.5,(n+s)*0.5,horizontal>vertical);
    return mix(c,pair,0.28);
}
@fragment fn fs(@builtin(position) pixel:vec4<f32>)->@location(0) vec4<f32> {
    let p=vec2<i32>(pixel.xy);
    // A stable 4x4 Bayer mask chooses between complete images, including sky.
    // There is no alpha-blended double surface or missing depth coverage.
    let bayer=array<f32,16>(0.,8.,2.,10.,12.,4.,14.,6.,3.,11.,1.,9.,15.,7.,13.,5.);
    let index=u32(p.x&3)+4u*u32(p.y&3);
    let old=params.progress<(bayer[index]+0.5)/16.;
    return vec4<f32>(filtered(p,old),1.);
}
