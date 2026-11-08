// Shared linear-light response for native faces, the collar and terrain pages.
// Warm directional sunlight keeps voxel faces legible; cool sky and restrained
// ground bounce provide hemispherical fill without flattening the shaded faces.
// No visibility/shadow approximation or camera-dependent exposure is applied.
fn legacy_terrain_lighting(normal:vec3<f32>) -> vec3<f32> {
    let n=normalize(normal);
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let sky=0.5+0.5*n.y;
    let ambient=mix(vec3<f32>(0.12,0.105,0.085),vec3<f32>(0.28,0.33,0.40),sky);
    return ambient+vec3<f32>(0.78,0.70,0.58)*max(dot(n,sun),0.);
}
fn terrain_lighting(normal:vec3<f32>, visibility:f32) -> vec3<f32> {
    let n=normalize(normal);
    if camera.sun.w > 0.5 {
        let sky=0.5+0.5*n.y;
        let overcast=camera.sky.x;
        let ambient=mix(vec3(0.15,0.14,0.115),vec3(0.29,0.36,0.44),sky);
        let direct=vec3(0.83,0.75,0.60)*max(dot(n,camera.sun.xyz),0.);
        return ambient*(1.+overcast*0.3)+direct*(1.-0.72*overcast)*visibility;
    }
    return legacy_terrain_lighting(normal);
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
struct Camera { view_projection: mat4x4<f32>, eye:vec4<f32>, paint_origin:vec4<u32>, sun:vec4<f32>, sky:vec4<f32>, paint:vec4<f32>, shadow_projection:mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var shadow_depth: texture_depth_2d;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;
// Finite map of published terrain, not a full-world or volume visibility oracle.
fn terrain_shadow(local:vec3<f32>) -> f32 {
    if camera.sky.y < 0.5 { return 1.; }
    let clip = camera.shadow_projection*vec4<f32>(local,1.);
    let p = clip.xyz/clip.w;
    let uv = p.xy*vec2<f32>(0.5,-0.5)+0.5;
    if any(uv<vec2<f32>(0.002)) || any(uv>vec2<f32>(0.998)) || p.z<=0. || p.z>=1. { return 1.; }
    let texel = 1./vec2<f32>(textureDimensions(shadow_depth));
    let depth = p.z-0.000025;
    let a = textureSampleCompareLevel(shadow_depth,shadow_sampler,uv+vec2(-0.65,-0.65)*texel,depth);
    let b = textureSampleCompareLevel(shadow_depth,shadow_sampler,uv+vec2(0.65,-0.65)*texel,depth);
    let c = textureSampleCompareLevel(shadow_depth,shadow_sampler,uv+vec2(-0.65,0.65)*texel,depth);
    let d = textureSampleCompareLevel(shadow_depth,shadow_sampler,uv+vec2(0.65,0.65)*texel,depth);
    // Fade at the finite boundary to avoid a visible moving square.
    let edge = min(min(uv.x,uv.y),min(1.-uv.x,1.-uv.y));
    return mix(1.,(a+b+c+d)*0.25,smoothstep(0.002,0.04,edge));
}
struct FaceProfile {top:vec4<f32>,px:vec4<f32>,nx:vec4<f32>,pz:vec4<f32>,nz:vec4<f32>};
@group(0) @binding(1) var<storage,read> face_profiles:array<FaceProfile>;
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) local:vec3<f32>,
    @location(3) @interpolate(flat) profile_index:u32,
};
@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>, @location(2) normal: vec3<f32>,
    @location(3) profile_index:u32) -> VertexOut {
    return vertex_out(position,color,normal,profile_index);
}
fn vertex_out(position:vec3<f32>,color:vec3<f32>,normal:vec3<f32>,profile_index:u32)->VertexOut {
    var out:VertexOut;
    out.position=camera.view_projection*vec4<f32>(position,1.);
    out.color=color;out.normal=normal;out.local=position;
    out.profile_index=profile_index;
    return out;
}
// Lossless triangle records retain the original indexed vertex sequence.
@vertex fn vs_proxy(@builtin(vertex_index) vertex:u32,
    @location(0) p0:vec3<f32>,@location(1) p1:vec3<f32>,@location(2) p2:vec3<f32>,
    @location(3) normal:vec3<f32>,@location(4) color:vec3<f32>,@location(5) profile_index:u32)->VertexOut {
    let positions=array<vec3<f32>,3>(p0,p1,p2);
    return vertex_out(positions[vertex],color,normal,profile_index);
}
// Same pooled storage page as face_profiles; scalar addressing preserves the
// original 12-byte f32 position triples without vec3 storage padding.
@group(0) @binding(2) var<storage,read> proxy_positions:array<f32>;
@vertex fn vs_proxy_indexed(@builtin(vertex_index) vertex:u32,
    @location(0) addresses:vec3<u32>, @location(1) normal:vec3<f32>,
    @location(2) color:vec3<f32>, @location(3) profile_index:u32)->VertexOut {
    let address=addresses[vertex];
    let position=vec3<f32>(proxy_positions[address],proxy_positions[address+1u],proxy_positions[address+2u]);
    return vertex_out(position,color,normal,profile_index);
}
// Mode2 derives only flat normals. Canonical positions/material colors and
// ordered geometry remain exact; CPU/GPU normalize rounding may differ.
fn proxy_position(address:u32)->vec3<f32> {
    return vec3<f32>(proxy_positions[address],proxy_positions[address+1u],proxy_positions[address+2u]);
}
@vertex fn vs_proxy_derived(@builtin(vertex_index) vertex:u32,
    @location(0) addresses:vec3<u32>, @location(1) material:u32,
    @location(2) profile_index:u32)->VertexOut {
    let a=proxy_position(addresses.x); let b=proxy_position(addresses.y); let c=proxy_position(addresses.z);
    let positions=array<vec3<f32>,3>(a,b,c);
    return vertex_out(positions[vertex],MATERIAL_COLORS[material],normalize(cross(b-a,c-a)),profile_index);
}
const MATERIAL_COLORS:array<vec3<f32>,5> = array<vec3<f32>,5>(
    vec3<f32>(0.,0.,0.),vec3<f32>(0.18,0.34,0.10),vec3<f32>(0.32,0.22,0.13),
    vec3<f32>(0.35,0.37,0.39),vec3<f32>(0.41,0.25,0.13));
@vertex fn vs_native(@builtin(vertex_index) vertex:u32,
    @location(0) anchor:vec3<f32>,@location(1) extent:vec2<f32>,
    @location(2) axis_sign:u32,@location(3) material:u32,@location(4) profile_index:u32)->VertexOut {
    let axis=axis_sign&3u;let negative=(axis_sign&4u)!=0u;
    var corner=vertex;
    if negative && (corner==1u || corner==3u) {corner=4u-corner;}
    var position=anchor;
    position[(axis+1u)%3u]+=extent.x*select(0.,1.,corner==1u || corner==2u);
    position[(axis+2u)%3u]+=extent.y*select(0.,1.,corner>=2u);
    var normal=vec3<f32>(0.);normal[axis]=select(1.,-1.,negative);
    return vertex_out(position,MATERIAL_COLORS[min(material,4u)],normal,profile_index);
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Identical material/light transfer to terrain, per fragment so the collar's
    // interpolated endpoint normals share the terrain page's exact lighting.
    let visibility = terrain_shadow(in.local);
    let light = terrain_lighting(in.normal,visibility);
    var tint=vec3<f32>(1.);
    let local_cell=vec3<i32>(floor(in.local*16.-in.normal*0.01));
    if camera.eye.w>0. {
        let cell=bitcast<vec3<u32>>(local_cell)+camera.paint_origin.xyz;
        let distance=length((vec3<f32>(local_cell)+0.5)/16.-camera.eye.xyz);
        tint=terrain_tint(cell,1.-smoothstep(12.,40.,distance));
    }
    // Evaluate footprint before branching. The smaller singular value retains
    // exact native face response while either tangent direction still resolves
    // a voxel. Filtering changes radiance only, never normal, depth or coverage.
    let dx=dpdx(in.local); let dy=dpdy(in.local);
    let xx=dot(dx,dx); let yy=dot(dy,dy); let xy=dot(dx,dy);
    let largest=0.5*(xx+yy+sqrt(max((xx-yy)*(xx-yy)+4.*xy*xy,0.)));
    let cross_area=cross(dx,dy);
    let smallest=dot(cross_area,cross_area)/max(largest,0.00000001);
    let footprint=sqrt(smallest)/0.0625;
    var blend=select(0.,1.,(camera.paint_origin.w&1u)!=0u);
    if (camera.paint_origin.w&2u)!=0u {blend*=smoothstep(1.,2.,footprint);}
    let resolved=in.color*tint*light;
    if blend<=0. {return vec4<f32>(resolved,1.);}
    // All three triangle vertices share this ID. Fetch only for unresolved
    // radiance, rather than exporting80bytes of repeated vertex varyings.
    let profile=face_profiles[in.profile_index];
    // Source-derived canonical exposed face/material integrals. Self-occlusion
    // within the patch remains approximate; this is not a visibility oracle.
    let v=normalize(camera.eye.xyz-in.local);
    let w=vec4<f32>(max(v.x,0.),max(-v.x,0.),max(v.z,0.),max(-v.z,0.));
    let top=max(v.y,0.);
    let area=top*profile.top.w+dot(w,vec4<f32>(profile.px.w,profile.nx.w,profile.pz.w,profile.nz.w));
    if area>0.000001 && blend>0. {
        let filtered=top*profile.top.rgb*terrain_lighting(vec3<f32>(0.,1.,0.),visibility)
            +w.x*profile.px.rgb*terrain_lighting(vec3<f32>(1.,0.,0.),visibility)
            +w.y*profile.nx.rgb*terrain_lighting(vec3<f32>(-1.,0.,0.),visibility)
            +w.z*profile.pz.rgb*terrain_lighting(vec3<f32>(0.,0.,1.),visibility)
            +w.w*profile.nz.rgb*terrain_lighting(vec3<f32>(0.,0.,-1.),visibility);
        let cell=bitcast<vec3<u32>>(local_cell)+camera.paint_origin.xyz;
        return vec4<f32>(mix(resolved,filtered/area*terrain_tint(cell,0.),blend),1.);
    }
    return vec4<f32>(in.color * tint * light, 1.0);
}
