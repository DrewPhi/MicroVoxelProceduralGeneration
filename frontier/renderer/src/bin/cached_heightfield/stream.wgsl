// Height samples and max bounds for a world-aligned sliding texture window.
struct Job {
    params: vec4u, // seed low/high, window cells, output mip
    base: vec4i,   // origin in world sample indices
    rect: vec4u,   // dirty logical rectangle (x, y, width, height)
    metrics: vec4f,
    lattice: array<vec4i,9>, // unused by generation without a gradient atlas
}
@group(0) @binding(0) var<uniform> view: Job;
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var dest: texture_storage_2d<r32float,write>;
const USE_ATLAS: bool = false;
var<private> gradients: array<u32,1>;

fn height_address(logical: vec2i) -> vec2i {
    let side=i32(view.params.z);
    // The extra row/column holds the true right/top halo, never a wrapped
    // sample from the other edge of the active window.
    return select((logical+view.base.xy)&vec2i(side-1),logical,logical==vec2i(side));
}
fn mip_address(logical: vec2i, level: u32) -> vec2i {
    if level>6u { return logical; }
    let side=i32(view.params.z)>>level;
    return (logical+(view.base.xy>>vec2u(level)))&vec2i(side-1);
}
@compute @workgroup_size(8,8)
fn emit_height(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.rect.zw) { return; }
    let p=vec2i(view.rect.xy+id.xy);
    let world=(vec2f(p+view.base.xy)+view.metrics.y)*view.metrics.x;
    textureStore(dest,height_address(p),vec4f(elevation(world,true),0.,0.,0.));
}
@compute @workgroup_size(8,8)
fn emit_leaf(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.rect.zw) { return; }
    let p=vec2i(view.rect.xy+id.xy);
    let a=textureLoad(previous,height_address(p),0).r;
    let b=textureLoad(previous,height_address(p+vec2i(1,0)),0).r;
    let c=textureLoad(previous,height_address(p+vec2i(0,1)),0).r;
    let d=textureLoad(previous,height_address(p+1),0).r;
    textureStore(dest,mip_address(p,0u),vec4f(max(max(a,b),max(c,d)),0.,0.,0.));
}
@compute @workgroup_size(8,8)
fn emit_parent(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.rect.zw) { return; }
    let p=vec2i(view.rect.xy+id.xy);
    let level=view.params.w;
    let q=p*2;
    let a=textureLoad(previous,mip_address(q,level-1u),0).r;
    let b=textureLoad(previous,mip_address(q+vec2i(1,0),level-1u),0).r;
    let c=textureLoad(previous,mip_address(q+vec2i(0,1),level-1u),0).r;
    let d=textureLoad(previous,mip_address(q+1,level-1u),0).r;
    textureStore(dest,mip_address(p,level),vec4f(max(max(a,b),max(c,d)),0.,0.,0.));
}
