// The Alpine source from world/src/alpine.rs. Bounds use the same expression
// and the same seeded gradients. Every terminal intersection uses native roofs.
struct View {
    projection: mat4x4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    sun: vec4<f32>,
    settings: vec4<f32>,
    tile: vec4<f32>,
    grid: vec2<i32>,
    seed: vec2<u32>,
    flags: vec4<u32>,
    lattice: array<vec4<i32>, 9>,
    output: vec4<u32>,
    cache: vec4<u32>,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read_write> build_bounds: array<f32>;
@group(0) @binding(2) var<storage, read> bounds: array<f32>;
@group(0) @binding(3) var<storage, read_write> build_lattice: array<u32>;
@group(0) @binding(4) var<storage, read> lattice: array<u32>;
@group(0) @binding(5) var<storage, read_write> fine: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write> build_beams: array<f32>;
@group(0) @binding(7) var<storage, read> beams: array<f32>;
@group(0) @binding(8) var<storage, read_write> build_hits: array<vec4<u32>>;
@group(0) @binding(9) var<storage, read> swept_hits: array<vec4<u32>>;

override FINE_MODE: u32 = 0u;
const SIDE: u32 = 512u;
const LEVEL1: u32 = SIDE*SIDE;
const LEVEL2: u32 = LEVEL1 + 128u*128u;
const BLOCK: f32 = 32.0;
const CELL: f32 = 0.0625;
const INF: f32 = 1e25;

fn high_product(a: u32, b: u32) -> u32 {
    let a0 = a & 65535u;
    let a1 = a >> 16u;
    let b0 = b & 65535u;
    let b1 = b >> 16u;
    let u = a0*b0;
    let t = a1*b0 + (u >> 16u);
    let v = a0*b1 + (t & 65535u);
    return a1*b1 + (t >> 16u) + (v >> 16u);
}
fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    return vec2(a.x*b.x, high_product(a.x,b.x) + a.x*b.y + a.y*b.x);
}
fn shr64(a: vec2<u32>, n: u32) -> vec2<u32> {
    return vec2((a.x >> n) | (a.y << (32u-n)), a.y >> n);
}
fn signed64(x: i32) -> vec2<u32> {
    return vec2(bitcast<u32>(x), select(0u, 0xffffffffu, x < 0));
}
fn gradient_code(cell: vec2<i32>, salt: u32) -> u32 {
    var h = view.seed ^ vec2(salt,0u)
        ^ mul64(signed64(cell.x), vec2(0x7f4a7c15u,0x9e3779b9u))
        ^ mul64(signed64(cell.y), vec2(0x1ce4e5b9u,0xbf58476du));
    h = mul64(h ^ shr64(h,30u), vec2(0x1ce4e5b9u,0xbf58476du));
    h = mul64(h ^ shr64(h,27u), vec2(0x133111ebu,0x94d049bbu));
    return (h ^ shr64(h,31u)).x & 7u;
}
fn decode_gradient(k: u32) -> vec2<f32> {
    let s = 0.7071067811865476;
    let gradients = array<vec2<f32>,8>(vec2(1.,0.), vec2(-1.,0.), vec2(0.,1.), vec2(0.,-1.),
                          vec2(s,s), vec2(s,-s), vec2(-s,s), vec2(-s,-s));
    return gradients[k];
}
fn lattice_slot(salt: u32) -> u32 {
    if salt >= 0xa101u && salt <= 0xa106u { return salt-0xa101u; }
    if salt >= 0xa110u && salt <= 0xa112u { return salt-0xa110u+6u; }
    return 9u;
}
fn corners(c: vec2<i32>, salt: u32) -> u32 {
    return gradient_code(c,salt)
        | (gradient_code(c+vec2(1,0),salt)<<3u)
        | (gradient_code(c+vec2(0,1),salt)<<6u)
        | (gradient_code(c+vec2(1,1),salt)<<9u);
}
fn cached_corners(c: vec2<i32>, salt: u32) -> u32 {
    let slot = lattice_slot(salt);
    if view.flags.z != 0u && slot < 9u {
        let atlas = view.lattice[slot];
        let p = c-atlas.xy;
        if all(p >= vec2(0)) && all(p < vec2(atlas.z)) {
            return lattice[u32(atlas.w + p.y*atlas.z + p.x)];
        }
    }
    return corners(c,salt);
}
@compute @workgroup_size(64)
fn fill_lattice(@builtin(global_invocation_id) id: vec3<u32>) {
    let slot = id.y;
    let atlas = view.lattice[slot];
    let index = i32(id.x);
    if index >= atlas.z*atlas.z { return; }
    let c = atlas.xy+vec2(index%atlas.z,index/atlas.z);
    let salt = select(0xa101u+slot,0xa110u+slot-6u,slot>=6u);
    build_lattice[u32(atlas.w+index)] = corners(c,salt);
}

fn fade(t: vec2<f32>) -> vec2<f32> {
    return t*t*t*(t*(t*6.0-15.0)+10.0);
}
fn noise(p: vec2<f32>, salt: u32) -> f32 {
    let c = vec2<i32>(floor(p));
    let f = fract(p);
    let w = fade(f);
    let code = cached_corners(c,salt);
    let a = dot(decode_gradient(code&7u), f);
    let b = dot(decode_gradient((code>>3u)&7u), f-vec2(1.,0.));
    let d = dot(decode_gradient((code>>6u)&7u), f-vec2(0.,1.));
    let e = dot(decode_gradient((code>>9u)&7u), f-vec2(1.));
    return clamp(1.6*mix(mix(a,b,w.x),mix(d,e,w.x),w.y),-1.,1.);
}
// Value and analytic lattice derivatives of the unclamped gradient noise.
fn noise_dual(p: vec2<f32>, salt: u32) -> vec3<f32> {
    let c = vec2<i32>(floor(p));
    let f = fract(p);
    let w = fade(f);
    let dw = 30.*f*f*(f-1.)*(f-1.);
    let code = cached_corners(c,salt);
    let ga = decode_gradient(code&7u);
    let gb = decode_gradient((code>>3u)&7u);
    let gd = decode_gradient((code>>6u)&7u);
    let ge = decode_gradient((code>>9u)&7u);
    let a = dot(ga, f);
    let b = dot(gb, f-vec2(1.,0.));
    let d = dot(gd, f-vec2(0.,1.));
    let e = dot(ge, f-vec2(1.));
    let low = mix(a,b,w.x);
    let high = mix(d,e,w.x);
    let grad = mix(mix(ga,gb,w.x),mix(gd,ge,w.x),w.y);
    return 1.6*vec3(mix(low,high,w.y),grad + vec2(
        dw.x*mix(b-a,e-d,w.y),dw.y*(high-low)));
}
fn band(p: vec2<f32>, wavelength: f32, salt: u32) -> f32 {
    return noise(p/wavelength + vec2(12.7,-29.1), salt);
}
fn bump(d: f32) -> f32 {
    let q = clamp(1.-d*d,0.,1.);
    return q*q;
}
fn hill(p: vec2<f32>) -> f32 {
    let q = clamp(1.-dot(p,p),0.,1.);
    return q*q;
}
fn elevation(p: vec2<f32>, detail: bool) -> f32 {
    let meadow = 42.*band(p,750.,0xa101u) + 12.*band(p,240.,0xa102u);
    let center = 3300. + 340.*band(vec2(p.x,0.),1800.,0xa103u);
    let env = bump((p.y-center)/1900.);
    let peaks = 650.*bump((p.x+1800.)/1100.) + 470.*bump((p.x+650.)/850.)
                + 800.*bump((p.x-1100.)/1000.) + 600.*bump((p.x-2600.)/1000.);
    let crest = 500. + peaks + 100.*band(p,850.,0xa104u)
                + 55.*band(p,180.,0xa105u) + 25.*band(p,480.,0xa106u);
    var h = meadow + env*crest
          + 190.*hill((p-vec2(-900.,1500.))/vec2(1500.,900.))
          + 290.*hill((p-vec2(1550.,2000.))/vec2(1400.,1100.));
    if detail {
        var sum = 0.;
        var norm = 0.;
        var weight = 1.;
        var wavelength = 45.;
        for (var i = 0u; i < view.flags.x; i += 1u) {
            sum += weight*band(p,wavelength,0xa110u+i);
            norm += weight;
            weight *= 0.65*view.settings.z;
            wavelength *= 0.5;
        }
        h += sum/norm*(env*7.5+0.45);
        h += 0.03*band(p,1.2,0xa130u) + 0.01*band(p,0.75,0xa131u);
    }
    return h*view.settings.y;
}

fn band_dual(p: vec2<f32>, wave: f32, salt: u32) -> vec3<f32> {
    let n = noise_dual(p/wave + vec2(12.7,-29.1),salt);
    return vec3(clamp(n.x,-1.,1.),select(vec2(0.),n.yz/wave,abs(n.x)<1.));
}
fn bump_grad(x: f32) -> f32 {
    return -4.*x*max(0.,1.-x*x);
}
fn hill_grad(p: vec2<f32>) -> vec2<f32> {
    return -4.*p*max(0.,1.-dot(p,p));
}
fn alpine_cover_slope(p: vec2<f32>) -> f32 {
    let m = 42.*band_dual(p,750.,0xa101u).yz+12.*band_dual(p,240.,0xa102u).yz;
    let ctr = 340.*band_dual(vec2(p.x,0.),1800.,0xa103u);
    let dist = (p.y-3300.-ctr.x)/1900.;
    let env = bump(dist);
    let grad_env = bump_grad(dist)*vec2(-ctr.y,1.)/1900.;
    let a = (p.x+1800.)/1100.;
    let b = (p.x+650.)/850.;
    let c = (p.x-1100.)/1000.;
    let e = (p.x-2600.)/1000.;
    let peak = 650.*bump(a)+470.*bump(b)+800.*bump(c)+600.*bump(e);
    let grad_peak = 650.*bump_grad(a)/1100.+470.*bump_grad(b)/850.
                  + 800.*bump_grad(c)/1000.+600.*bump_grad(e)/1000.;
    let crest = 100.*band_dual(p,850.,0xa104u)+55.*band_dual(p,180.,0xa105u)
                +25.*band_dual(p,480.,0xa106u)+vec3(500.+peak,grad_peak,0.);
    let a_size = vec2(1500.,900.);
    let b_size = vec2(1400.,1100.);
    let hills = 190.*hill_grad((p-vec2(-900.,1500.))/a_size)/a_size
              + 290.*hill_grad((p-vec2(1550.,2000.))/b_size)/b_size;
    return length(m+env*crest.yz+crest.x*grad_env+hills)*view.settings.x*view.settings.y;
}

// A noise derivative along either lattice axis is bounded by
// 1.6 * (1 + (15/8) * 2*sqrt(2)). Clamping preserves this bound.
// Pad the interval for f32 arithmetic within the supported coordinate domain.
fn band_range(p: vec2<f32>, radius: vec2<f32>, wavelength: f32, salt: u32) -> vec2<f32> {
    let delta = radius/wavelength;
    let sum = delta.x+delta.y;
    let coarse = 10.2*sum;
    if coarse >= 2. { return vec2(-1.,1.); }
    let n = noise_dual(p/wavelength + vec2(12.7,-29.1),salt);
    // Every entry of the raw noise Hessian is bounded by 48, from
    // |fade'| <= 15/8, |fade''| <= 10/sqrt(3), and unit gradients.
    // This Taylor enclosure remains valid across lattice boundaries.
    let r = min(coarse,dot(abs(n.yz),delta)+24.*sum*sum)+0.00025;
    return clamp(vec2(n.x) + vec2(-r,r),vec2(-1.),vec2(1.));
}
fn square(a: vec2<f32>) -> vec2<f32> {
    let lo = max(0.,max(a.x,-a.y));
    let hi = max(-a.x,a.y);
    return vec2(lo*lo,hi*hi);
}
fn bump_range(d: vec2<f32>) -> vec2<f32> {
    let q = clamp(1.-square(d).yx,vec2(0.),vec2(1.));
    return q*q;
}
fn hill_range(x: vec2<f32>, z: vec2<f32>) -> vec2<f32> {
    let q = clamp(1.-(square(x)+square(z)).yx,vec2(0.),vec2(1.));
    return q*q;
}
fn product(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let p = a.x*b;
    let q = a.y*b;
    return vec2(min(min(p.x,p.y),min(q.x,q.y)),max(max(p.x,p.y),max(q.x,q.y)));
}
fn upper_height(lo: vec2<f32>, hi: vec2<f32>) -> f32 {
    let p = (lo+hi)*0.5*view.settings.x;
    let r = ((hi-lo)*0.5 + vec2(0.002))*view.settings.x;
    let x = p.x + vec2(-r.x,r.x);
    let z = p.y + vec2(-r.y,r.y);
    let meadow = 42.*band_range(p,r,750.,0xa101u) + 12.*band_range(p,r,240.,0xa102u);
    let center = 3300. + 340.*band_range(vec2(p.x,0.),vec2(r.x,0.),1800.,0xa103u);
    let env = bump_range((z-center.yx)/1900.);
    let crest = 500. + 650.*bump_range((x+1800.)/1100.) + 470.*bump_range((x+650.)/850.)
        + 800.*bump_range((x-1100.)/1000.) + 600.*bump_range((x-2600.)/1000.)
        + 100.*band_range(p,r,850.,0xa104u) + 55.*band_range(p,r,180.,0xa105u)
        + 25.*band_range(p,r,480.,0xa106u);
    let hills = 190.*hill_range((x+900.)/1500.,(z-1500.)/900.)
              + 290.*hill_range((x-1550.)/1400.,(z-2000.)/1100.);
    var sum = vec2(0.);
    var norm = 0.;
    var weight = 1.;
    var wavelength = 45.;
    for (var i = 0u; i < view.flags.x; i += 1u) {
        sum += weight*band_range(p,r,wavelength,0xa110u+i);
        norm += weight;
        weight *= 0.65*view.settings.z;
        wavelength *= 0.5;
    }
    let detailed = meadow + product(env,crest) + hills
        + product(sum/norm,env*7.5+0.45);
    // Both microrelief bands can only add 0.04 authored meters.
    let upper = (detailed.y + 0.043)*view.settings.y + 0.002;
    return floor(upper/CELL)*CELL;
}

@compute @workgroup_size(8,8)
fn build(@builtin(global_invocation_id) id: vec3<u32>) {
    let lo = vec2<f32>(view.grid+vec2<i32>(id.xy))*BLOCK;
    build_bounds[id.y*SIDE+id.x] = upper_height(lo,lo+BLOCK);
}

fn reduce_max(id: vec2<u32>, side: u32, src: u32, dst: u32) {
    let base = id*4u;
    var top = -INF;
    for (var y = 0u; y < 4u; y += 1u) {
        for (var x = 0u; x < 4u; x += 1u) {
            top = max(top,build_bounds[src+(base.y+y)*side+base.x+x]);
        }
    }
    build_bounds[dst+id.y*(side/4u)+id.x] = top;
}
@compute @workgroup_size(8,8)
fn reduce_128(@builtin(global_invocation_id) id: vec3<u32>) {
    reduce_max(id.xy,512u,0u,LEVEL1);
}
@compute @workgroup_size(8,8)
fn reduce_512(@builtin(global_invocation_id) id: vec3<u32>) {
    reduce_max(id.xy,128u,LEVEL1,LEVEL2);
}

const BEAM_SIZE: u32 = 4u;
fn ray_line(ndc: vec2<f32>) -> vec3<f32> {
    let screen = ndc*view.tile.x+view.tile.yz;
    return view.forward.xyz+view.right.xyz*screen.x+view.up.xyz*screen.y;
}
fn beam_ray_interval(lo: vec2<f32>, hi: vec2<f32>) -> array<vec3<f32>,2> {
    let a = ray_line(lo);
    let b = ray_line(vec2(hi.x,lo.y));
    let c = ray_line(vec2(lo.x,hi.y));
    let d = ray_line(hi);
    return array(min(min(a,b),min(c,d)),max(max(a,b),max(c,d)));
}
fn beam_empty(lo: vec3<f32>, hi: vec3<f32>) -> bool {
    // Check the complete ray packet footprint against at most 4x4 squares.
    // Use the finest available bound that spans the footprint cheaply.
    let below = vec2<i32>(floor(lo.xz/BLOCK))-view.grid;
    let above = vec2<i32>(floor(hi.xz/BLOCK))-view.grid;
    if any(below<vec2(0)) || any(above>=vec2(i32(SIDE))) { return false; }
    for (var i = 0u; i < 3u; i += 1u) {
        let shift = i*2u;
        let offset = array<u32,3>(0u,LEVEL1,LEVEL2)[i];
        let a = below>>vec2(shift);
        let b = above>>vec2(shift);
        if all(b-a <= vec2(3)) {
            let side = i32(SIDE>>shift);
            var top = -INF;
            for (var y = a.y; y <= b.y; y += 1) {
                for (var x = a.x; x <= b.x; x += 1) {
                    top = max(top,bounds[offset+u32(y*side+x)]);
                }
            }
            return lo.y > top+0.002;
        }
    }
    return false;
}
@compute @workgroup_size(8,8)
fn beam_prefix(@builtin(global_invocation_id) id: vec3<u32>) {
    let groups = (view.output.xy+BEAM_SIZE-1u)/BEAM_SIZE;
    if any(id.xy>=groups) { return; }
    let size = vec2<f32>(view.output.xy);
    let x0y0 = vec2<f32>(id.xy*BEAM_SIZE)/size*2.-1.;
    let x1y1 = vec2<f32>(min((id.xy+1u)*BEAM_SIZE,view.output.xy))/size*2.-1.;
    let dir = beam_ray_interval(vec2(x0y0.x,-x1y1.y),vec2(x1y1.x,-x0y0.y));
    var a = 0.025;
    var b = view.eye.w;
    var stride = b-a;
    // The visited intervals always form a prefix. A limited iteration count
    // leaves the last proved empty boundary as a valid starting point.
    for (var i = 0u; i < 96u; i += 1u) {
        let lo = view.eye.xyz+min(dir[0]*a,dir[0]*b)-vec3(0.005);
        let hi = view.eye.xyz+max(dir[1]*a,dir[1]*b)+vec3(0.005);
        if beam_empty(lo,hi) {
            a = b;
            if a >= view.eye.w { break; }
            stride = min(stride*2.,view.eye.w-a);
        } else {
            if stride <= 8. { break; }
            stride *= 0.5;
        }
        b = min(a+stride,view.eye.w);
    }
    build_beams[id.y*groups.x+id.x] = a;
}

fn cell_at(p: vec2<f32>, d: vec2<f32>, size: f32) -> vec2<i32> {
    let f = floor(p/size);
    return vec2<i32>(f) - select(vec2(0),vec2(1),(d<vec2(0.)) & (p/size==f));
}
fn grid_exit(o: vec2<f32>, d: vec2<f32>, c: vec2<i32>, size: f32) -> vec2<f32> {
    let plane = vec2<f32>(c+select(vec2(0),vec2(1),d>vec2(0.)))*size;
    return select((plane-o)/d,vec2(INF),abs(d)<vec2(1e-10));
}
fn native_height(c: vec2<i32>) -> f32 {
    let p = (vec2<f32>(c)+0.5)*(CELL*view.settings.x);
    return floor(elevation(p,true)/CELL)*CELL;
}
struct Hit { t: f32, face: u32, column: vec2<i32> }
const FINE: f32 = 4.0;
const FINE_SIDE: u32 = SIDE*8u;
const FINE_WORDS: u32 = FINE_SIDE*FINE_SIDE/2u;
const CACHE_COUNTERS: u32 = FINE_WORDS;
const BOUND_TABLE: u32 = CACHE_COUNTERS+4u;
fn pack_height(height: f32) -> u32 {
    if height <= -2048. || height >= 2048. { return 0u; }
    return u32(i32(height/CELL)+32768);
}
fn unpack_height(value: u32) -> f32 {
    return f32(i32(value)-32768)*CELL;
}
fn memoized_height(lo: vec2<f32>, index: u32) -> f32 {
    let address = index>>1u;
    let shift = (index&1u)*16u;
    let cached = (atomicLoad(&fine[address])>>shift)&65535u;
    if cached != 0u { return unpack_height(cached); }
    if FINE_MODE == 3u { return INF; }
    let height = upper_height(lo,lo+FINE);
    atomicOr(&fine[address],pack_height(height)<<shift);
    return height;
}
fn mix_cache_key(value: u32) -> u32 {
    var h = value;
    h = (h^(h>>16u))*0x7feb352du;
    h = (h^(h>>15u))*0x846ca68bu;
    return h^(h>>16u);
}
// An entry matches only on the full local coordinate. A published key with
// zero value is safe to fill concurrently because evaluation is deterministic.
fn lookup_address(base: u32, mask: u32, encoded: u32, kind: u32) -> u32 {
    if encoded == 0u { return 0u; }
    let h = mix_cache_key(encoded);
    for (var probe = 0u; probe < 12u; probe += 1u) {
        let address = base+2u*((h+probe)&mask);
        let key = atomicLoad(&fine[address]);
        if key == encoded { return address+1u; }
        if key == 0u && FINE_MODE != 3u {
            let claim = atomicCompareExchangeWeak(&fine[address],0u,encoded);
            if claim.exchanged {
                atomicAdd(&fine[CACHE_COUNTERS+kind],1u);
                return address+1u;
            }
            if claim.old_value == encoded { return address+1u; }
        }
    }
    if FINE_MODE != 3u { atomicAdd(&fine[CACHE_COUNTERS+2u+kind],1u); }
    return 0u;
}
fn sub_bound(cell: vec2<i32>) -> f32 {
    let p = vec2<u32>(cell-view.grid*64);
    let key = 1u+p.x+(p.y<<15u);
    let address = lookup_address(BOUND_TABLE,view.cache.x-1u,key,0u);
    if address != 0u {
        let value = atomicLoad(&fine[address]);
        if value != 0u { return unpack_height(value); }
    }
    if FINE_MODE == 3u { return INF; }
    let lo = vec2<f32>(cell)*0.5;
    let height = upper_height(lo,lo+0.5);
    let encoded = pack_height(height);
    if address != 0u && encoded != 0u { atomicStore(&fine[address],encoded); }
    return height;
}
fn cached_native_height(c: vec2<i32>) -> f32 {
    if FINE_MODE < 2u { return native_height(c); }
    let p = vec2<u32>(c-view.grid*512);
    if any(p>=vec2(262144u)) {
        if FINE_MODE == 3u { return INF; }
        return native_height(c);
    }
    let shard = (p.x&3u)+4u*(p.y&3u);
    let key = 1u+(p.x>>2u)+((p.y>>2u)<<16u);
    let shard_size = view.cache.y/16u;
    let base = BOUND_TABLE+2u*view.cache.x+2u*shard_size*shard;
    let address = lookup_address(base,shard_size-1u,key,1u);
    if address != 0u {
        let value = atomicLoad(&fine[address]);
        if value != 0u { return unpack_height(value); }
    }
    if FINE_MODE == 3u { return INF; }
    let height = native_height(c);
    let encoded = pack_height(height);
    if address != 0u && encoded != 0u { atomicStore(&fine[address],encoded); }
    return height;
}
fn native_walk(o: vec3<f32>, d: vec3<f32>, step: vec2<i32>, start: f32, end: f32, entry: u32) -> Hit {
    var c = cell_at(fma(d.xz,vec2(start-0.002),o.xz),d.xz,CELL);
    var face = entry;
    var enter = start;
    for (var k = 0u; k < 26u; k += 1u) {
        let ce = grid_exit(o.xz,d.xz,c,CELL);
        let exit = min(ce.x,ce.y);
        if exit > enter {
            let h = cached_native_height(c);
            if h == INF { return Hit(-2.,0u,vec2(0)); }
            if fma(d.y,enter,o.y) <= h { return Hit(enter,face,c); }
            if d.y < 0. {
                let roof = (h-o.y)/d.y;
                if roof >= enter && roof < min(exit,end) { return Hit(roof,0u,c); }
            }
        }
        if exit >= end { return Hit(-1.,0u,vec2(0)); }
        enter = max(start,exit);
        if ce.x <= ce.y {
            c.x += step.x;
            face = 1u;
        }
        if ce.y <= ce.x {
            c.y += step.y;
            face = 2u;
        }
    }
    return Hit(-2.,0u,vec2(0));
}
fn fine_walk(o: vec3<f32>, d: vec3<f32>, step: vec2<i32>, start: f32, end: f32, entry: u32, block: vec2<i32>) -> Hit {
    let base = block*8;
    var cell = clamp(cell_at(fma(d.xz,vec2(start),o.xz),d.xz,FINE),base,base+7);
    var face = entry;
    var a = start;
    for (var i = 0u; i < 17u; i += 1u) {
        let ce = grid_exit(o.xz,d.xz,cell,FINE);
        let b = min(end,min(ce.x,ce.y));
        if b > a {
            let p = vec2<u32>(cell-view.grid*8);
            let key = p.y*FINE_SIDE+p.x;
            let top = memoized_height(vec2<f32>(cell)*FINE,key);
            if top == INF { return Hit(-2.,0u,vec2(0)); }
            if min(fma(d.y,a,o.y),fma(d.y,b,o.y)) <= top+0.002 {
                let child_base = cell*8;
                var child = clamp(cell_at(fma(d.xz,vec2(a),o.xz),d.xz,0.5),child_base,child_base+7);
                var l = a;
                var child_face = face;
                var finished = false;
                for (var j = 0u; j < 17u; j += 1u) {
                    let edge = grid_exit(o.xz,d.xz,child,0.5);
                    let r = min(b,min(edge.x,edge.y));
                    if r > l {
                        let upper = sub_bound(child);
                        if upper == INF { return Hit(-2.,0u,vec2(0)); }
                        if min(fma(d.y,l,o.y),fma(d.y,r,o.y)) <= upper+0.002 {
                            let hit = native_walk(o,d,step,l,r,child_face);
                            if hit.t != -1. { return hit; }
                        }
                    }
                    if r >= b { finished = true; break; }
                    l = max(l,r);
                    if edge.x <= edge.y { child.x += step.x; child_face = 1u; }
                    if edge.y <= edge.x { child.y += step.y; child_face = 2u; }
                }
                if !finished { return Hit(-2.,0u,vec2(0)); }
            }
        }
        if b >= end { return Hit(-1.,0u,vec2(0)); }
        a = max(a,b);
        if ce.x <= ce.y { cell.x += step.x; face = 1u; }
        if ce.y <= ce.x { cell.y += step.y; face = 2u; }
    }
    return Hit(-2.,0u,vec2(0));
}
fn trace(o: vec3<f32>, d: vec3<f32>, start: f32, end: f32) -> Hit {
    let miss = Hit(-1.,0u,vec2(0));
    let unresolved = Hit(-2.,0u,vec2(0));
    let step = select(vec2(-1),vec2(1),d.xz>vec2(0.));
    var block = cell_at(fma(d.xz,vec2(start),o.xz),d.xz,BLOCK);
    var a = start;
    var entry = 0u;
    var work = 0u;
    for (var i = 0u; i < 512u; i += 1u) {
        let slot = block-view.grid;
        if any(slot<vec2(0)) || any(slot>=vec2(i32(SIDE))) { return unresolved; }
        var skipped = false;
        for (var i = 0u; i < 2u; i += 1u) {
            let shift = select(4u,2u,i==1u);
            let group = slot >> vec2(shift);
            let base = view.grid + (group << vec2(shift));
            let span = 1i << shift;
            let boundary = base + select(vec2(0),vec2(span),d.xz>vec2(0.));
            let exit = select((vec2<f32>(boundary)*BLOCK-o.xz)/d.xz,vec2(INF),abs(d.xz)<vec2(1e-10));
            let next = min(end,min(exit.x,exit.y));
            let offset = select(LEVEL2,LEVEL1,i==1u);
            let side = SIDE >> shift;
            let top = bounds[offset+u32(group.y)*side+u32(group.x)];
            if next > a && min(fma(d.y,a,o.y),fma(d.y,next,o.y)) > top+0.002 {
                if next >= end { return miss; }
                let position = fma(d.xz,vec2(next),o.xz);
                block = clamp(cell_at(position,d.xz,BLOCK),base,base+span-1);
                if exit.x <= exit.y {
                    block.x = boundary.x - select(1,0,step.x>0);
                    entry = 1u;
                }
                if exit.y <= exit.x {
                    block.y = boundary.y - select(1,0,step.y>0);
                    entry = 2u;
                }
                a = next;
                skipped = true;
                break;
            }
        }
        if skipped { continue; }
        let edge = grid_exit(o.xz,d.xz,block,BLOCK);
        let b = min(end,min(edge.x,edge.y));
        let min_y = min(fma(d.y,a,o.y),fma(d.y,b,o.y));
        if b > a && min_y <= bounds[u32(slot.y)*SIDE+u32(slot.x)]+0.002 {
            if FINE_MODE != 0u {
                let hit = fine_walk(o,d,step,a,b,entry,block);
                if hit.t != -1. { return hit; }
            } else {

            // Depth-first subdivision. A 32 m tile needs at most eight
            // subdivision levels before the terminal walk covers a few cells.
            var ends: array<f32,10>;
            var count = 0u;
            var l = a;
            var r = b;
            var normal = entry;
            var completed = false;
            for (var j = 0u; j < 1024u; j += 1u) {
                work += 1u;
                if work > 1536u { return unresolved; }
                let begin = fma(d.xz,vec2(l),o.xz);
                let finish = fma(d.xz,vec2(r),o.xz);
                if max(abs(finish.x-begin.x),abs(finish.y-begin.y)) <= 0.20 {
                    // Overlap both segment endpoints by a millimeter to account for
                    // f32 address rounding. Test the analytic entry times against
                    // the original interval to retain first-hit order.
                    var c = cell_at(fma(d.xz,vec2(max(start,l-0.002)),o.xz),d.xz,CELL);
                    var face = normal;
                    var enter = l;
                    for (var k = 0u; k < 12u; k += 1u) {
                        let ce = grid_exit(o.xz,d.xz,c,CELL);
                        let exit = min(ce.x,ce.y);
                        if exit > enter {
                            let h = native_height(c);
                            if fma(d.y,enter,o.y) <= h { return Hit(enter,face,c); }
                            if d.y < 0. {
                                let roof = (h-o.y)/d.y;
                                if roof >= enter && roof < min(exit,r) { return Hit(roof,0u,c); }
                            }
                        }
                        if exit >= r { break; }
                        enter = max(l,exit);
                        if ce.x <= ce.y {
                            c.x += step.x;
                            face = 1u;
                        }
                        if ce.y <= ce.x {
                            c.y += step.y;
                            face = 2u;
                        }
                    }
                } else {
                    // Bounds surround the native centers of every crossed column.
                    let lo = min(begin,finish)-CELL*0.5;
                    let hi = max(begin,finish)+CELL*0.5;
                    let y = min(fma(d.y,l,o.y),fma(d.y,r,o.y));
                    if y <= upper_height(lo,hi)+0.002 {
                        if count >= 10u { return unresolved; }
                        ends[count] = r;
                        count += 1u;
                        r = (l+r)*0.5;
                        continue;
                    }
                }
                if count == 0u { completed = true; break; }
                count -= 1u;
                l = r;
                r = ends[count];
                normal = 0u;
            }
            if !completed { return unresolved; }
            }
        }
        if b >= end { return miss; }
        a = max(a,b);
        if edge.x <= edge.y {
            block.x += step.x;
            entry = 1u;
        }
        if edge.y <= edge.x {
            block.y += step.y;
            entry = 2u;
        }
    }
    return unresolved;
}

override SWEEP_HEIGHT: u32 = 8u;
fn pack_hit(hit: Hit) -> vec4<u32> {
    return vec4(bitcast<u32>(hit.t), hit.face, bitcast<vec2<u32>>(hit.column));
}
fn unpack_hit(packed: vec4<u32>) -> Hit {
    return Hit(bitcast<f32>(packed.x),packed.y,bitcast<vec2<i32>>(packed.zw));
}
fn first_depth(pixel: vec2<u32>) -> f32 {
    if view.output.z == 0u { return 0.025; }
    let slot = pixel/BEAM_SIZE;
    let stride = (view.output.x+BEAM_SIZE-1u)/BEAM_SIZE;
    return beams[slot.y*stride+slot.x];
}
@compute @workgroup_size(32,2)
fn sweep_columns(@builtin(global_invocation_id) id: vec3<u32>) {
    let rows = view.output.y;
    let width = view.output.x;
    let low = id.y*SWEEP_HEIGHT;
    if id.x >= width || low >= rows { return; }
    let hi = min(low+SWEEP_HEIGHT,rows);
    let px = f32(id.x)+0.5;
    let o = view.eye.xyz;
    var previous = 0.025;
    var all_sky = false;
    for (var y = hi; y > low; y -= 1u) {
        let row = y-1u;
        var hit = Hit(-1.,0u,vec2(0));
        if !all_sky {
            let ndc = (vec2(px,f32(row)+0.5)/vec2<f32>(view.output.xy)*2.-1.)*vec2(1.,-1.);
            let d = normalize(ray_line(ndc));
            let forward = dot(d,view.forward.xyz);
            let prefix = first_depth(vec2(id.x,row));
            if prefix < view.eye.w {
                // For a level camera, rays in a screen column agree in xz at
                // every forward depth. The next ray lies above the previous.
                let a = max(0.025,max(prefix,previous)-0.006);
                hit = trace(o,d,a/forward,view.eye.w/forward);
                if hit.t >= 0. { previous = hit.t*forward; }
            }
            all_sky = hit.t == -1.;
        }
        build_hits[row*width+id.x] = pack_hit(hit);
    }
}

fn slope_for_row(row: u32) -> f32 {
    let y = 1.-(f32(row)+0.5)*2./f32(view.output.y);
    return (y*view.tile.x+view.tile.z)*view.up.y;
}
fn above_bound(slope: f32, near_t: f32, far_t: f32, upper: f32) -> bool {
    return min(fma(slope,near_t,view.eye.y),fma(slope,far_t,view.eye.y)) > upper+0.002;
}
// A level camera's screen column is a vertical plane. Traverse its xz
// footprint once and emit every newly visible row before advancing the plane.
@compute @workgroup_size(32,2)
fn sweep_visible_columns(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = view.output.x;
    let low = id.y*SWEEP_HEIGHT;
    if id.x >= width || low >= view.output.y { return; }
    var row = min(low+SWEEP_HEIGHT,view.output.y);
    let x = (f32(id.x)+0.5)*2./f32(width)-1.;
    let horiz = ray_line(vec2(x,0.)).xz;
    let step = select(vec2(-1),vec2(1),horiz>vec2(0.));
    let o = view.eye.xz;
    let end = view.eye.w;
    let prefix = first_depth(vec2(id.x,row-1u));
    var a = select(max(0.025,prefix-0.006),end,prefix>=end);
    var work = 0u;
    var face = 0u;
    var slope = slope_for_row(row-1u);
    var block = cell_at(fma(horiz,vec2(a),o),horiz,BLOCK);
    for (var outer = 0u; outer < 512u && row > low && a < end; outer += 1u) {
        let slot = block-view.grid;
        if any(slot<vec2(0)) || any(slot>=vec2(i32(SIDE))) { break; }
        var advanced = false;
        for (var level = 0u; level < 2u; level += 1u) {
            let shift = select(4u,2u,level==1u);
            let group = slot>>vec2(shift);
            let base = view.grid+(group<<vec2(shift));
            let span = 1i<<shift;
            let boundary = base+select(vec2(0),vec2(span),horiz>vec2(0.));
            let t = select((vec2<f32>(boundary)*BLOCK-o)/horiz,vec2(INF),abs(horiz)<vec2(1e-10));
            let stop = min(end,min(t.x,t.y));
            let offset = select(LEVEL2,LEVEL1,level==1u);
            let index = offset+u32(group.y)*(SIDE>>shift)+u32(group.x);
            if stop > a && above_bound(slope,a,stop,bounds[index]) {
                a = stop;
                block = clamp(cell_at(fma(horiz,vec2(stop),o),horiz,BLOCK),base,base+span-1);
                if t.x <= t.y { block.x = boundary.x-select(1,0,step.x>0); face = 1u; }
                if t.y <= t.x { block.y = boundary.y-select(1,0,step.y>0); face = 2u; }
                advanced = true;
                break;
            }
        }
        if advanced { continue; }
        let cross = grid_exit(o,horiz,block,BLOCK);
        let edge = min(end,min(cross.x,cross.y));
        let bound = bounds[u32(slot.y)*SIDE+u32(slot.x)];
        if edge > a && !above_bound(slope,a,edge,bound) {
        var stack: array<f32,10>;
        var count = 0u;
        var l = a;
        var r = edge;
        var entry = face;
        for (var inner = 0u; inner < 1024u && row > low; inner += 1u) {
            work += 1u;
            if work > max(1536u,SWEEP_HEIGHT*96u) { break; }
            let begin = fma(horiz,vec2(l),o);
            let finish = fma(horiz,vec2(r),o);
            if max(abs(finish.x-begin.x),abs(finish.y-begin.y)) <= 0.20 {
                var c = cell_at(fma(horiz,vec2(l-0.002),o),horiz,CELL);
                var enter = l;
                for (var k = 0u; k < 12u && row > low; k += 1u) {
                    let ce = grid_exit(o,horiz,c,CELL);
                    let exit = min(ce.x,ce.y);
                    if min(exit,r) > enter {
                        let top = native_height(c);
                        while row > low {
                            var t = enter;
                            var hit_face = entry;
                            if fma(slope,enter,view.eye.y) > top {
                                if slope >= 0. { break; }
                                t = (top-view.eye.y)/slope;
                                if t < enter || t >= min(exit,r) { break; }
                                hit_face = 0u;
                            }
                            let distance = t*length(vec3(horiz.x,slope,horiz.y));
                            build_hits[(row-1u)*width+id.x] = pack_hit(Hit(distance,hit_face,c));
                            row -= 1u;
                            if row > low { slope = slope_for_row(row-1u); }
                        }
                    }
                    if exit >= r { break; }
                    enter = max(l,exit);
                    if ce.x <= ce.y { c.x += step.x; entry = 1u; }
                    if ce.y <= ce.x { c.y += step.y; entry = 2u; }
                }
            } else {
                let lo = min(begin,finish)-CELL*0.5;
                let hi = max(begin,finish)+CELL*0.5;
                if !above_bound(slope,l,r,upper_height(lo,hi)) {
                    stack[count] = r;
                    count += 1u;
                    r = (l+r)*0.5;
                    continue;
                }
            }
            if count == 0u { break; }
            count -= 1u;
            l = r;
            r = stack[count];
            entry = 0u;
        }
        }
        if work > max(1536u,SWEEP_HEIGHT*96u) { break; }
        a = max(a,edge);
        if cross.x <= cross.y { block.x += step.x; face = 1u; }
        if cross.y <= cross.x { block.y += step.y; face = 2u; }
    }
    let status = select(-2.,-1.,a>=end && work<=max(1536u,SWEEP_HEIGHT*96u));
    while row > low {
        row -= 1u;
        build_hits[row*width+id.x] = pack_hit(Hit(status,0u,vec2(0)));
    }
}

struct Vertex { @builtin(position) position: vec4<f32>, @location(0) ndc: vec2<f32> }
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let p = array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.))[index];
    return Vertex(vec4(p,0.,1.),p);
}
struct Fragment { @builtin(frag_depth) depth: f32, @location(0) color: vec4<f32> }
@fragment fn fragment(in: Vertex) -> Fragment {
    let d = normalize(ray_line(in.ndc));
    let forward = dot(d,view.forward.xyz);
    let o = view.eye.xyz;
    let pixel = vec2<u32>(in.position.xy);
    var hit = Hit(-1.,0u,vec2(0));
    if view.output.w != 0u {
        hit = unpack_hit(swept_hits[pixel.y*view.output.x+pixel.x]);
    } else {
        let prefix = first_depth(pixel);
        if prefix >= view.eye.w { discard; }
        let start = max(0.025,prefix-0.005);
        hit = trace(o,d,start/forward,view.eye.w/forward);
    }
    if hit.t == -1. { discard; }
    if hit.t == -2. { return Fragment(0.99999994,vec4(1.,0.,1.,1.)); }
    let delta = d*hit.t;
    let clip = view.projection*vec4(delta,1.);
    if view.flags.y == 1u {
        return Fragment(clip.z/clip.w,vec4(hit.t, f32(hit.face), 0., 1.));
    }
    let p = (vec2<f32>(hit.column)+0.5)*(CELL*view.settings.x);
    let slope = alpine_cover_slope(p);
    let cover = 0.75*noise(p/120.+vec2(3.7,-8.1),0xac01u)
                + 0.25*noise(p/28.+vec2(-17.2,4.8),0xac02u);
    let h = o.y+delta.y;
    let stone = slope>0.68+0.20*cover || h>1000.*view.settings.y*(0.93+0.14*cover);
    var albedo = select(vec3(0.18,0.34,0.10),vec3(0.35,0.37,0.39),stone);
    // Approximate the exposed soil band and trail for the visual diagnostic.
    if !stone && hit.face != 0u { albedo = vec3(0.32,0.22,0.13); }
    let trail = 16.+60.*sin(p.y/310.)+0.055*p.y;
    if p.y>-550. && p.y<1500. && abs(p.x-trail)<0.55 { albedo=vec3(0.32,0.22,0.13); }
    var n = vec3(0.,1.,0.);
    if hit.face == 1u { n = vec3(-sign(d.x),0.,0.); }
    if hit.face == 2u { n = vec3(0.,0.,-sign(d.z)); }
    let light = 0.48 + 0.52*max(0.,dot(n,view.sun.xyz));
    return Fragment(clip.z/clip.w,vec4(albedo*light,1.));
}
