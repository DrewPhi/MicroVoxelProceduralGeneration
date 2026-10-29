struct Camera { vp:mat4x4<f32>, selection:vec4<f32>, viewport:vec4<f32>, inverse:mat4x4<f32>, eye:vec4<f32>, effects:vec4<f32> }
@group(0) @binding(0) var<uniform> camera:Camera;
fn sky_color(direction:vec3<f32>)->vec3<f32> {
    let sun=normalize(vec3<f32>(-0.4,0.85,-0.35));
    let elevation=pow(max(direction.y,0.),0.45);
    let horizon=vec3<f32>(0.72,0.82,0.87);
    let zenith=vec3<f32>(0.30,0.53,0.77);
    let glow=pow(max(dot(direction,sun),0.),24.);
    let disk=smoothstep(0.9993,0.9998,dot(direction,sun));
    return pow(mix(horizon,zenith,elevation),vec3<f32>(2.2))+vec3<f32>(0.18,0.13,0.07)*glow+vec3<f32>(1.,0.85,0.62)*disk;
}
@vertex fn vs_sky(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    let corners=array<vec2<f32>,6>(vec2<f32>(-1,-1),vec2<f32>(1,-1),vec2<f32>(1,1),vec2<f32>(-1,-1),vec2<f32>(1,1),vec2<f32>(-1,1));
    return vec4<f32>(corners[i],0.999999,1.);
}
@fragment fn fs_sky(@builtin(position) pixel:vec4<f32>)->@location(0) vec4<f32> {
    let ndc=(pixel.xy/camera.viewport.xy*2.-1.)*vec2<f32>(1.,-1.);
    let p=camera.inverse*vec4<f32>(ndc,0.5,1.);
    return vec4<f32>(sky_color(normalize(p.xyz/p.w)),1.);
}
