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
@vertex fn vs_shadow(@location(0) position:vec3<f32>, @location(1) normal:vec3<f32>, @location(3) root:vec3<f32>,
    @location(4) turn:u32)->ShadowOut {
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
