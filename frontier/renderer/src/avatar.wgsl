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
}

@vertex fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) normal: vec3<f32>,
) -> Surface {
    return Surface(scene.view_projection * vec4(position, 1.0), color, normal, position);
}

@fragment fn fs_main(surface: Surface) -> @location(0) vec4<f32> {
    let normal = normalize(surface.normal);
    let sun = normalize(scene.sun.xyz);
    let key = max(dot(normal, sun), 0.0);
    let sky = 0.42 + 0.20 * max(normal.y, 0.0);
    var color = surface.color * (vec3(0.86, 0.94, 1.08) * sky + key * vec3(0.77, 0.68, 0.51));
    if surface.color.b > surface.color.g * 1.3 && surface.color.g > 0.28 {
        let half_angle = normalize(sun - normalize(surface.location));
        let glint = pow(max(dot(normal, half_angle), 0.0), 22.0);
        color += vec3(0.58, 0.75, 0.83) * (glint * 0.53);
    }
    return vec4(color, 1.0);
}
