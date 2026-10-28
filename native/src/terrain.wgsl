struct Camera { vp:mat4x4<f32>, selection:vec4<f32>, viewport:vec4<f32> }
struct Tile { relative_step:vec4<f32>, clip_cells:vec4<f32> }
struct Quad { origin:vec3<i32>, face:u32, size:vec2<u32>, color_offset:u32, padding:u32 }
@group(0) @binding(0) var<uniform> camera:Camera;
@group(1) @binding(0) var<uniform> tile:Tile;
@group(1) @binding(1) var<storage,read> quads:array<Quad>;
@group(1) @binding(2) var<storage,read> colors:array<u32>;
struct Output {
    @builtin(position) position:vec4<f32>,
    @location(0) uv:vec2<f32>,
    @location(1) @interpolate(flat) size:vec2<u32>,
    @location(2) @interpolate(flat) offset:u32,
    @location(3) @interpolate(flat) normal:vec3<f32>,
    @location(4) relative:vec3<f32>,
}
fn normal(face:u32)->vec3<f32> {
    switch face {case 0u:{return vec3<f32>(0,1,0);}case 1u:{return vec3<f32>(0,-1,0);}case 2u:{return vec3<f32>(-1,0,0);}case 3u:{return vec3<f32>(1,0,0);}case 4u:{return vec3<f32>(0,0,-1);}default:{return vec3<f32>(0,0,1);}}
}
fn face_offset(face:u32,uv:vec2<f32>)->vec3<f32> {
    switch face {case 0u:{return vec3<f32>(uv.y,0,uv.x);}case 1u:{return vec3<f32>(uv.x,0,uv.y);}case 2u:{return vec3<f32>(0,uv.y,uv.x);}case 3u:{return vec3<f32>(0,uv.x,uv.y);}case 4u:{return vec3<f32>(uv.y,uv.x,0);}default:{return vec3<f32>(uv.x,uv.y,0);}}
}
@vertex fn vs_main(@builtin(vertex_index) vertex:u32,@builtin(instance_index) instance:u32)->Output {
    let corners=array<vec2<f32>,6>(vec2<f32>(0,0),vec2<f32>(1,0),vec2<f32>(1,1),vec2<f32>(0,0),vec2<f32>(1,1),vec2<f32>(0,1));
    let q=quads[instance];let uv=corners[vertex]*vec2<f32>(q.size);
    // The finite world edge can cut through a coarse cell. Clamp that cell's
    // rectangle at the exact integer world boundary, never past known terrain.
    let local=min(vec3<f32>(q.origin)+face_offset(q.face,uv)*tile.relative_step.w,tile.clip_cells.xyz);
    let relative=tile.relative_step.xyz+local*0.0625;
    var out:Output;out.position=camera.vp*vec4<f32>(relative,1);out.uv=uv;out.size=q.size;out.offset=q.color_offset;out.normal=normal(q.face);out.relative=relative;return out;
}
@fragment fn fs_main(in:Output)->@location(0) vec4<f32> {
    let cell=min(vec2<u32>(max(floor(in.uv),vec2<f32>(0))),in.size-vec2<u32>(1u));
    let packed=colors[in.offset+cell.y*in.size.x+cell.x];
    let rgb=vec3<f32>(f32(packed&255u),f32((packed>>8u)&255u),f32((packed>>16u)&255u))/255.;
    let sunlight=max(0.,dot(in.normal,normalize(vec3<f32>(-0.4,0.85,-0.35))));
    // Input palettes are sRGB. Lighting is computed in linear color; the target
    // attachment converts back to sRGB. No sub-cell color noise or grid lines.
    var lit=pow(rgb,vec3<f32>(2.2))*(0.52+0.48*sunlight);
    let voxel_center=(floor(in.relative*16.-in.normal*0.001)+vec3<f32>(0.5))*0.0625;
    let fog=1.-exp(-length(voxel_center)*0.00011);
    lit=mix(lit,pow(vec3<f32>(0.69,0.79,0.87),vec3<f32>(2.2)),fog);
    // Selection covers only the voxel under the crosshair. Its twelve edges
    // remain visible even when the surrounding surface is one merged rectangle.
    if camera.selection.w>0. {
        let p=(in.relative-camera.selection.xyz)/0.0625;
        if all(p>=vec3<f32>(-0.002)) && all(p<=vec3<f32>(1.002)) {
            let edge=min(abs(p),abs(vec3<f32>(1)-p));
            if (edge.x<0.025 && edge.y<0.025)||(edge.x<0.025 && edge.z<0.025)||(edge.y<0.025 && edge.z<0.025) {lit=vec3<f32>(0.015);}
        }
    }
    return vec4<f32>(lit,1);
}
// Tiny crosshair drawn directly on the GPU without a UI framework.
@vertex fn vs_hud(@builtin(vertex_index) vertex:u32)->@builtin(position) vec4<f32> {
    let corners=array<vec2<f32>,6>(vec2<f32>(-1,-1),vec2<f32>(1,-1),vec2<f32>(1,1),vec2<f32>(-1,-1),vec2<f32>(1,1),vec2<f32>(-1,1));
    return vec4<f32>(corners[vertex]*vec2<f32>(14./camera.viewport.x,14./camera.viewport.y),0.,1.);
}
@fragment fn fs_hud(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {
    let delta=abs(p.xy-camera.viewport.xy*0.5);
    if (delta.x<0.8 && delta.y<6.) || (delta.y<0.8 && delta.x<6.) {return vec4<f32>(0.95,0.97,0.96,1.);}
    discard;
}
