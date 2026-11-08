// Scalar arrays deliberately have four-byte alignment: one 44-byte record per
// greedy quad, with original float pigment and material response unchanged.
struct Quad {
    origin:array<i32,3>, extent:array<u32,2>, face:u32,
    color:array<f32,3>, ao:f32, transmission:f32,
};
@group(2) @binding(0) var<storage,read> quads:array<Quad>;
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
struct Camera { view_projection:mat4x4<f32>, eye:vec4<f32>, paint_origin:vec4<u32> };
@group(0) @binding(0) var<uniform> camera:Camera;
struct NearShadow { projection:mat4x4<f32>, parameters:vec4<f32> };
@group(1) @binding(0) var<uniform> shadow:NearShadow;
fn rotate(p:vec3<f32>, turn:u32)->vec3<f32> {
    switch turn&3u {
        case 1u: { return vec3<f32>(-p.z,p.y,p.x); }
        case 2u: { return vec3<f32>(-p.x,p.y,-p.z); }
        case 3u: { return vec3<f32>(p.z,p.y,-p.x); }
        default: { return p; }
    }
}
struct ShadowOut {
    @builtin(position) position:vec4<f32>,
    @location(0) local:vec3<f32>,
    @location(1) @interpolate(flat) ground:u32,
    @location(2) normal:vec3<f32>,
};
@vertex fn vs_shadow(@builtin(vertex_index) vertex:u32, @location(3) root:vec3<f32>,
    @location(4) turn:u32)->ShadowOut {
    let quad=quads[vertex/6u];
    let native=native_vertex(quad,triangle_corner(quad.face,vertex));
    return shadow_vertex(native.position,native.normal,root,turn);
}
@vertex fn vs_shadow_compact_indexed(@builtin(vertex_index) vertex:u32, @location(3) root:vec3<f32>,
    @location(4) turn:u32)->ShadowOut {
    let native=native_vertex(quads[vertex/4u],vertex%4u);
    return shadow_vertex(native.position,native.normal,root,turn);
}
@vertex fn vs_shadow_expanded(@location(0) position:vec3<f32>, @location(1) normal:vec3<f32>,
    @location(3) root:vec3<f32>, @location(4) turn:u32)->ShadowOut {
    return shadow_vertex(position,normal,root,turn);
}
fn shadow_vertex(position:vec3<f32>,normal:vec3<f32>,root:vec3<f32>,turn:u32)->ShadowOut {
    let pivot=vec3<f32>(0.03125,0.,0.03125);
    let local=root+pivot+rotate(position-pivot,turn);
    var out:ShadowOut;
    out.position=shadow.projection*vec4<f32>(local-camera.eye.xyz,1.);
    out.local=local;
    out.ground=turn&4u;
    out.normal=rotate(normal,turn);
    return out;
}
@fragment fn fs_shadow(i:ShadowOut) {
    if i.ground!=0u {
        // Fade per native voxel rather than per 8m tile. The pattern belongs to
        // canonical cells, so neither camera motion nor floating-origin rebases
        // introduce time noise. Trees retain their opaque depth unchanged.
        let distance=length(i.local.xz-camera.eye.xz);
        let coverage=1.-smoothstep(12.,18.,distance);
        let p=bitcast<vec3<u32>>(vec3<i32>(floor(i.local*16.-i.normal*0.01)))+camera.paint_origin.xyz;
        var h=p.x*1597334677u ^ p.y*3812015801u ^ p.z*2798796415u;
        h=(h^(h>>16u))*2246822519u;
        if f32(h&65535u)/65536.>=coverage {discard;}
    }
}
