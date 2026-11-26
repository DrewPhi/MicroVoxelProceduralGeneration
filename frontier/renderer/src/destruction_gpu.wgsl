struct View {
    matrix: mat4x4<f32>,
    sun: vec4<f32>,
}
@group(0) @binding(0) var<uniform> view: View;

struct Vertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
}

@vertex fn vertex(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) x: vec4<f32>,
    @location(4) y: vec4<f32>,
    @location(5) z: vec4<f32>,
    @location(6) t: vec4<f32>,
) -> Vertex {
    let model = mat4x4<f32>(x, y, z, t);
    return Vertex(view.matrix * model * vec4(position, 1.), (model * vec4(normal, 0.)).xyz, color);
}

@fragment fn fragment(p: Vertex) -> @location(0) vec4<f32> {
    let n = normalize(p.normal);
    let direct = max(0., dot(n, view.sun.xyz));
    return vec4(p.color * (0.29 + (0.5 + 0.5 * n.y) * 0.22 + 0.76 * direct), 1.);
}
