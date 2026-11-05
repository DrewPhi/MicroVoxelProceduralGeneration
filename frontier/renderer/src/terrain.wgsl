// Projected visible area of a cubic staircase with this macro normal. Axis
// normals reduce to the exact native face response; subpixel stair sides retain
// their material coverage instead of becoming a smooth grass sheet.
fn column_side_color(color:vec3<f32>, slope:f32) -> vec3<f32> {
    // Canonical column: one grass voxel, eleven soil voxels, then stone.
    // This planar projected-coverage estimate is not a bound on rough terrain.
    let span=max(slope,0.00001);
    let grass=min(span,1.);
    let soil=clamp(span-1.,0.,11.);
    let stone=max(span-12.,0.);
    let layered=(color*grass+vec3<f32>(0.40,0.27,0.15)*soil+vec3<f32>(0.48,0.50,0.54)*stone)/span;
    return mix(color,layered,smoothstep(0.02,0.12,color.g-color.r));
}
fn voxel_appearance(color:vec3<f32>, normal:vec3<f32>, view:vec3<f32>) -> vec3<f32> {
    let n=normalize(normal);
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let sx=sign(n.x); let sz=sign(n.z);
    let weights=vec3<f32>(abs(n.x)*max(sx*view.x,0.),max(n.y,0.)*max(view.y,0.),abs(n.z)*max(sz*view.z,0.));
    let lights=vec3<f32>(0.28+0.72*max(sx*sun.x,0.),0.28+0.72*sun.y,0.28+0.72*max(sz*sun.z,0.));
    let side_x=column_side_color(color,abs(n.x)/max(n.y,0.00001));
    let side_z=column_side_color(color,abs(n.z)/max(n.y,0.00001));
    let sum=weights.x+weights.y+weights.z;
    let result=(side_x*weights.x*lights.x+side_z*weights.z*lights.z+color*weights.y*lights.y)/max(sum,0.00001);
    return select(color*(0.28+0.72*max(dot(n,sun),0.)),result,sum>0.00001);
}
struct Camera {
    matrix: mat4x4<f32>, centers:array<vec4<f32>,11>,
    cutout:vec4<f32>, eye:vec4<f32>,
};
struct Sample { heights:vec4<f32>, normal:vec4<f32>, parent_normal:vec4<f32>, color:vec4<f32>, parent_color:vec4<f32> };
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var<storage,read> pages:array<Sample>;
struct Out {
    @builtin(position) position:vec4<f32>, @location(0) normal:vec3<f32>,
    @location(1) color:vec3<f32>, @location(2) ground:vec2<f32>, @location(3) height:f32,
};
@vertex fn vs_main(@location(0) p:vec2<f32>,@location(1) index:u32,@location(2) morph:f32)->Out {
    let s=pages[index]; var o:Out;
    let level_index=index/16641u;
    let level=camera.centers[level_index];
    let ground=p+level.xy;
    let step=0.5*exp2(f32(level_index));
    // Camera-relative morph weights are continuous while immutable page centers
    // snap. All fine edges are fully parent, all coarse inner edges remain own.
    let eye=clamp(camera.eye.xy,level.xy-vec2<f32>(8.*step),level.xy+vec2<f32>(8.*step));
    let distance=max(abs(ground.x-eye.x),abs(ground.y-eye.y))/step;
    let blend=smoothstep(42.,54.,distance)*level.z;
    o.position=camera.matrix*vec4<f32>(ground.x,mix(s.heights.x,s.heights.y,blend),ground.y,1.);
    o.normal=mix(s.normal.xyz,s.parent_normal.xyz,blend);
    o.color=mix(s.color.xyz,s.parent_color.xyz,blend); o.ground=ground; o.height=mix(s.heights.x,s.heights.y,blend); return o;
}
@fragment fn fs_main(i:Out)->@location(0) vec4<f32> {
    let d=abs(i.ground-camera.cutout.xy);
    if camera.cutout.w>0. && max(d.x,d.y)<camera.cutout.z { discard; }
    let view=normalize(vec3<f32>(camera.eye.x-i.ground.x,camera.eye.z-i.height,camera.eye.y-i.ground.y));
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let exact=i.color*(0.28+0.72*max(dot(normalize(i.normal),sun),0.));
    return vec4<f32>(mix(exact,voxel_appearance(i.color,i.normal,view),camera.eye.w),1.);
}
