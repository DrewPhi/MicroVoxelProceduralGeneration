struct Scene {
    view_projection: mat4x4<f32>,
    sun: vec4<f32>,
}
@group(0) @binding(0) var<uniform> scene: Scene;

struct Surface {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) location: vec3<f32>,
    @location(3) @interpolate(flat) water_depth: f32,
}

@vertex fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) water_depth: f32,
) -> Surface {
    return Surface(scene.view_projection * vec4(position, 1.0), color, normal, position, water_depth);
}

@fragment fn fs_main(surface: Surface) -> @location(0) vec4<f32> {
    let normal = normalize(surface.normal);
    let sun = normalize(scene.sun.xyz);
    if surface.water_depth > 0.0 {
        let ray = normalize(surface.location);
        let cosine = max(-dot(ray, normal), 0.0);
        let through = refract(ray, normal, 1.0 / 1.333);
        let path_length = surface.water_depth / max(-dot(through, normal), 0.55);
        let reflected = water_sky(reflect(ray, normal), sun, scene.sun.w);
        let fresnel = water_fresnel(cosine);
        let transmitted = water_transmittance(path_length);
        let mean_transmission = dot(transmitted, vec3(0.2126, 0.7152, 0.0722));
        let glow = water_glint(ray, normal, sun, scene.sun.w) * min(1.0, 3.0 * fresnel);
        let color = reflected * fresnel + water_medium(vec3(0.0), path_length) * (1.0 - fresnel) + glow;
        let opacity = 1.0 - mean_transmission * (1.0 - fresnel);
        return vec4(color, opacity);
    }
    let key = max(dot(normal, sun), 0.0);
    let sky = 0.42 + 0.20 * max(normal.y, 0.0);
    let color = surface.color * (vec3(0.86, 0.94, 1.08) * sky + key * vec3(0.77, 0.68, 0.51));
    return vec4(color, 1.0);
}
