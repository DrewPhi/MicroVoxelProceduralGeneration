// Greenfield analytical exponential-height fog and a single world-anchored cloud
// sheet. Fog is independent of geometry availability. Distances are metres.
struct AtmosphereUniform {
    inverse_view_projection: mat4x4<f32>,
    relative_eye: vec4<f32>,
    world_eye: vec4<f32>,
    sun: vec4<f32>,
    fog: vec4<f32>, // density, height falloff, exposure, enabled
    weather: vec4<f32>, // coverage, speed, overcast, elapsed seconds
};
@group(0) @binding(0) var<uniform> atmosphere: AtmosphereUniform;
// SCENE_BINDINGS

struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var p = array<vec2<f32>, 3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    var out: VertexOutput;
    out.position = vec4(p[index],0.0,1.0);
    out.uv = vec2(p[index].x * 0.5 + 0.5, 0.5 - p[index].y * 0.5);
    return out;
}
fn hash(p: vec2<f32>) -> f32 {
    // No screen-space noise: this deterministic field moves only with world wind.
    let q = fract(p * vec2(0.1031,0.11369));
    let h = q + dot(q,q.yx + vec2(19.19));
    return fract((h.x+h.y)*h.x);
}
fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p); let f = fract(p); let u = f*f*(3.0-2.0*f);
    return mix(mix(hash(i),hash(i+vec2(1.0,0.0)),u.x),
        mix(hash(i+vec2(0.0,1.0)),hash(i+vec2(1.0,1.0)),u.x),u.y);
}
fn cloud_field(p: vec2<f32>) -> f32 {
    return noise(p)*0.58 + noise(p*2.03+vec2(13.4,7.8))*0.28 + noise(p*4.11)*0.14;
}
fn clear_sky(ray: vec3<f32>) -> vec3<f32> {
    let elevation = max(ray.y,0.0);
    let sunset = 1.0-smoothstep(0.04,0.45,atmosphere.sun.y);
    let horizon = mix(vec3(0.63,0.77,0.88),vec3(0.94,0.63,0.40),sunset*0.7);
    let zenith = mix(vec3(0.16,0.37,0.66),vec3(0.18,0.25,0.43),sunset);
    var color = mix(horizon,zenith,pow(elevation,0.45));
    let sun_dot = max(dot(ray,atmosphere.sun.xyz),0.0);
    let sun_color = mix(vec3(1.0,0.91,0.72),vec3(1.0,0.48,0.22),sunset);
    color += sun_color * (pow(sun_dot,32.0)*0.13 + smoothstep(0.99988,0.99996,sun_dot)*2.0);
    return mix(color,vec3(0.57,0.62,0.67),atmosphere.weather.z*0.72);
}
fn sky(ray: vec3<f32>) -> vec3<f32> {
    var color = clear_sky(ray);
    let coverage = atmosphere.weather.x;
    // A plane at 1800 m: no volumetric loops, bounded low-frequency detail.
    if ray.y > 0.015 && coverage > 0.001 && atmosphere.world_eye.y < 1800.0 {
        let distance = (1800.0-atmosphere.world_eye.y)/ray.y;
        let wind = vec2(1.0,0.32)*atmosphere.weather.w*atmosphere.weather.y;
        let p = (atmosphere.world_eye.xz+ray.xz*distance)*0.00034+wind;
        let value = cloud_field(p);
        let threshold = mix(0.88,0.22,coverage);
        let density = smoothstep(threshold,threshold+0.17,value);
        let horizon_fade = smoothstep(0.015,0.16,ray.y);
        let cloud = mix(vec3(0.94,0.94,0.91),vec3(0.60,0.64,0.68),atmosphere.weather.z);
        let shade = 0.80 + 0.20*smoothstep(threshold,threshold+0.25,value);
        color = mix(color,cloud*shade,density*horizon_fade*0.87);
    }
    return color;
}
fn reconstruct(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let p = atmosphere.inverse_view_projection * vec4(uv.x*2.0-1.0,1.0-uv.y*2.0,depth,1.0);
    return p.xyz / p.w;
}
fn fog_transmittance(distance: f32, direction_y: f32) -> f32 {
    let falloff = atmosphere.fog.y;
    let start_density = atmosphere.fog.x*exp(clamp(-falloff*atmosphere.world_eye.y,-16.0,16.0));
    let delta = falloff*direction_y*distance;
    // Integral of rho0 exp(-height*k) along the ray. A series avoids
    // cancellation for almost horizontal rays, with the correct zero-k limit.
    var integral = 1.0-delta*0.5+delta*delta/6.0;
    if abs(delta) > 0.01 {
        integral = (1.0-exp(clamp(-delta,-60.0,60.0)))/delta;
    }
    return exp(-min(start_density*distance*integral,80.0));
}
fn composite(color: vec3<f32>, depth: f32, uv: vec2<f32>) -> vec3<f32> {
    if atmosphere.fog.w < 0.5 { return color*atmosphere.fog.z; }
    let position = reconstruct(uv,depth);
    let offset = position-atmosphere.relative_eye.xyz;
    let distance = length(offset);
    let ray = offset/max(distance,0.0001);
    if depth >= 1.0 { return sky(ray)*atmosphere.fog.z; }
    let transmittance = fog_transmittance(distance,ray.y);
    // Aerial perspective uses the distant sky's scattered-light hue.
    return mix(clear_sky(ray),color,transmittance)*atmosphere.fog.z;
}
@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    // SAMPLE_COMPOSITE
}
