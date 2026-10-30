struct Camera { vp:mat4x4<f32>, selection:vec4<f32>, viewport:vec4<f32>, inverse:mat4x4<f32>, eye:vec4<f32>, effects:vec4<f32>, grid_eye:vec4<i32>, eye_fraction:vec4<f32> }
// Origins stay on the integer voxel grid. Only the shared camera uniform moves.
struct Tile { origin_step:vec4<f32>, clip_cells:vec4<f32>, arena_offsets:vec4<u32> }
struct Quad { origin:vec3<i32>, face:u32, size:vec2<u32>, color_offset:u32, padding:u32 }
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var horizon_visibility:texture_2d<f32>;
@group(1) @binding(0) var<uniform> tile:Tile;
@group(1) @binding(1) var<storage,read> quads:array<Quad>;
@group(1) @binding(2) var<storage,read> colors:array<u32>;
@group(1) @binding(3) var<storage,read> border_vertices:array<u32>;
struct BorderVertex {record:u32,u:u32,v:u32}
struct Output {
    @builtin(position) position:vec4<f32>,
    @location(0) uv:vec2<f32>,
    @location(1) @interpolate(flat) size:vec2<u32>,
    @location(2) @interpolate(flat) offset:u32,
    @location(3) normal:vec3<f32>,
    @location(4) relative:vec3<f32>,
    @location(5) @interpolate(flat) step:f32,
    @location(6) @interpolate(flat) flags:u32,
    @location(7) @interpolate(flat) water:f32,
    @location(8) @interpolate(flat) phase:f32,
    @location(9) smooth_rgb:vec3<f32>,
}
fn normal(face:u32)->vec3<f32> {
    switch face {case 0u:{return vec3<f32>(0,1,0);}case 1u:{return vec3<f32>(0,-1,0);}case 2u:{return vec3<f32>(-1,0,0);}case 3u:{return vec3<f32>(1,0,0);}case 4u:{return vec3<f32>(0,0,-1);}default:{return vec3<f32>(0,0,1);}}
}
fn face_offset(face:u32,uv:vec2<f32>)->vec3<f32> {
    switch face {case 0u:{return vec3<f32>(uv.y,0,uv.x);}case 1u:{return vec3<f32>(uv.x,0,uv.y);}case 2u:{return vec3<f32>(0,uv.y,uv.x);}case 3u:{return vec3<f32>(0,uv.x,uv.y);}case 4u:{return vec3<f32>(uv.y,uv.x,0);}default:{return vec3<f32>(uv.x,uv.y,0);}}
}
@vertex fn vs_main(@builtin(vertex_index) vertex:u32,@builtin(instance_index) instance:u32)->Output {
    let corners=array<vec2<f32>,6>(vec2<f32>(0,0),vec2<f32>(1,0),vec2<f32>(1,1),vec2<f32>(0,0),vec2<f32>(1,1),vec2<f32>(0,1));
    let q=quads[tile.arena_offsets.x+instance];return project_quad(q,corners[vertex]*vec2<f32>(q.size));
}
@vertex fn vs_conforming(@builtin(vertex_index) vertex:u32)->Output {
    var reference:BorderVertex;
    let base_offset=tile.arena_offsets.z;
    if border_vertices[base_offset]==1u {
        let a=border_vertices[base_offset+1u+vertex*2u];let b=border_vertices[base_offset+2u+vertex*2u];
        reference=BorderVertex(a>>16u,(a&65535u)|((b&255u)<<16u),b>>8u);
    } else {
        let base=base_offset+1u+vertex*3u;reference=BorderVertex(border_vertices[base],border_vertices[base+1u],border_vertices[base+2u]);
    }
    let q=quads[tile.arena_offsets.x+reference.record];
    let uv=vec2<f32>(f32(reference.u),f32(reference.v))/f32(1u<<(q.padding&31u));
    return project_quad(q,uv);
}
fn project_quad(q:Quad,uv:vec2<f32>)->Output {
    // The finite world edge can cut through a coarse cell. Clamp that cell's
    // rectangle at the exact integer world boundary, never past known terrain.
    let is_smooth=(q.face&7u)==6u;
    let face=select(q.face&7u,0u,is_smooth);
    var local=min(vec3<f32>(q.origin)+face_offset(face,uv)*tile.origin_step.w,tile.clip_cells.xyz);
    var surface_normal=normal(face);
    var smooth_rgb=vec3<f32>(-1.);
    if is_smooth {
        var h=vec4<f32>(bitcast<f32>(color_at(q.color_offset+1u)),bitcast<f32>(color_at(q.color_offset+2u)),bitcast<f32>(color_at(q.color_offset+3u)),bitcast<f32>(color_at(q.color_offset+4u)));
        let parent=vec4<f32>(bitcast<f32>(color_at(q.color_offset+5u)),bitcast<f32>(color_at(q.color_offset+6u)),bitcast<f32>(color_at(q.color_offset+7u)),bitcast<f32>(color_at(q.color_offset+8u)));
        let a=linear_rgb(color_at(q.color_offset+9u));
        let b=linear_rgb(color_at(q.color_offset+10u));
        let c=linear_rgb(color_at(q.color_offset+11u));
        let d=linear_rgb(color_at(q.color_offset+12u));
        smooth_rgb=mix(mix(a,b,uv.x),mix(d,c,uv.x),uv.y);
        // Disable parent morph at the patch perimeter to retain exact joins.
        let extent=tile.origin_step.w*32.;
        let border=min(min(local.x,local.z),min(extent-local.x,extent-local.z));
        if tile.arena_offsets.w==1u && tile.clip_cells.w>=1. && tile.clip_cells.w<2. && border>tile.origin_step.w {
            h=mix(parent,h,smoothstep(0.,1.,tile.clip_cells.w-1.));
        }
        local.y=mix(mix(h.x,h.y,uv.x),mix(h.w,h.z,uv.x),uv.y);
        let dx=mix(h.w-h.x,h.z-h.y,uv.x)/tile.origin_step.w;
        let dz=mix(h.y-h.x,h.z-h.w,uv.y)/tile.origin_step.w;
        surface_normal=normalize(vec3<f32>(-dx,1.,-dz));
    }
    if (q.face&7u)==7u {
        let t=uv/vec2<f32>(q.size);
        let side=(q.size.y-1u)&3u;
        let edge_u=select(t.x,1.-t.x,q.size.y>4u);
        let start=bitcast<f32>(color_at(q.color_offset+5u));
        let end=bitcast<f32>(color_at(q.color_offset+6u));
        let along=mix(start,end,edge_u)*f32(q.size.x);
        local=vec3<f32>(q.origin);
        if side<2u {local.z+=along;} else {local.x+=along;}
        let native=mix(bitcast<f32>(color_at(q.color_offset+1u)),bitcast<f32>(color_at(q.color_offset+2u)),edge_u);
        let proxy=mix(bitcast<f32>(color_at(q.color_offset+3u)),bitcast<f32>(color_at(q.color_offset+4u)),edge_u);
        local.y=mix(native,proxy,t.y);
        surface_normal=normal(side+2u);
    }
    let water=q.face>>3u;
    if water>0u && !is_smooth {local.y=min(local.y,f32(water));}
    let relative=vec3<f32>(vec3<i32>(tile.origin_step.xyz)-camera.grid_eye.xyz)*camera.viewport.w+local*camera.viewport.w-camera.eye_fraction.xyz;
    var out:Output;out.position=camera.vp*vec4<f32>(relative,1);out.uv=uv;out.size=q.size;out.offset=q.color_offset;out.normal=surface_normal;out.relative=relative;out.step=tile.origin_step.w;out.flags=q.padding;out.water=f32(water);out.phase=tile.clip_cells.w;out.smooth_rgb=smooth_rgb;return out;
}
fn linear_rgb(packed:u32)->vec3<f32> {
    return pow(vec3<f32>(f32(packed&255u),f32((packed>>8u)&255u),f32((packed>>16u)&255u))/255.,vec3<f32>(2.2));
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
    let uv=clamp(world.xz/camera.eye.w*vec2<f32>(size)-0.5,vec2<f32>(0.),vec2<f32>(size)-1.);
    let a=vec2<i32>(floor(uv));let b=min(a+vec2<i32>(1),size-vec2<i32>(1));let t=fract(uv);
    return mix(mix(textureLoad(horizon_visibility,a,0).r,textureLoad(horizon_visibility,vec2<i32>(b.x,a.y),0).r,t.x),
        mix(textureLoad(horizon_visibility,vec2<i32>(a.x,b.y),0).r,textureLoad(horizon_visibility,b,0).r,t.x),t.y);
}
fn color_at(index:u32)->u32 {
    let base=tile.arena_offsets.y;
    let mode=colors[base];
    if mode==1u {return colors[base+4u];}
    if mode==2u {let packed=colors[base+4u+colors[base+1u]+index/4u];let id=(packed>>((index%4u)*8u))&255u;return colors[base+4u+id];}
    return colors[base+4u+index];
}
@fragment fn fs_main(in:Output)->@location(0) vec4<f32> {
    // Complementary local coverage: every pixel belongs to exactly one
    // representation; unchanged regions never participate in a transition.
    if in.phase>=1. {
        let p=vec2<u32>(in.position.xy)&vec2<u32>(3u);
        let bayer=array<u32,16>(0u,8u,2u,10u,12u,4u,14u,6u,3u,11u,1u,9u,15u,7u,13u,5u);
        let threshold=(f32(bayer[p.y*4u+p.x])+0.5)/16.;
        if in.phase<2. {if threshold>=in.phase-1. {discard;}}
        else {if threshold<in.phase-2. {discard;}}
    }
    if camera.effects.x < -1.5 {return vec4<f32>(vec3<f32>(select(0.,1.,in.water>0.)),1.);}
    if camera.effects.x<0. {return vec4<f32>(1.);}
    let cell=min(vec2<u32>(max(floor(in.uv),vec2<f32>(0))),in.size-vec2<u32>(1u));
    let packed=color_at(in.offset+select(cell.y*in.size.x+cell.x,0u,(in.flags&16384u)!=0u));
    let rgb=vec3<f32>(f32(packed&255u),f32((packed>>8u)&255u),f32((packed>>16u)&255u))/255.;
    var sunlight=max(0.,dot(in.normal,normalize(vec3<f32>(-0.4,0.85,-0.35))));
    // Input palettes are sRGB. Lighting is computed in linear color; the target
    // attachment converts back to sRGB. No sub-cell color noise or grid lines.
    let world_cell=camera.viewport.w*in.step;
    var world_center=(floor((in.relative+camera.eye.xyz)/world_cell-in.normal*0.001)+vec3<f32>(0.5))*world_cell;
    if in.water>0. {world_center.y=in.water*camera.viewport.w;}
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
    let base_linear=select(pow(rgb,vec3<f32>(2.2)),in.smooth_rgb,in.smooth_rgb.x>=0.);
    var lit=base_linear*(sky_light*ambient+0.58*sunlight*visibility);
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
