// Greenfield analytical exponential-height fog and a single world-anchored cloud
// sheet. Fog is independent of geometry availability. Distances are metres.
struct AtmosphereUniform {
    inverse_view_projection: mat4x4<f32>,
    relative_eye: vec4<f32>,
    world_eye: vec4<f32>,
    sun: vec4<f32>,
    fog: vec4<f32>, // density, height falloff, exposure, enabled
    weather: vec4<f32>, // coverage, speed, overcast, elapsed seconds
    beauty: vec4<f32>, // contract v2: enabled, reserved, reserved, reserved
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
// Beauty profile: greenfield artistic single-scattering approximation. World
// positions and actual depth alone drive fog; no clip distance/LOD inputs exist.
fn alpine_clear_sky(ray: vec3<f32>) -> vec3<f32> {
    let elevation = max(ray.y, 0.0);
    let dusk = 1.0 - smoothstep(0.02, 0.40, atmosphere.sun.y);
    let forward = max(dot(ray, atmosphere.sun.xyz), 0.0);
    let sun_color = mix(vec3(1.0, 0.91, 0.74), vec3(1.0, 0.55, 0.29), dusk);
    let horizon = mix(vec3(0.48, 0.63, 0.76), vec3(0.72, 0.52, 0.40), dusk * 0.65);
    let zenith = mix(vec3(0.065, 0.22, 0.46), vec3(0.08, 0.13, 0.27), dusk);
    var color = mix(horizon, zenith, pow(elevation, 0.42));
    // Broad low-energy Mie lobe plus a correctly small angular solar disc.
    color += sun_color * (pow(forward, 12.0) * 0.055 + pow(forward, 96.0) * 0.17);
    color += sun_color * smoothstep(0.999975, 0.999989, forward) * 3.0;
    return mix(color, vec3(0.48, 0.54, 0.61), atmosphere.weather.z * 0.72);
}
fn alpine_cloud_layer(ray: vec3<f32>, color: vec3<f32>, height: f32, scale: f32,
                      weight: f32, shift: vec2<f32>) -> vec3<f32> {
    let distance = (height - atmosphere.world_eye.y) / ray.y;
    if distance <= 0.0 { return color; }
    let wind = vec2(1.0, 0.32) * atmosphere.weather.w * atmosphere.weather.y;
    let p = (atmosphere.world_eye.xz + ray.xz * distance) * scale + wind + shift;
    let broad = noise(p);
    // Three bounded bands break the bank into small lobes. The fine band shapes
    // the edge, rather than painting a flat white texture over a giant mass.
    let value = broad * 0.55 + noise(p * 2.13 + vec2(11.1, 5.7)) * 0.28
        + noise(p * 5.07 + vec2(3.9, 18.2)) * 0.17;
    let threshold = mix(0.87, 0.35, atmosphere.weather.x);
    let density = smoothstep(threshold, threshold + 0.13, value);
    // A density probe toward the sun approximates lit edges and shaded bodies.
    // This lights the sheet only: it is not a volumetric shadow or sun shaft.
    let toward_sun = noise(p + atmosphere.sun.xz * 0.23);
    let edge_light = clamp((broad - toward_sun) * 4.0 + 0.48, 0.0, 1.0);
    let thickness = smoothstep(threshold + 0.03, threshold + 0.25, value);
    let warm = 1.0 - smoothstep(0.03, 0.45, atmosphere.sun.y);
    let lit = mix(vec3(0.92, 0.95, 0.97), vec3(1.0, 0.83, 0.65), warm * 0.7);
    let shadow = mix(vec3(0.39, 0.48, 0.59), vec3(0.36, 0.40, 0.47), atmosphere.weather.z);
    let cloud = mix(shadow, lit, clamp(0.55 + edge_light * 0.52 - thickness * 0.30, 0.0, 1.0));
    let fade = smoothstep(0.025, 0.18, ray.y);
    let alpha = (1.0 - exp(-density * 2.5)) * fade * weight;
    return mix(color, cloud, alpha);
}
fn alpine_sky(ray: vec3<f32>) -> vec3<f32> {
    var color = alpine_clear_sky(ray);
    if ray.y > 0.025 && atmosphere.weather.x > 0.001 {
        // Far thin bank first, then lower broken cumulus. Both remain anchored
        // in the world during movement; no temporal history or geometry blur.
        color = alpine_cloud_layer(ray, color, 3400.0, 0.00065, 0.22, vec2(17.4, 8.1));
        color = alpine_cloud_layer(ray, color, 1900.0, 0.00135, 0.92, vec2(0.0));
    }
    return color;
}
fn bounded_height_integral(start: f32, end: f32, distance: f32, falloff: f32) -> f32 {
    // Density rho*exp(-k*max(y,0)). Cap LOCAL density at sea-level rho,
    // never optical depth or visibility: very long paths can still become hazy.
    // Split a sea-level crossing analytically; no ray-march or cancellation.
    if falloff < 0.0000001 { return distance; }
    let low = min(start, end);
    let high = max(start, end);
    let span = high - low;
    if span < 0.01 { return distance * exp(-falloff * max((start + end) * 0.5, 0.0)); }
    let below = clamp(-low / span, 0.0, 1.0);
    let lo = max(low, 0.0);
    let hi = max(high, 0.0);
    let delta = falloff * (hi - lo);
    var average = exp(-falloff * lo) * (1.0 - delta * 0.5 + delta * delta / 6.0);
    if delta > 0.01 { average = (exp(-falloff * lo) - exp(-falloff * hi)) / delta; }
    return distance * (below + (1.0 - below) * average);
}
fn beauty_finish(color: vec3<f32>) -> vec3<f32> {
    let exposed = max(color * atmosphere.fog.z, vec3(0.0));
    // Luminance-only shoulder preserves hues and native voxel edge contrast.
    // Midtones stay unchanged; the sun/cloud highlights roll off without clips.
    let luminance = dot(exposed, vec3(0.2126, 0.7152, 0.0722));
    if luminance <= 0.65 { return exposed; }
    let mapped = 0.65 + 0.35 * (1.0 - exp(-(luminance - 0.65) / 0.35));
    return exposed * (mapped / luminance);
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
    if atmosphere.beauty.x > 0.5 {
        if depth >= 1.0 { return beauty_finish(alpine_sky(ray)); }
        let end_height = atmosphere.world_eye.y + offset.y;
        let integrated = bounded_height_integral(atmosphere.world_eye.y, end_height, distance, atmosphere.fog.y);
        // Clean air includes a weak altitude-independent Rayleigh contribution.
        // The user density scales BOTH terms, so zero fog remains exactly clear.
        let optical_depth = atmosphere.fog.x * (integrated * 0.78 + distance * 0.22);
        let transmittance = exp(-optical_depth);
        let haze = alpine_clear_sky(vec3(ray.x, max(ray.y, 0.04), ray.z));
        return beauty_finish(mix(haze, color, transmittance));
    }
    if depth >= 1.0 { return sky(ray)*atmosphere.fog.z; }
    let transmittance = fog_transmittance(distance,ray.y);
    // Aerial perspective uses the distant sky's scattered-light hue.
    return mix(clear_sky(ray),color,transmittance)*atmosphere.fog.z;
}
@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    // SAMPLE_COMPOSITE
}
