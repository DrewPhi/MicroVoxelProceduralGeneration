const unit: f32 = GRID_UNIT;
struct Camera { matrix: mat4x4<f32>, eye: vec4<f32>, sun: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
struct Vertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) relative: vec3<f32>,
    @location(1) @interpolate(flat) color: vec3<f32>,
    @location(2) view_depth: f32,
};
@vertex fn vs(@builtin(vertex_index) i:u32, @location(0) packed:vec2<u32>, @location(1) heights:vec2<f32>) -> Vertex {
    let x = f32(bitcast<i32>(packed.x<<21u)>>21u)*unit;
    let z = f32(bitcast<i32>(packed.x<<10u)>>21u)*unit;
    let axis = (packed.y>>24u)&7u;
    let size = unit*f32(1u<<(packed.y>>27u));
    let u = f32(i&1u);
    let v = f32(i>>1u);
    var p = vec3(x+v*size,heights.y,z+u*size);
    var normal = vec3(0.,1.,0.);
    if axis==1u || axis==2u {
        p = vec3(x,mix(heights.x,heights.y,v),z+select(u,1.-u,axis==1u)*size);
        normal = vec3(select(-1.,1.,axis==1u),0.,0.);
    } else if axis>=3u {
        p = vec3(x+select(u,1.-u,axis==4u)*size,mix(heights.x,heights.y,v),z);
        normal = vec3(0.,0.,select(-1.,1.,axis==3u));
    }
    let albedo=vec3<f32>(vec3<u32>(packed.y,packed.y>>8u,packed.y>>16u)&vec3(255u))/255.;
    // Distant steps occupy a fraction of a pixel in height. Integrate the
    // surrounding slope into their illumination while keeping each face flat.
    let slope=vec2<f32>(f32(bitcast<i32>(packed.x<<5u)>>27u),f32(bitcast<i32>(packed.x)>>27u))/15.;
    let relief=normalize(vec3(slope.x,max(0.,1.-abs(slope.x)-abs(slope.y)),slope.y));
    let center=vec3(x+.5*size,heights.y,z+.5*size);
    let footprint=size/max(1.,distance(center,camera.eye.xyz));
    let broad=mix(.72,.30,smoothstep(.008,.040,footprint));
    let direct=mix(max(0.,dot(normal,camera.sun.xyz)),max(0.,dot(relief,camera.sun.xyz)),broad);
    let color=albedo*(vec3(.24,.31,.40)+vec3(.93,.84,.65)*direct);
    let clip=camera.matrix*vec4(p,1.);
    return Vertex(clip,p-camera.eye.xyz,color,clip.w);
}
@fragment fn fs(v:Vertex) -> @location(0) vec4<f32> {
    if v.view_depth < camera.eye.w { discard; }
    return vec4(v.color,length(v.relative)*.001);
}
