const side: u32 = GRID_SIDEu;
const spacing: f32 = GRID_STEP;
struct Camera { matrix: mat4x4<f32>, eye: vec4<f32>, sun: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage,read> lattice: array<vec4<f32>>;
struct Vertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) relative: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) view_depth: f32,
};
@vertex fn vs(@builtin(vertex_index) i:u32) -> Vertex {
    let x = i % side;
    let z = (i / side) % side;
    let level = i / (side*side);
    let base = level*side*side;
    let step = spacing / f32(4u >> level);
    let xz = (vec2<f32>(f32(x), f32(z)) - f32(side / 2u)) * step;
    let h = lattice[i];
    let p = vec3(xz.x, h.w, xz.y);
    let right = lattice[base+min(x+1u,side-1u)+z*side].w;
    let left = lattice[base+max(x,1u)-1u+z*side].w;
    let north = lattice[base+x+(max(z,1u)-1u)*side].w;
    let south = lattice[base+x+min(z+1u,side-1u)*side].w;
    let normal = normalize(vec3(left-right, 2.*step, north-south));
    let color = h.rgb*(vec3(.24,.31,.40)+vec3(.93,.84,.65)*max(0.,dot(normal,camera.sun.xyz)));
    let clip = camera.matrix*vec4(p,1.);
    return Vertex(clip, p-camera.eye.xyz, color, clip.w);
}
@fragment fn fs(v:Vertex) -> @location(0) vec4<f32> {
    if v.view_depth < camera.eye.w { discard; }
    return vec4(v.color, length(v.relative)*.001);
}
