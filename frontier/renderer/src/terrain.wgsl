struct Camera { matrix: mat4x4<f32>, centers:array<vec4<f32>,7> };
struct Sample { heights:vec4<f32>, normal:vec4<f32>, parent_normal:vec4<f32>, color:vec4<f32>, parent_color:vec4<f32> };
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var<storage,read> pages:array<Sample>;
struct Out { @builtin(position) position:vec4<f32>, @location(0) normal:vec3<f32>, @location(1) color:vec3<f32> };
@vertex fn vs_main(@location(0) p:vec2<f32>,@location(1) index:u32,@location(2) morph:f32)->Out {
    let s=pages[index]; var o:Out;
    let offset=camera.centers[index/16641u].xy;
    o.position=camera.matrix*vec4<f32>(p.x+offset.x,mix(s.heights.x,s.heights.y,morph),p.y+offset.y,1.);
    o.normal=mix(s.normal.xyz,s.parent_normal.xyz,morph); o.color=mix(s.color.xyz,s.parent_color.xyz,morph); return o;
}
@fragment fn fs_main(i:Out)->@location(0) vec4<f32> {
    let n=normalize(i.normal); let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let light=0.28+0.72*max(dot(n,sun),0.);
    return vec4<f32>(i.color*light,1.);
}
