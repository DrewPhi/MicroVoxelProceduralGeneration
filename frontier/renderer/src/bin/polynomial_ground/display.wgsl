struct View {
    eye: vec4f,
    forward: vec4f,
    right: vec4f,
    up: vec4f,
    size: vec4u,
    origin: vec4i,
}
struct Vertex {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage,read> pixels: array<vec2u>;
override SRGB: bool = true;

@vertex
fn vertex(@builtin(vertex_index) i: u32) -> Vertex {
    let uv=vec2f(f32((i<<1u)&2u), f32(i&2u));
    return Vertex(vec4f(uv*vec2f(2.,-2.)+vec2f(-1.,1.),0.,1.),uv);
}

@fragment
fn fragment(input: Vertex) -> @location(0) vec4f {
    let p=min(vec2u(input.uv*vec2f(view.size.xy)),view.size.xy-1u);
    let m=pixels[p.y*view.size.x+p.x].y;
    let rgb=vec3f(vec3u(m&255u,(m>>8u)&255u,(m>>16u)&255u))/255.;
    if SRGB {return vec4f(pow(rgb,vec3f(2.2)),1.);}
    return vec4f(rgb,1.);
}
