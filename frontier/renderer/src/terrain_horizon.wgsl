// World-aligned coarse horizon. Refresh when the outer height window moves
// or sun azimuth changes. Elevation remains a continuous receiver query.
struct Params {
    grid: vec4f,
    sun: vec4f,
    cache: vec4i,
}
@group(0) @binding(0) var<uniform> terrain: Params;
@group(0) @binding(1) var heights: texture_2d<f32>;
@group(0) @binding(2) var field: texture_storage_2d<rgba16float,write>;

fn height_sample(pos:vec2i)->f32 {
    var cell=pos;
    if terrain.cache.w==2 {
        let side=i32(terrain.grid.w);
        cell=select((pos+terrain.cache.xy)&vec2i(side-1),pos,pos==vec2i(side));
    }
    return textureLoad(heights,cell,0).x;
}

fn height(p: vec2f) -> f32 {
    let position = clamp(p, vec2f(0.), vec2f(terrain.grid.w-.0001));
    let base = vec2i(floor(position));
    let f = fract(position);
    let a = height_sample(base);
    let b = height_sample(base+vec2i(1,0));
    let c = height_sample(base+vec2i(0,1));
    let d = height_sample(base+1);
    return mix(mix(a,b,f.x),mix(c,d,f.x),f.y);
}

@compute @workgroup_size(8,8)
fn build(@builtin(global_invocation_id) i: vec3u) {
    let texel = terrain.grid.w / f32(textureDimensions(field).x);
    let p = (vec2f(i.xy) + .5) * texel;
    let ground = height(p);
    let distances = array<f32, 14>(32., 56., 90., 140., 215., 320., 480., 710., 1050., 1510., 2170., 3000., 4000., 5200.);
    var angle = 0.;
    var blocker = 4800.;
    for (var k=0u; k<14u; k+=1u) {
        let d = distances[k];
        let q = p + terrain.sun.xy * (d*terrain.grid.z);
        if any(q < vec2f(0.)) || any(q > vec2f(terrain.grid.w)) { break; }
        let candidate = (height(q) - ground) / d;
        if candidate > angle {
            angle = candidate;
            blocker = d;
        }
    }
    let range = 154.;
    let delta = range * terrain.grid.z;
    let a = max(0., height(p + vec2f(delta,0.)) - ground);
    let b = max(0., height(p - vec2f(delta,0.)) - ground);
    let c = max(0., height(p + vec2f(0.,delta)) - ground);
    let d = max(0., height(p - vec2f(0.,delta)) - ground);
    let blocked = min(.55, (a+b+c+d)/(2.6*range));
    // A little low-order diffuse ground return fills sheltered terrain where
    // the upper hemisphere is reduced by the surrounding slopes.
    let openness = 1. - .77*blocked;
    textureStore(field,vec2i(i.xy),vec4f(angle,openness,log2(blocker),ground));
}
