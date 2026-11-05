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
    let layered=(color*grass+vec3<f32>(0.32,0.22,0.13)*soil+vec3<f32>(0.35,0.37,0.39)*stone)/span;
    return mix(color,layered,smoothstep(0.02,0.12,color.g-color.r));
}
fn voxel_appearance(color:vec3<f32>, normal:vec3<f32>, view:vec3<f32>, distance:f32) -> vec3<f32> {
    let n=normalize(normal);
    let sx=select(-1.,1.,n.x>=0.); let sz=select(-1.,1.,n.z>=0.);
    // Area-weighted native face distribution has no grazing-view singularity.
    // Use the direct projected distribution only nearby and when both the macro
    // surface and its treads face the eye well enough for stable normalization.
    // This is a stable distant appearance approximation, not a visibility oracle.
    let area=vec3<f32>(abs(n.x),max(n.y,0.),abs(n.z));
    let projected=max(n*view,vec3<f32>(0.));
    let angular=smoothstep(0.25,0.55,dot(n,view))*smoothstep(0.1,0.35,view.y);
    let nearby=1.-smoothstep(48.,96.,distance);
    let weights=mix(area,projected,angular*nearby);
    let light_x=terrain_lighting(vec3<f32>(sx,0.,0.));
    let light_y=terrain_lighting(vec3<f32>(0.,1.,0.));
    let light_z=terrain_lighting(vec3<f32>(0.,0.,sz));
    let side_x=column_side_color(color,abs(n.x)/max(n.y,0.00001));
    let side_z=column_side_color(color,abs(n.z)/max(n.y,0.00001));
    let sum=weights.x+weights.y+weights.z;
    return (side_x*weights.x*light_x+side_z*weights.z*light_z+color*weights.y*light_y)/max(sum,0.25);
}
// Coordinate-only paint: stable across chunk publication, no per-cell payload.
fn paint_hash(p:vec3<u32>) -> f32 {
    var h=p.x*1597334677u ^ p.y*3812015801u ^ p.z*2798796415u;
    h=(h^(h>>16u))*2246822519u; h=(h^(h>>13u))*3266489917u;
    return f32((h^(h>>16u))&65535u)/65535.;
}
fn paint_field(cell:vec3<u32>, shift:u32) -> f32 {
    let mask=0xffffffffu>>shift;
    let q=cell.xz>>vec2<u32>(shift);
    let fraction=(vec2<f32>(cell.xz&vec2<u32>((1u<<shift)-1u))+0.5)/f32(1u<<shift);
    let t=fraction*fraction*(3.-2.*fraction);
    let a=paint_hash(vec3<u32>(q.x,19u,q.y));
    let b=paint_hash(vec3<u32>((q.x+1u)&mask,19u,q.y));
    let c=paint_hash(vec3<u32>(q.x,19u,(q.y+1u)&mask));
    let d=paint_hash(vec3<u32>((q.x+1u)&mask,19u,(q.y+1u)&mask));
    return mix(mix(a,b,t.x),mix(c,d,t.x),t.y);
}
fn terrain_tint(cell:vec3<u32>, grain:f32) -> vec3<f32> {
    let broad=paint_field(cell,9u)-0.5; // 32m patches
    let local=paint_field(cell,6u)-0.5; // 4m patches
    let speckle=(paint_hash(cell)-0.5)*0.055*grain;
    return vec3<f32>(1.+broad*0.24+local*0.10+speckle,
                     1.+broad*0.15+local*0.08+speckle,
                     1.+broad*0.07+local*0.06+speckle);
}
struct Camera {
    matrix: mat4x4<f32>, centers:array<vec4<f32>,11>,
    cutout:vec4<f32>, eye:vec4<f32>, volume:vec4<f32>,
    morph_centers:array<vec4<f32>,11>, paint_origin:vec4<u32>,
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
    let eye=camera.morph_centers[level_index].xy;
    let distance=max(abs(ground.x-eye.x),abs(ground.y-eye.y))/step;
    let blend=smoothstep(42.,54.,distance)*level.z;
    o.position=camera.matrix*vec4<f32>(ground.x,mix(s.heights.x,s.heights.y,blend),ground.y,1.);
    o.normal=mix(s.normal.xyz,s.parent_normal.xyz,blend);
    // Product rule for H = Hfine + blend * (Hparent-Hfine). The residual
    // contributes a slope across the morph band; interpolating normals alone
    // misses it. Smooth the square-distance corner subgradient over one sample
    // to avoid a lighting discontinuity along its equal-distance diagonals.
    let t=clamp((distance-42.)/12.,0.,1.);
    let rate=6.*t*(1.-t)*level.z/(12.*step);
    let delta=ground-eye;
    let axis=smoothstep(-step,step,abs(delta.x)-abs(delta.y));
    let gradient=vec2<f32>(sign(delta.x)*axis,sign(delta.y)*(1.-axis))*rate;
    let correction=o.normal.y*(s.heights.y-s.heights.x)*gradient;
    o.normal.x-=correction.x; o.normal.z-=correction.y;
    o.color=mix(s.color.xyz,s.parent_color.xyz,blend); o.ground=ground; o.height=mix(s.heights.x,s.heights.y,blend); return o;
}
@fragment fn fs_main(i:Out)->@location(0) vec4<f32> {
    let owner=abs(i.ground-camera.volume.xy);
    if camera.volume.w>0. && max(owner.x,owner.y)<=camera.volume.z { discard; }
    let d=abs(i.ground-camera.cutout.xy);
    if camera.cutout.w>0. && max(d.x,d.y)<camera.cutout.z { discard; }
    let view=normalize(vec3<f32>(camera.eye.x-i.ground.x,camera.eye.z-i.height,camera.eye.y-i.ground.y));
    let exact=i.color*terrain_lighting(i.normal);
    let cell=bitcast<vec3<u32>>(vec3<i32>(floor(vec3<f32>(i.ground.x,i.height,i.ground.y)*16.)))+camera.paint_origin.xyz;
    let tint=terrain_tint(cell,0.);
    return vec4<f32>(tint*mix(exact,voxel_appearance(i.color,i.normal,view,length(vec3<f32>(camera.eye.x-i.ground.x,camera.eye.z-i.height,camera.eye.y-i.ground.y))),camera.eye.w),1.);
}
