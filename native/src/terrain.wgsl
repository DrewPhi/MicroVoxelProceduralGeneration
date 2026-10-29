struct Camera { vp:mat4x4<f32>, selection:vec4<f32>, viewport:vec4<f32>, inverse:mat4x4<f32>, eye:vec4<f32>, effects:vec4<f32> }
// Origins stay on the integer voxel grid. Only the shared camera uniform moves.
struct Tile { origin_step:vec4<f32>, clip_cells:vec4<f32> }
struct Quad { origin:vec3<i32>, face:u32, size:vec2<u32>, color_offset:u32, padding:u32 }
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var horizon_visibility:texture_2d<f32>;
@group(1) @binding(0) var<uniform> tile:Tile;
@group(1) @binding(1) var<storage,read> quads:array<Quad>;
@group(1) @binding(2) var<storage,read> colors:array<u32>;
@group(1) @binding(3) var<storage,read> border_vertices:array<vec2<u32>>;
struct Output {
    @builtin(position) position:vec4<f32>,
    @location(0) uv:vec2<f32>,
    @location(1) @interpolate(flat) size:vec2<u32>,
    @location(2) @interpolate(flat) offset:u32,
    @location(3) @interpolate(flat) normal:vec3<f32>,
    @location(4) relative:vec3<f32>,
    @location(5) @interpolate(flat) step:f32,
    @location(6) @interpolate(flat) flags:u32,
}
fn normal(face:u32)->vec3<f32> {
    switch face {case 0u:{return vec3<f32>(0,1,0);}case 1u:{return vec3<f32>(0,-1,0);}case 2u:{return vec3<f32>(-1,0,0);}case 3u:{return vec3<f32>(1,0,0);}case 4u:{return vec3<f32>(0,0,-1);}default:{return vec3<f32>(0,0,1);}}
}
fn face_offset(face:u32,uv:vec2<f32>)->vec3<f32> {
    switch face {case 0u:{return vec3<f32>(uv.y,0,uv.x);}case 1u:{return vec3<f32>(uv.x,0,uv.y);}case 2u:{return vec3<f32>(0,uv.y,uv.x);}case 3u:{return vec3<f32>(0,uv.x,uv.y);}case 4u:{return vec3<f32>(uv.y,uv.x,0);}default:{return vec3<f32>(uv.x,uv.y,0);}}
}
@vertex fn vs_main(@builtin(vertex_index) vertex:u32,@builtin(instance_index) instance:u32)->Output {
    let corners=array<vec2<f32>,6>(vec2<f32>(0,0),vec2<f32>(1,0),vec2<f32>(1,1),vec2<f32>(0,0),vec2<f32>(1,1),vec2<f32>(0,1));
    let q=quads[instance];return project_quad(q,corners[vertex]*vec2<f32>(q.size));
}
@vertex fn vs_conforming(@builtin(vertex_index) vertex:u32)->Output {
    let reference=border_vertices[vertex];let q=quads[reference.x];
    let uv=vec2<f32>(f32(reference.y&65535u),f32(reference.y>>16u))/f32(q.padding&32767u);
    return project_quad(q,uv);
}
fn project_quad(q:Quad,uv:vec2<f32>)->Output {
    // The finite world edge can cut through a coarse cell. Clamp that cell's
    // rectangle at the exact integer world boundary, never past known terrain.
    var local=min(vec3<f32>(q.origin)+face_offset(q.face,uv)*tile.origin_step.w,tile.clip_cells.xyz);
    let water=q.padding>>16u;
    if water>0u {local.y=min(local.y,f32(water));}
    let relative=(tile.origin_step.xyz*camera.viewport.w-camera.eye.xyz)+local*camera.viewport.w;
    var out:Output;out.position=camera.vp*vec4<f32>(relative,1);out.uv=uv;out.size=q.size;out.offset=q.color_offset;out.normal=normal(q.face);out.relative=relative;out.step=tile.origin_step.w;out.flags=q.padding;return out;
}
fn sky_color(direction:vec3<f32>)->vec3<f32> {
    let sun=normalize(vec3<f32>(-0.4,0.85,-0.35));
    let elevation=pow(max(direction.y,0.),0.45);
    let horizon=vec3<f32>(0.72,0.82,0.87);
    let zenith=vec3<f32>(0.30,0.53,0.77);
    let glow=pow(max(dot(direction,sun),0.),24.);
    let disk=smoothstep(0.9993,0.9998,dot(direction,sun));
    return pow(mix(horizon,zenith,elevation),vec3<f32>(2.2))+vec3<f32>(0.18,0.13,0.07)*glow+vec3<f32>(1.,0.85,0.62)*disk;
}
fn shadow_at(world:vec3<f32>)->f32 {
    let size=vec2<i32>(textureDimensions(horizon_visibility));
    let uv=clamp(world.xz/10000.*vec2<f32>(size)-0.5,vec2<f32>(0.),vec2<f32>(size)-1.);
    let a=vec2<i32>(floor(uv));let b=min(a+vec2<i32>(1),size-vec2<i32>(1));let t=fract(uv);
    return mix(mix(textureLoad(horizon_visibility,a,0).r,textureLoad(horizon_visibility,vec2<i32>(b.x,a.y),0).r,t.x),
        mix(textureLoad(horizon_visibility,vec2<i32>(a.x,b.y),0).r,textureLoad(horizon_visibility,b,0).r,t.x),t.y);
}
@fragment fn fs_main(in:Output)->@location(0) vec4<f32> {
    let cell=min(vec2<u32>(max(floor(in.uv),vec2<f32>(0))),in.size-vec2<u32>(1u));
    let packed=colors[in.offset+cell.y*in.size.x+cell.x];
    let rgb=vec3<f32>(f32(packed&255u),f32((packed>>8u)&255u),f32((packed>>16u)&255u))/255.;
    var sunlight=max(0.,dot(in.normal,normalize(vec3<f32>(-0.4,0.85,-0.35))));
    // Input palettes are sRGB. Lighting is computed in linear color; the target
    // attachment converts back to sRGB. No sub-cell color noise or grid lines.
    let world_cell=camera.viewport.w*in.step;
    var world_center=(floor((in.relative+camera.eye.xyz)/world_cell-in.normal*0.001)+vec3<f32>(0.5))*world_cell;
    if (in.flags>>16u)>0u {world_center.y=f32(in.flags>>16u)*camera.viewport.w;}
    var visibility=1.;if camera.effects.y>0.5 {visibility=shadow_at(world_center);}
    var ambient=select(1.,f32((packed>>24u)&255u)/255.,camera.effects.x>0.5);
    var sky_light=0.34+0.10*max(in.normal.y,0.);
    if (in.flags&32768u)>0u {
        // A native voxel remains crisp when resolvable. Distant proxy risers
        // progressively share filtered slope lighting; no change to base RGB.
        let native_pixels=camera.viewport.w*camera.viewport.y*camera.effects.z/max(length(in.relative),0.01);
        let blend=1.-smoothstep(0.8,3.0,native_pixels);
        sunlight=mix(sunlight,f32((packed>>24u)&63u)/63.,blend);
        sky_light=mix(sky_light,0.44,blend);
        let near_ao=select(1.,0.7+0.1*f32(packed>>30u),camera.effects.x>0.5);
        ambient=mix(near_ao,1.,blend);
    }
    var lit=pow(rgb,vec3<f32>(2.2))*(sky_light*ambient+0.58*sunlight*visibility);
    let distance=length(in.relative);
    // Clear foreground, increasingly separated distant ridges. This is a
    // static visibility approximation, independent of future weather state.
    let density=camera.viewport.z*1.65;
    let fog=1.-exp(-max(distance-60.,0.)*density);
    let cutoff=smoothstep(camera.effects.w*0.75,camera.effects.w,distance);
    lit=mix(lit,sky_color(normalize(in.relative)),max(fog,cutoff));
    // Selection covers only the voxel under the crosshair. Its twelve edges
    // remain visible even when the surrounding surface is one merged rectangle.
    if camera.selection.w>0. {
        let p=(in.relative-camera.selection.xyz)/camera.viewport.w;
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
