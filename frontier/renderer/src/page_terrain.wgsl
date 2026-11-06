struct Camera { matrix:mat4x4<f32>, eye:vec4<f32>, near:vec4<f32>, volume:vec4<f32>, paint_origin:vec4<u32>, settings:vec4<f32> };
struct Sample { height:vec4<f32>, normal:vec4<f32>, color:vec4<f32> };
struct Instance { offset:vec2<f32>, slot:u32, pad:u32 };
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var<storage,read> pages:array<Sample>;
@group(0) @binding(2) var<storage,read> instances:array<Instance>;
struct Out { @builtin(position) position:vec4<f32>, @location(0) normal:vec3<f32>, @location(1) color:vec3<f32>, @location(2) world:vec3<f32> };
@vertex fn vs_main(@builtin(vertex_index) vertex:u32,@builtin(instance_index) instance:u32)->Out {
    let page=instances[instance]; let s=pages[page.slot*1089u+vertex];
    let ground=page.offset+vec2<f32>(f32(vertex%33u),f32(vertex/33u))*camera.settings.x;
    var out:Out;out.world=vec3<f32>(ground.x,s.height.x,ground.y);
    out.position=camera.matrix*vec4<f32>(out.world,1.);out.normal=s.normal.xyz;out.color=s.color.xyz;return out;
}
@fragment fn fs_main(i:Out)->@location(0) vec4<f32> {
    if max(abs(i.world.x-camera.near.x),abs(i.world.z-camera.near.y))<camera.near.z { discard; }
    if camera.volume.w>0. && max(abs(i.world.x-camera.volume.x),abs(i.world.z-camera.volume.y))<=camera.volume.z { discard; }
    let view=normalize(camera.eye.xyz-i.world);
    let exact=i.color*terrain_lighting(i.normal);
    let appearance=voxel_appearance(i.color,i.normal,view,distance(camera.eye.xyz,i.world));
    let cell=bitcast<vec3<u32>>(vec3<i32>(floor(i.world*16.)))+camera.paint_origin.xyz;
    return vec4<f32>(terrain_tint(cell,0.)*mix(exact,appearance,camera.eye.w),1.);
}
