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
struct Camera {matrix:mat4x4<f32>,centers:array<vec4<f32>,11>,eye:vec4<f32>};
struct Sample {heights:vec4<f32>,normal:vec4<f32>,parent_normal:vec4<f32>,color:vec4<f32>,parent_color:vec4<f32>};
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var<storage,read> pages:array<Sample>;
struct Out { @builtin(position) position:vec4<f32>, @location(0) normal:vec3<f32>, @location(1) color:vec3<f32>, @location(2) ground:vec2<f32>, @location(3) height:f32 };
@vertex fn vs_main(@location(0) grid:vec2<u32>,@location(1) offset:vec2<f32>,@location(2) level:u32,@location(3) edges:u32)->Out {
    var p=grid;
    var parent=false;
    if (grid.x==0u && (edges&1u)!=0u)||(grid.x==16u && (edges&2u)!=0u) {p.y-=p.y%2u;parent=true;}
    if (grid.y==0u && (edges&4u)!=0u)||(grid.y==16u && (edges&8u)!=0u) {p.x-=p.x%2u;parent=true;}
    if (grid.x==0u||grid.x==16u)&&(grid.y==0u||grid.y==16u) {
        let corner=select(0u,1u,grid.x==16u)+select(0u,2u,grid.y==16u);
        parent=parent||((edges&(1u<<(4u+corner)))!=0u);
    }
    let step=0.5*exp2(f32(level));
    let ground=offset+vec2<f32>(p)*step;
    let address=vec2<i32>(round((ground-camera.centers[level].xy)/step))+vec2<i32>(64);
    let index=level*16641u+u32(address.y*129+address.x);
    let s=pages[index];
    let h=select(s.heights.x,s.heights.y,parent);
    var out:Out;
    out.position=camera.matrix*vec4<f32>(ground.x,h,ground.y,1.);
    out.normal=select(s.normal.xyz,s.parent_normal.xyz,parent);
    out.color=select(s.color.xyz,s.parent_color.xyz,parent);
    out.ground=ground;out.height=h;return out;
}
@fragment fn fs_main(i:Out)->@location(0) vec4<f32> {
    let view=normalize(vec3<f32>(camera.eye.x-i.ground.x,camera.eye.z-i.height,camera.eye.y-i.ground.y));
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let exact=i.color*(0.28+0.72*max(dot(normalize(i.normal),sun),0.));
    return vec4<f32>(mix(exact,voxel_appearance(i.color,i.normal,view),camera.eye.w),1.);
}
