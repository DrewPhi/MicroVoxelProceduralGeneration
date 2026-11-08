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
@vertex fn vs_shadow(@location(0) position:vec3<f32>, @location(3) root:vec3<f32>,
    @location(4) turn:u32)->@builtin(position) vec4<f32> {
    let pivot=vec3<f32>(0.03125,0.,0.03125);
    let local=root+pivot+rotate(position-pivot,turn);
    return shadow.projection*vec4<f32>(local-camera.eye.xyz,1.);
}
