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
fn voxel_appearance(color:vec3<f32>, normal:vec3<f32>, view:vec3<f32>, distance:f32) -> vec3<f32> {
    let n=normalize(normal);
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let sx=sign(n.x); let sz=sign(n.z);
    // Area-weighted native face distribution has no grazing-view singularity.
    // Use the direct projected distribution only nearby and when both the macro
    // surface and its treads face the eye well enough for stable normalization.
    // This is a stable distant appearance approximation, not a visibility oracle.
    let area=vec3<f32>(abs(n.x),max(n.y,0.),abs(n.z));
    let projected=max(n*view,vec3<f32>(0.));
    let angular=smoothstep(0.25,0.55,dot(n,view))*smoothstep(0.1,0.35,view.y);
    let nearby=1.-smoothstep(48.,96.,distance);
    let weights=mix(area,projected,angular*nearby);
    let lights=vec3<f32>(0.28+0.72*max(sx*sun.x,0.),0.28+0.72*sun.y,0.28+0.72*max(sz*sun.z,0.));
    let side_x=column_side_color(color,abs(n.x)/max(n.y,0.00001));
    let side_z=column_side_color(color,abs(n.z)/max(n.y,0.00001));
    let sum=weights.x+weights.y+weights.z;
    return (side_x*weights.x*lights.x+side_z*weights.z*lights.z+color*weights.y*lights.y)/max(sum,0.25);
}
struct Camera { view_projection: mat4x4<f32>, eye:vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) local:vec3<f32>,
};
@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>, @location(2) normal: vec3<f32>) -> VertexOut {
    var out: VertexOut;
    out.position = camera.view_projection * vec4<f32>(position, 1.0);
    out.color = color;
    out.normal = normal;
    out.local = position;
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Identical material/light transfer to terrain, per fragment so the collar's
    // interpolated endpoint normals share the terrain page's exact lighting.
    let sunlight = normalize(vec3<f32>(-0.5, 0.8, -0.3));
    let light = 0.28 + 0.72 * max(dot(normalize(in.normal), sunlight), 0.0);
    return vec4<f32>(in.color * light, 1.0);
}

struct TerrainCamera { matrix:mat4x4<f32>, centers:array<vec4<f32>,11>,cutout:vec4<f32>,eye:vec4<f32>,volume:vec4<f32>,morph_centers:array<vec4<f32>,11> };
struct TerrainSample { heights:vec4<f32>,normal:vec4<f32>,parent_normal:vec4<f32>,color:vec4<f32>,parent_color:vec4<f32> };
@group(1) @binding(0) var<uniform> terrain:TerrainCamera;
@group(1) @binding(1) var<storage,read> pages:array<TerrainSample>;
fn sample_index(p:vec2<i32>) -> u32 { return 16641u+u32((p.y+64)*129+p.x+64); }
@fragment fn fs_hybrid(i:VertexOut)->@location(0) vec4<f32> {
    let view=normalize(camera.eye.xyz-i.local);
    let ground=i.local.xz-camera.eye.xz+terrain.eye.xy-terrain.centers[1].xy;
    let owner=abs(ground+terrain.centers[1].xy-terrain.volume.xy);
    if terrain.volume.w>0. && max(owner.x,owner.y)<=terrain.volume.z { discard; }
    // The smaller singular value measures the best-resolved world direction.
    // A max derivative incorrectly declares a grazing but still wide voxel
    // unresolved and washes away native edges close to the eye.
    let dx=dpdx(i.local); let dy=dpdy(i.local);
    let xx=dot(dx,dx); let yy=dot(dy,dy); let xy=dot(dx,dy);
    let largest=0.5*(xx+yy+sqrt(max((xx-yy)*(xx-yy)+4.*xy*xy,0.)));
    let area=cross(dx,dy);
    let smallest=dot(area,area)/max(largest,0.00000001);
    let footprint=sqrt(smallest)/0.0625;
    // Filtering is confined to the owner perimeter. Inside it, resolvable
    // native faces wider than two pixels retain exact normals and canonical materials. The collar
    // reaches the identical proxy transfer at its shared outer endpoint.
    let delta=abs(ground+terrain.centers[1].xy-terrain.cutout.xy);
    let radius=max(delta.x,delta.y);
    let outer=terrain.cutout.z;
    let inner=outer-2.;
    let perimeter=smoothstep(inner-12.,inner,radius);
    let collar=smoothstep(inner,outer,radius);
    let unresolved=smoothstep(0.5,1.,footprint);
    let blend=mix(perimeter*unresolved,1.,collar)*terrain.eye.w;
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let exact=i.color*(0.28+0.72*max(dot(normalize(i.normal),sun),0.));
    // Most native fragments need no macro samples. Derivatives were evaluated
    // before this branch so neighboring fragments remain well-defined.
    if blend<=0. { return vec4<f32>(exact,1.); }
    let cell=clamp(vec2<i32>(floor(ground)),vec2<i32>(-64),vec2<i32>(63));
    let uv=clamp(ground-vec2<f32>(cell),vec2<f32>(0.),vec2<f32>(1.));
    let a=pages[sample_index(cell)]; let b=pages[sample_index(cell+vec2<i32>(1,0))];
    let c=pages[sample_index(cell+vec2<i32>(0,1))]; let d=pages[sample_index(cell+vec2<i32>(1,1))];
    var weights=vec4<f32>(1.-uv.x-uv.y,uv.x,uv.y,0.);
    if uv.x+uv.y>1. {weights=vec4<f32>(0.,1.-uv.y,1.-uv.x,uv.x+uv.y-1.);}
    let normal=a.normal.xyz*weights.x+b.normal.xyz*weights.y+c.normal.xyz*weights.z+d.normal.xyz*weights.w;
    let color=a.color.xyz*weights.x+b.color.xyz*weights.y+c.color.xyz*weights.z+d.color.xyz*weights.w;
    return vec4<f32>(mix(exact,voxel_appearance(color,normal,view,length(camera.eye.xyz-i.local)),blend),1.);
}
