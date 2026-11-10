// Scalar arrays deliberately have four-byte alignment: one 44-byte record per
// greedy quad, with original float pigment and material response unchanged.
struct Quad {
    origin:array<i32,3>, extent:array<u32,2>, face:u32,
    color:array<f32,3>, ao:f32, transmission:f32,
};
@group(3) @binding(0) var<storage,read> quads:array<Quad>;
struct NativeVertex { position:vec3<f32>, normal:vec3<f32> };
fn triangle_corner(face:u32, vertex:u32)->u32 {
    var corner=vertex%6u;
    if (face&1u)!=0u {
        switch corner {
            case 0u,3u: {corner=0u;}
            case 1u: {corner=1u;}
            case 2u,4u: {corner=2u;}
            default: {corner=3u;}
        }
    } else {
        switch corner {
            case 0u,3u: {corner=0u;}
            case 1u,5u: {corner=2u;}
            case 2u: {corner=1u;}
            default: {corner=3u;}
        }
    }
    return corner;
}
fn native_vertex(q:Quad, corner:u32)->NativeVertex {
    let axis=q.face/2u;
    let u=(axis+1u)%3u;
    let v=(axis+2u)%3u;
    var p=vec3<i32>(q.origin[0],q.origin[1],q.origin[2]);
    if corner==1u || corner==2u {p[u]+=bitcast<i32>(q.extent[0]);}
    if corner==2u || corner==3u {p[v]+=bitcast<i32>(q.extent[1]);}
    var out:NativeVertex;
    out.position=vec3<f32>(p)*0.0625;
    out.normal=vec3<f32>(0.);
    out.normal[axis]=select(-1.,1.,(q.face&1u)!=0u);
    return out;
}
// Solid native cube surfaces. Colors belong to voxels, with identical base RGB
// on all six faces; the same directional/hemispherical light as native terrain.
struct Camera { view_projection:mat4x4<f32>, eye:vec4<f32>, paint_origin:vec4<u32> };
@group(0) @binding(0) var<uniform> camera:Camera;
struct Lighting { sun:vec4<f32>, sky:vec4<f32> };
@group(1) @binding(0) var<uniform> lighting:Lighting;

struct NearShadow { projection:mat4x4<f32>, parameters:vec4<f32>, naadf_lo:vec4<f32>, naadf_hi:vec4<f32> };
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

struct Out {
    @builtin(position) position:vec4<f32>,
    @location(0) color:vec3<f32>,
    @location(1) normal:vec3<f32>,
    @location(2) @interpolate(flat) fade:f32,
    @location(3) @interpolate(flat) seed:u32,
    @location(4) local:vec3<f32>,
    @location(5) ao:f32,
    @location(6) transmission:f32,
};
fn rotate(p:vec3<f32>, turn:u32)->vec3<f32> {
    switch turn&3u {
        case 1u: { return vec3<f32>(-p.z,p.y,p.x); }
        case 2u: { return vec3<f32>(-p.x,p.y,-p.z); }
        case 3u: { return vec3<f32>(p.z,p.y,-p.x); }
        default: { return p; }
    }
}
@vertex fn vs_main(
    @builtin(vertex_index) vertex:u32,
    @location(3) root:vec3<f32>, @location(4) turn:u32, @location(5) fade:f32, @location(6) seed:u32
)->Out {
    let quad=quads[vertex/6u];
    let native=native_vertex(quad,triangle_corner(quad.face,vertex));
    return vertex_out(native.position,native.normal,vec3<f32>(quad.color[0],quad.color[1],quad.color[2]),root,turn,fade,seed,quad.ao,quad.transmission);
}
@vertex fn vs_compact_indexed(
    @builtin(vertex_index) vertex:u32,
    @location(3) root:vec3<f32>, @location(4) turn:u32, @location(5) fade:f32, @location(6) seed:u32
)->Out {
    let quad=quads[vertex/4u];
    let native=native_vertex(quad,vertex%4u);
    return vertex_out(native.position,native.normal,vec3<f32>(quad.color[0],quad.color[1],quad.color[2]),root,turn,fade,seed,quad.ao,quad.transmission);
}
@vertex fn vs_expanded(
    @location(0) position:vec3<f32>, @location(1) normal:vec3<f32>, @location(2) color:vec3<f32>,
    @location(3) root:vec3<f32>, @location(4) turn:u32, @location(5) fade:f32, @location(6) seed:u32,
    @location(7) ao:f32, @location(8) transmission:f32
)->Out {
    return vertex_out(position,normal,color,root,turn,fade,seed,ao,transmission);
}
fn vertex_out(position:vec3<f32>, normal:vec3<f32>, color:vec3<f32>, root:vec3<f32>, turn:u32,
    fade:f32, seed:u32, ao:f32, transmission:f32)->Out {
    var out:Out;
    // World cell indices rotate about the root cell center, so q1 maps the
    // native cell index (x,z) to (-z,x), including negative-index cells.
    let pivot=vec3<f32>(0.03125,0.,0.03125);
    out.local=root+pivot+rotate(position-pivot,turn);
    out.position=camera.view_projection*vec4<f32>(out.local,1.);
    out.color=color;
    out.normal=rotate(normal,turn);
    out.fade=fade;
    out.seed=seed;
    out.ao=ao;
    out.transmission=transmission;
    return out;
}
fn terrain_lighting(normal:vec3<f32>,visibility:f32,ao:f32)->vec3<f32> {
    let n=normalize(normal);
    if lighting.sun.w>0.5 {
        let sky=0.5+0.5*n.y;
        let overcast=lighting.sky.x;
        let ambient=mix(vec3(0.15,0.14,0.115),vec3(0.29,0.36,0.44),sky);
        let direct=vec3(0.83,0.75,0.60)*max(dot(n,lighting.sun.xyz),0.);
        return ambient*(1.+overcast*0.3)*ao+direct*(1.-0.72*overcast)*visibility;
    }
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.3));
    let sky=0.5+0.5*n.y;
    let ambient=mix(vec3<f32>(0.12,0.105,0.085),vec3<f32>(0.28,0.33,0.40),sky);
    return ambient+vec3<f32>(0.78,0.70,0.58)*max(dot(n,sun),0.);
}
// A bounded thin-leaf approximation, selected by canonical material rather than
// pigment. It changes illumination only: wood remains opaque and voxel geometry,
// silhouettes and depth stay identical. Ambient visibility limits interior glow.
fn beauty_plant_lighting(i:Out,visibility:f32,ao:f32)->vec3<f32> {
    let n=normalize(i.normal);
    let sun=normalize(lighting.sun.xyz);
    let overcast=clamp(lighting.sky.x,0.,1.);
    let sky=0.5+0.5*n.y;
    let ambient=mix(vec3(0.12,0.145,0.075),vec3(0.25,0.33,0.43),sky);
    let sunlight=vec3(2.0,1.7,1.2)*(1.-0.72*overcast);
    let direct=sunlight*max(dot(n,sun),0.)*visibility;
    let view=normalize(camera.eye.xyz-i.local);
    // Forward scattering is strongest when the observer faces the sun through
    // the leaf. A small residual represents leaf thickness below shadow texels;
    // this is artistic single scattering, not a volume transport solution.
    let forward=pow(max(dot(-view,sun),0.),4.);
    let back=max(-dot(n,sun),0.);
    let transmitted=sunlight*vec3(0.88,1.,0.44)*0.50*i.transmission
        *(0.25+0.75*forward)*back*(0.08+0.92*visibility)*ao*ao;
    // Nontransmitting plant material is wood, selected by its explicit material
    // response rather than pigment. Approximate warm ground irradiance reaches
    // vertical/down-facing bark in addition to cool sky fill. It follows solar
    // elevation, overcast and ambient visibility; it is reflected illumination,
    // independent of the directly shadowed sun term, never added emission.
    let wood=1.-step(0.001,i.transmission);
    let ground=0.5-0.5*n.y;
    let bounce=sunlight*vec3(0.45,0.30,0.12)*max(sun.y,0.)*ground*ao*wood;
    return ambient*(1.+overcast*0.3)*ao+direct+transmitted+bounce;
}
@fragment fn fs_main(i:Out)->@location(0) vec4<f32> {
    // Ownership follows the occupied native cell, including faces on box edges.
    if near_shadow.naadf_hi.w>0.5 {
        let point=(floor(i.local*16.-i.normal*0.01)+0.5)/16.-camera.eye.xyz;
        if all(point>=near_shadow.naadf_lo.xyz)&&all(point<near_shadow.naadf_hi.xyz) {discard;}
    }

    // Fixed canonical-cell/instance threshold: no time noise or transparency.
    // Opaque survivors write normal scene depth and receive its atmosphere pass.
    if i.fade<1. {
        let p=bitcast<vec3<u32>>(vec3<i32>(floor(i.local*16.-i.normal*0.01)))+camera.paint_origin.xyz;
        var h=p.x*1597334677u ^ p.y*3812015801u ^ p.z*2798796415u ^ i.seed;
        h=(h^(h>>16u))*2246822519u;
        if f32(h&65535u)/65536.>=i.fade { discard; }
    }
    let ao=mix(1.,i.ao,lighting.sky.y);
    let visibility=plant_shadow(i.local,i.normal,lighting.sun.xyz);
    if lighting.sky.y>0.5 && lighting.sun.w>0.5 {
        return vec4<f32>(i.color*beauty_plant_lighting(i,visibility,ao),1.);
    }
    return vec4<f32>(i.color*terrain_lighting(i.normal,visibility,ao),1.);
}
