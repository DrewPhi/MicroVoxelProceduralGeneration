struct Camera { vp:mat4x4<f32>, selection:vec4<f32>, viewport:vec4<f32> }
@group(0) @binding(0) var<uniform> camera:Camera;
@group(1) @binding(0) var environment_map:texture_2d<f32>;
@group(1) @binding(1) var nearest_sampler:sampler;
struct Output { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> }
@vertex fn vs_map(@builtin(vertex_index) vertex:u32)->Output {
    let corners=array<vec2<f32>,6>(vec2<f32>(-1,-1),vec2<f32>(1,-1),vec2<f32>(1,1),vec2<f32>(-1,-1),vec2<f32>(1,1),vec2<f32>(-1,1));
    let p=corners[vertex];let scale=min(camera.viewport.x,camera.viewport.y)/camera.viewport.xy;
    var out:Output;out.position=vec4<f32>(p*scale,0,1);out.uv=vec2<f32>(p.x*0.5+0.5,0.5-p.y*0.5);return out;
}
@fragment fn fs_map(in:Output)->@location(0) vec4<f32> {return textureSample(environment_map,nearest_sampler,in.uv);}
