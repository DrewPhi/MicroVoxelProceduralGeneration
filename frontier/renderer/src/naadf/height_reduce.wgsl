// Next level covers the exact union of four child XZ tiles.
@group(0) @binding(0) var previous:texture_2d<u32>;
@group(0) @binding(1) var reduced:texture_storage_2d<r32uint,write>;

@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=textureDimensions(reduced)) {return;}
    let p=vec2<i32>(id.xy*2u);
    let a=textureLoad(previous,p,0).x;
    let b=textureLoad(previous,p+vec2(1,0),0).x;
    let c=textureLoad(previous,p+vec2(0,1),0).x;
    let d=textureLoad(previous,p+vec2(1,1),0).x;
    textureStore(reduced,vec2<i32>(id.xy),vec4(max(max(a,b),max(c,d)),0u,0u,0u));
}
