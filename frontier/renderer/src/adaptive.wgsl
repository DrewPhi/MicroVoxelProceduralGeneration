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
        var ambient=mix(vec3(0.15,0.14,0.115),vec3(0.29,0.36,0.44),sky);
        var sun_color=vec3(0.83,0.75,0.60);
        if (camera.paint_origin.w&4u)!=0u {
            ambient=mix(vec3(0.12,0.145,0.075),vec3(0.25,0.33,0.43),sky);
            sun_color=vec3(2.0,1.7,1.2);
        }
        let direct=sun_color*max(dot(n,camera.sun.xyz),0.);
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
// Optional grain is a display override, independent of canonical tint parity.
fn display_terrain_tint(cell:vec3<u32>,grain:f32)->vec3<f32> {
    let original=terrain_tint(cell,grain);
    if camera.paint.z<0. {return original;}
    let old_grain=(paint_hash(cell)-0.5)*0.055*grain;
    let brightness=(paint_hash(cell)-0.5)*camera.paint.z*grain;
    let hue=(paint_hash(cell^vec3<u32>(761u,157u,1103u))-0.5)*camera.paint.z*0.15*grain;
    return original+vec3<f32>(brightness-old_grain+hue,brightness-old_grain,brightness-old_grain-hue);
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var shadow_depth: texture_depth_2d;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;

struct NearShadow { projection:mat4x4<f32>, parameters:vec4<f32> };
@group(2) @binding(0) var near_shadow_depth:texture_depth_2d;
@group(2) @binding(1) var near_shadow_sampler:sampler_comparison;
@group(2) @binding(2) var<uniform> near_shadow:NearShadow;
fn plant_shadow(local:vec3<f32>,normal:vec3<f32>,sun:vec3<f32>)->f32 {
    if near_shadow.parameters.x<0.5 { return 1.; }
    // Bounded receiver normal/light bias in metres; avoid the broad normal
    // offsets that detach microvoxel branches from their own shadow.
    let slope=1.-max(dot(normalize(normal),sun),0.);
    let receiver=local-camera.eye.xyz+normal*(0.012+0.018*slope)+sun*0.012;
    let p=(near_shadow.projection*vec4<f32>(receiver,1.)).xyz;
    let uv=p.xy*vec2<f32>(0.5,-0.5)+0.5;
    if any(uv<vec2<f32>(0.002)) || any(uv>vec2<f32>(0.998)) || p.z<=0. || p.z>=1. {return 1.;}
    let texel=1./vec2<f32>(textureDimensions(near_shadow_depth));
    let depth=p.z-0.00003;
    let a=textureSampleCompareLevel(near_shadow_depth,near_shadow_sampler,uv+vec2(-0.5,-0.5)*texel,depth);
    let b=textureSampleCompareLevel(near_shadow_depth,near_shadow_sampler,uv+vec2(0.5,-0.5)*texel,depth);
    let c=textureSampleCompareLevel(near_shadow_depth,near_shadow_sampler,uv+vec2(-0.5,0.5)*texel,depth);
    let d=textureSampleCompareLevel(near_shadow_depth,near_shadow_sampler,uv+vec2(0.5,0.5)*texel,depth);
    let edge=min(min(uv.x,uv.y),min(1.-uv.x,1.-uv.y));
    return mix(1.,(a+b+c+d)*0.25,smoothstep(0.002,0.07,edge));
}
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
fn mix_profile(a:FaceProfile,b:FaceProfile,t:f32)->FaceProfile {
    return FaceProfile(mix(a.top,b.top,t),mix(a.px,b.px,t),mix(a.nx,b.nx,t),mix(a.pz,b.pz,t),mix(a.nz,b.nz,t));
}
// Packed map references are relative to the descriptor, not the pack itself.
fn map_node(id:u32,node:u32)->FaceProfile {
    let record=face_profiles[id+1u+node/16u];
    let lane=node%16u;
    var row=record.px;
    switch lane/4u {
        case 1u: {row=record.nx;}
        case 2u: {row=record.pz;}
        case 3u: {row=record.nz;}
        default: {}
    }
    return face_profiles[u32(i32(id)+i32(row[lane%4u]))];
}
// Evaluate one -3 descriptor with an explicit view vector. Spatial nodes use
// the same fragment view so their directional response agrees at shared edges.
fn conditional_profile(id:u32,view:vec3<f32>)->FaceProfile {
    let p=face_profiles[id];
    let horizontal=length(view.xz);
    var azimuth=0.;
    if horizontal>0.000001 {
        let angle=atan2(view.z,view.x)*0.6366197723675814;
        azimuth=angle-floor(angle/4.)*4.;
    }
    let first=u32(floor(azimuth))%4u;
    let next=(first+1u)%4u;
    let azimuth_fraction=fract(azimuth);
    let elevation=clamp((view.y/max(horizontal,0.000001)-p.px.y)/(p.px.z-p.px.y),0.,1.);
    let base=u32(i32(id)+i32(p.px.x));
    return mix_profile(
        mix_profile(face_profiles[base+first],face_profiles[base+next],azimuth_fraction),
        mix_profile(face_profiles[base+4u+first],face_profiles[base+4u+next],azimuth_fraction),elevation);
}
fn map_sample(id:u32,p:FaceProfile,coordinate:vec2<f32>)->FaceProfile {
    let dimension=u32(p.px.x);
    let uv=clamp(coordinate,vec2(0.),vec2(f32(dimension-1u)));
    let cell=min(vec2<u32>(floor(uv)),vec2<u32>(dimension-2u));
    let fraction=uv-vec2<f32>(cell);
    let node=cell.y*dimension+cell.x;
    return mix_profile(
        mix_profile(map_node(id,node),map_node(id,node+1u),fraction.x),
        mix_profile(map_node(id,node+dimension),map_node(id,node+dimension+1u),fraction.x),fraction.y);
}
fn add_profile(a:FaceProfile,b:FaceProfile)->FaceProfile {
    return FaceProfile(a.top+b.top,a.px+b.px,a.nx+b.nx,a.pz+b.pz,a.nz+b.nz);
}
fn scale_profile(a:FaceProfile,s:f32)->FaceProfile {
    return FaceProfile(a.top*s,a.px*s,a.nx*s,a.pz*s,a.nz*s);
}
fn appearance_profile(id:u32,local:vec3<f32>,dx:vec3<f32>,dy:vec3<f32>)->FaceProfile {
    let p=face_profiles[id];
    if p.top.w>=0. {return p;}
    if p.top.w == -2. {
        let uv=(local.xz-p.top.xy)*p.top.z;
        let a=dx.xz*p.top.z;let b=dy.xz*p.top.z;
        let major=select(b,a,dot(a,a)>=dot(b,b));
        let length_major=length(major);
        if camera.paint.x<1.5 || length_major<0.75 {return map_sample(id,p,uv);}
        // Fixed 2/4 taps only along the long pixel axis, not isotropic blur.
        // Clamp to four map cells to bound horizon work and local map support.
        var axis=major*min(1.,4./max(length_major,0.000001));
        // No halo exists outside this map. Contract both halves symmetrically
        // near its edge, converging to the shared bilinear edge value rather
        // than clamping different half-footprints on adjacent patches.
        let extent=vec2<f32>(p.px.x-1.);
        let center=clamp(uv,vec2(0.),extent);
        let room=max(vec2(0.),min(center,extent-center));
        let support=select(min(vec2(1.),2.*room/max(abs(axis),vec2(0.000001))),vec2(1.),abs(axis)<vec2(0.000001));
        axis*=min(support.x,support.y);
        let taps=u32(camera.paint.x);
        var total=scale_profile(p,0.);
        for(var i=0u;i<4u;i=i+1u) {
            if i>=taps {break;}
            total=add_profile(total,map_sample(id,p,uv+axis*((f32(i)+0.5)/f32(taps)-0.5)));
        }
        return scale_profile(total,1./f32(taps));
    }
    if p.top.w == -3. {
        return conditional_profile(id,camera.eye.xyz-vec3(p.top.x,p.top.z,p.top.y));
    }
    if p.top.w == -5. {
        let uv=clamp((local.xz-p.top.xy)*p.top.z,vec2(0.),vec2(1.));
        let ids=vec4<i32>(i32(id))+vec4<i32>(p.px);
        let view=camera.eye.xyz-local;
        return mix_profile(
            mix_profile(conditional_profile(u32(ids.x),view),conditional_profile(u32(ids.y),view),uv.x),
            mix_profile(conditional_profile(u32(ids.z),view),conditional_profile(u32(ids.w),view),uv.x),uv.y);
    }
    let uv=clamp((local.xz-p.top.xy)*p.top.z,vec2(0.),vec2(1.));
    let ids=vec4<i32>(i32(id))+vec4<i32>(p.px);
    // Interpolate color-times-area and area together; normalize only once after
    // weighting visible directions. Relative addresses survive pool relocation.
    return mix_profile(
        mix_profile(face_profiles[u32(ids.x)],face_profiles[u32(ids.y)],uv.x),
        mix_profile(face_profiles[u32(ids.z)],face_profiles[u32(ids.w)],uv.x),uv.y);
}
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
const MATERIAL_COLORS:array<vec3<f32>,7> = array<vec3<f32>,7>(
    vec3<f32>(0.,0.,0.),vec3<f32>(0.18,0.34,0.10),vec3<f32>(0.32,0.22,0.13),
    vec3<f32>(0.35,0.37,0.39),vec3<f32>(0.41,0.25,0.13),
    vec3<f32>(0.40,0.53,0.25),vec3<f32>(0.93,0.92,0.84));
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
    return vertex_out(position,MATERIAL_COLORS[min(material,6u)],normal,profile_index);
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Identical material/light transfer to terrain, per fragment so the collar's
    // interpolated endpoint normals share the terrain page's exact lighting.
    let visibility = min(terrain_shadow(in.local),plant_shadow(in.local,in.normal,camera.sun.xyz));
    let light = terrain_lighting(in.normal,visibility);
    var tint=vec3<f32>(1.);
    let local_cell=vec3<i32>(floor(in.local*16.-in.normal*0.01));
    if camera.eye.w>0. {
        let cell=bitcast<vec3<u32>>(local_cell)+camera.paint_origin.xyz;
        let distance=length((vec3<f32>(local_cell)+0.5)/16.-camera.eye.xyz);
        tint=display_terrain_tint(cell,1.-smoothstep(12.,40.,distance));
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
    let minified=smoothstep(1.,2.,footprint);
    if (camera.paint_origin.w&2u)!=0u {blend*=minified;}
    else if (u32(camera.paint.w)&31u)<=4u && (u32(camera.paint.w)>>5u)<=7u && camera.sky.w>0.5 {
        // Preserve a native-face component while near proxy geometry still
        // resolves, converging to full area shading when native cells minify.
        let near_weight=select(0.5+0.5*minified,minified,camera.sky.w>1.5);
        blend*=near_weight;
    }
    let diagnostic=u32(camera.sky.z);
    if diagnostic==1u {
        let code=u32(camera.paint.w);
        return vec4<f32>(select(0.,1.,(camera.paint_origin.w&2u)!=0u),f32(code&31u)/16.,f32(code>>5u)/24.,1.);
    }
    if diagnostic==2u {return vec4<f32>(normalize(in.normal)*0.5+0.5,1.);}
    if diagnostic==3u {return vec4<f32>(0.45,0.45,0.45,1.);}
    if diagnostic==4u {return vec4<f32>(tint,1.);}
    if diagnostic==5u {return vec4<f32>(blend,blend,blend,1.);}
    if diagnostic==6u {return vec4<f32>(light,1.);}
    if diagnostic==7u {return vec4<f32>(in.color,1.);}
    let resolved=in.color*tint*light;
    if blend<=0. && diagnostic!=8u {return vec4<f32>(resolved,1.);}
    // All three triangle vertices share this ID. Fetch only for unresolved
    // radiance, rather than exporting80bytes of repeated vertex varyings.
    let profile=appearance_profile(in.profile_index,in.local,dx,dy);
    // Source-derived canonical exposed face/material integrals. Self-occlusion
    // within the patch remains approximate; this is not a visibility oracle.
    let v=normalize(camera.eye.xyz-in.local);
    let w=vec4<f32>(max(v.x,0.),max(-v.x,0.),max(v.z,0.),max(-v.z,0.));
    let top=max(v.y,0.);
    let area=top*profile.top.w+dot(w,vec4<f32>(profile.px.w,profile.nx.w,profile.pz.w,profile.nz.w));
    if area>0.000001 && (blend>0. || diagnostic==8u) {
        let filtered=top*profile.top.rgb*terrain_lighting(vec3<f32>(0.,1.,0.),visibility)
            +w.x*profile.px.rgb*terrain_lighting(vec3<f32>(1.,0.,0.),visibility)
            +w.y*profile.nx.rgb*terrain_lighting(vec3<f32>(-1.,0.,0.),visibility)
            +w.z*profile.pz.rgb*terrain_lighting(vec3<f32>(0.,0.,1.),visibility)
            +w.w*profile.nz.rgb*terrain_lighting(vec3<f32>(0.,0.,-1.),visibility);
        let cell=bitcast<vec3<u32>>(local_cell)+camera.paint_origin.xyz;
        if diagnostic==8u {return vec4<f32>(filtered/area,1.);}
        return vec4<f32>(mix(resolved,filtered/area*display_terrain_tint(cell,0.),blend),1.);
    }
    return vec4<f32>(in.color * tint * light, 1.0);
}
